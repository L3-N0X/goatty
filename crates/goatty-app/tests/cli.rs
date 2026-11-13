//! `goatty settings …` and `goatty theme …`: no window, no display, a
//! sandboxed `$HOME`, and a socket path with no daemon behind it (host
//! settings then go to the sandbox's own host.json).

use std::path::Path;
use std::process::{Command, Output, Stdio};

fn goatty(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_goatty"))
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env_remove("XDG_DATA_HOME")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("GOATTY_THEME")
        .env_remove("GOATTY_FONT")
        .env_remove("GOATTY_UI_FONT")
        .env_remove("GOATTY_FONT_SIZE")
        .env("GOATTY_SOCKET", home.join("none.sock"))
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn ok(o: Output) -> Output {
    assert!(o.status.success(), "stdout:\n{}\nstderr:\n{}", out(&o), err(&o));
    o
}

#[test]
fn settings_set_show_validate() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let file = home.join(".config/goatty/settings.json");
    assert!(out(&ok(goatty(home, &["settings", "path"]))).contains(&file.display().to_string()));

    ok(goatty(home, &["settings", "set", "font.size", "16"]));
    ok(goatty(home, &["settings", "set", "font.ui_family", "Cantarell"]));
    ok(goatty(home, &["settings", "set", "keybindings.ctrl-k", "paste"]));
    assert_eq!(out(&ok(goatty(home, &["settings", "show", "font.size"]))).trim(), "16.0");
    assert_eq!(out(&ok(goatty(home, &["settings", "show", "font.ui_family"]))).trim(), "\"Cantarell\"");
    let stored = out(&ok(goatty(home, &["settings", "show"])));
    assert!(stored.contains("\"size\": 16.0") && stored.contains("\"ui_family\": \"Cantarell\"") && stored.contains("ctrl-k"), "{stored}");
    assert!(!goatty(home, &["settings", "set", "font.size", "huge"]).status.success());
    assert!(!goatty(home, &["settings", "set", "keybindings.ctrl-j", "explode"]).status.success());
    ok(goatty(home, &["settings", "validate"]));

    std::fs::write(&file, r#"{"version": 1, "theme": {"name": "nope"}, "keybindings": {"ctrl-+-+": "paste"}}"#).unwrap();
    let o = goatty(home, &["settings", "validate"]);
    assert!(!o.status.success());
    assert!(err(&o).contains("theme.name") && err(&o).contains("keybindings.ctrl-+-+"), "{}", err(&o));

    ok(goatty(home, &["settings", "unset", "theme.name"]));
    assert!(!std::fs::read_to_string(&file).unwrap().contains("nope"));
}

#[test]
fn themes_import_set_export_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let scheme = home.join("ocean.yaml");
    let mut yaml = String::from("scheme: \"Ocean Test\"\n");
    for (i, k) in ["00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "0A", "0B", "0C", "0D", "0E", "0F"].iter().enumerate() {
        yaml += &format!("base{k}: \"{:02x}{:02x}{:02x}\"\n", i * 16, 64, 128);
    }
    std::fs::write(&scheme, yaml).unwrap();

    let o = ok(goatty(home, &["theme", "import", scheme.to_str().unwrap()]));
    assert!(out(&o).contains("as ocean-test"), "{}", out(&o));
    assert!(home.join(".config/goatty/themes/ocean-test.json").is_file());
    let list = out(&ok(goatty(home, &["theme", "list"])));
    assert!(list.contains("ocean-test") && list.contains("mine") && list.contains("nord"), "{list}");

    ok(goatty(home, &["theme", "set", "ocean-test", "--for", "dark"]));
    ok(goatty(home, &["theme", "set", "solarized-light", "--for", "light"]));
    let theme = out(&ok(goatty(home, &["settings", "show", "theme"])));
    assert!(theme.contains("\"follow_system\": true") && theme.contains("\"dark\": \"ocean-test\""), "{theme}");
    assert!(out(&ok(goatty(home, &["theme", "list"]))).contains("dark"));
    assert!(!goatty(home, &["theme", "set", "does-not-exist"]).status.success());

    let exported = out(&ok(goatty(home, &["theme", "export", "ocean-test"])));
    assert!(exported.contains("\"name\": \"Ocean Test\""), "{exported}");
    // Importing an export again gets a fresh id.
    let again = home.join("again.json");
    std::fs::write(&again, exported).unwrap();
    assert!(out(&ok(goatty(home, &["theme", "import", again.to_str().unwrap()]))).contains("ocean-test-2"));

    ok(goatty(home, &["theme", "delete", "ocean-test-2"]));
    assert!(!goatty(home, &["theme", "delete", "nord"]).status.success(), "built-ins can't be deleted");
}

#[test]
fn profile_export_import_without_daemon() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    ok(goatty(home, &["settings", "set", "theme.name", "dracula"]));
    std::fs::create_dir_all(home.join(".config/goatty")).unwrap();
    std::fs::write(home.join(".config/goatty/host.json"), "{\"version\": 1, \"history\": {\"size\": 4321}}\n").unwrap();

    let profile = home.join("profile.json");
    ok(goatty(home, &["settings", "export", profile.to_str().unwrap()]));
    let text = std::fs::read_to_string(&profile).unwrap();
    assert!(text.contains("dracula") && text.contains("4321"), "{text}");

    // A second machine imports it (no terminal: needs --yes).
    let other = tempfile::tempdir().unwrap();
    assert!(!goatty(other.path(), &["settings", "import", profile.to_str().unwrap()]).status.success());
    ok(goatty(other.path(), &["settings", "import", "--yes", profile.to_str().unwrap()]));
    assert!(std::fs::read_to_string(other.path().join(".config/goatty/settings.json")).unwrap().contains("dracula"));
    assert!(std::fs::read_to_string(other.path().join(".config/goatty/host.json")).unwrap().contains("4321"));
    assert!(other.path().join(".local/share/goatty/current/zsh/.zshrc").is_file(), "host settings were applied as a generation");
}

#[test]
fn profile_import_lists_uninstalled_plugins() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    let reg = tmp.path().join("registry");
    let key_path = tmp.path().join("reg.key");
    let public = goatty_plugin::sign::keygen_file(&key_path).unwrap();
    let key = goatty_plugin::sign::load_secret(&key_path).unwrap();
    let plugin_dir = reg.join("plugins/greet/0.1.0");
    std::fs::create_dir_all(plugin_dir.join("zsh")).unwrap();
    std::fs::write(reg.join(goatty_plugin::index::META), format!("name = \"test\"\nkeys = [\"{public}\"]\n")).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        "schema = 1\nid = \"greet\"\nname = \"Greet\"\nversion = \"0.1.0\"\nsummary = \"Says hi\"\npublisher = \"tests\"\n[zsh]\ninit = \"zsh/init.zsh\"\n[permissions]\nshell-code = \"defines greet\"\n",
    )
    .unwrap();
    std::fs::write(plugin_dir.join("zsh/init.zsh"), "greet() { echo hi }\n").unwrap();
    let idx = goatty_plugin::index::build(&reg).unwrap();
    std::fs::write(reg.join(goatty_plugin::index::INDEX), idx.to_bytes()).unwrap();
    goatty_plugin::index::sign_dir(&reg, &key).unwrap();

    // Point the sandbox at the local signed registry instead of the network.
    let config_dir = home.join(".config/goatty");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("registries.json"),
        serde_json::json!({
            "version": 1,
            "builtin": false,
            "registries": [{ "name": "test", "url": reg, "keys": [public] }]
        })
        .to_string(),
    )
    .unwrap();

    let profile = tmp.path().join("profile.json");
    std::fs::write(&profile, r#"{"goatty_profile": 1, "host": {"version": 3, "plugins": {"greet": {"settings": {"name": "ada"}}}}}"#)
        .unwrap();

    let o = ok(goatty(home, &["settings", "import", "--yes", profile.to_str().unwrap()]));
    let stdout = out(&o);
    let stderr = err(&o);
    assert!(!stderr.contains("plugins.greet: not installed on this host"), "redundant issue should be suppressed:\n{stderr}");
    assert!(stdout.contains("1 plugin(s) in this profile are not installed"), "{stdout}");
    assert!(stdout.contains("greet (test) 0.1.0"), "{stdout}");
    assert!(stdout.contains("Review and install with: goattyd plugin install greet"), "{stdout}");
    assert!(!home.join(".local/share/goatty/plugins/installed/greet").exists(), "--yes must not bypass plan approval");
}
