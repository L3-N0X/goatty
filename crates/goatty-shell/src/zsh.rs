//! The managed ZDOTDIR: goatty's zsh startup files, rendered from the host
//! settings.
//!
//! The startup files themselves are static (`zsh/*.zsh` in this crate, so
//! they can be read and syntax-checked as they are). Everything that
//! depends on settings or paths goes into the generated `settings.zsh`
//! (sourced first by `.zshenv`) and `starship.toml` ([`crate::starship`]). [`crate::generation`]
//! turns the rendered files into an activated generation. See
//! `docs/managed-zsh.md` for the startup sequence.

use std::path::Path;

use goatty_config::HostSettings;
use goatty_plugin::manifest::Load;

use crate::builtins;
use crate::dirs::Dirs;
use crate::plugins::Active;

pub const ZSHENV: &str = include_str!("../zsh/zshenv.zsh");
pub const ZPROFILE: &str = include_str!("../zsh/zprofile.zsh");
pub const ZSHRC: &str = include_str!("../zsh/zshrc.zsh");
pub const INTEGRATION: &str = include_str!("../zsh/integration.zsh");

/// Inputs for [`render`].
pub struct ZshConfig<'a> {
    pub settings: &'a HostSettings,
    pub dirs: &'a Dirs,
    /// The goattyd binary `goatty-setup` runs. `None`: `goattyd` from PATH.
    pub daemon_exe: Option<&'a Path>,
    /// Enabled plugins, built-in and installed, in load order
    /// ([`crate::plugins::active`]).
    pub plugins: &'a [Active],
}

/// A file of the managed ZDOTDIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    /// Path relative to the ZDOTDIR (`plugins/<id>/…` for plugin files).
    pub name: String,
    pub contents: String,
}

impl GeneratedFile {
    pub fn new(name: impl Into<String>, contents: impl Into<String>) -> Self {
        Self { name: name.into(), contents: contents.into() }
    }
}

/// Every file of the managed ZDOTDIR.
pub fn render(cfg: &ZshConfig<'_>) -> Vec<GeneratedFile> {
    let mut files = vec![
        GeneratedFile::new("settings.zsh", render_settings(cfg)),
        GeneratedFile::new(".zshenv", ZSHENV),
        GeneratedFile::new(".zprofile", ZPROFILE),
        GeneratedFile::new(".zshrc", ZSHRC),
        GeneratedFile::new("integration.zsh", INTEGRATION),
        GeneratedFile::new("starship.toml", crate::starship::render(cfg.settings)),
        GeneratedFile::new("aliases.zsh", crate::aliases::render(cfg.settings)),
    ];
    files.extend(render_plugins(cfg));
    files
}

/// What each slot file says about itself.
fn slot_header(load: Load) -> &'static str {
    match load {
        Load::LineEditor => "line editor tools (after compinit, before the prompt): fzf, fzf-tab, autosuggestions, …",
        Load::Prompt => "the prompt (goatty sets a small one when none loads)",
        Load::Normal => "aliases and the plugins' shell code (after the prompt, before your aliases and additions)",
        Load::Last => "what has to load after your additions (syntax highlighting wraps every widget defined before it)",
    }
}

/// `plugins-early.zsh` and one file per load slot (always written, so the
/// `.zshrc` can source them unconditionally), plus each plugin's
/// generated config files and the shell files it ships (an installed
/// plugin's checked against the lock again).
fn render_plugins(cfg: &ZshConfig<'_>) -> Vec<GeneratedFile> {
    const NOTE: &str = "# Generated from the plugins and their settings in host.json: change them in goatty's settings window.\n";
    let mut early = format!("# goatty plugins: completion paths and environment (read before compinit).\n{NOTE}");
    // Built-in plugins that are turned off still show up in goatty-status.
    for b in builtins::BUILTINS.iter().filter(|b| cfg.settings.plugins.get(b.id).is_some_and(|c| !c.enabled)) {
        early.push_str(&format!("_goatty_state[{}]=off\n", b.id));
    }
    let mut slots: Vec<(Load, String)> = Load::ALL.iter().map(|&l| (l, format!("# goatty plugins: {}.\n{NOTE}", slot_header(l)))).collect();
    let mut files = Vec::new();
    let home = std::env::var("HOME").ok();
    for a in cfg.plugins {
        let id = &a.manifest.id;
        // Shipped files first: a plugin whose files don't check out is left out entirely.
        let mut shipped = Vec::new();
        let checked: Result<(), String> = a.shell_files().into_iter().try_for_each(|rel| {
            let bytes = a.read(cfg.dirs, &rel).map_err(|e| format!("{e:#}"))?;
            let text = String::from_utf8(bytes).map_err(|_| format!("{id}: {rel} isn't text"))?;
            shipped.push(GeneratedFile::new(format!("plugins/{id}/{rel}"), text));
            Ok(())
        });
        if let Err(e) = checked {
            log::warn!("leaving plugin {id} out of the shell configuration: {e}");
            continue;
        }
        let ctx = goatty_plugin::zsh::Ctx {
            dir: format!("$_goatty_dir/plugins/{id}"),
            home: home.as_deref(),
            label: a.label.clone(),
            feature: a.feature(),
            packages: "$_goatty_pkgs".into(),
        };
        let r = goatty_plugin::zsh::render(&a.manifest, &a.values, &ctx);
        if !r.early.is_empty() {
            early.push('\n');
            early.push_str(&r.early);
        }
        let slot = &mut slots.iter_mut().find(|(l, _)| *l == r.load).expect("every slot").1;
        slot.push('\n');
        slot.push_str(&r.main);
        files.extend(shipped);
        files.extend(r.files.into_iter().map(|(name, contents)| GeneratedFile::new(format!("plugins/{id}/{name}"), contents)));
    }
    let mut out = vec![GeneratedFile::new("plugins-early.zsh", early)];
    out.extend(slots.into_iter().map(|(l, text)| GeneratedFile::new(l.file(), text)));
    out.extend(files);
    out
}

/// goatty's own features (not plugins) as the `.zshrc` knows them, for the
/// enabled ones.
pub fn enabled_features(s: &HostSettings) -> Vec<&'static str> {
    [("integration", s.shell.zsh.integration)].into_iter().filter_map(|(name, on)| on.then_some(name)).collect()
}

fn flag(b: bool) -> &'static str {
    if b { "1" } else { "0" }
}

fn render_settings(cfg: &ZshConfig<'_>) -> String {
    let d = cfg.dirs;
    let s = cfg.settings;
    let mut out = String::from(
        "# goatty managed zsh: settings and paths, read first by .zshenv.\n\
         # Generated by goatty from host.json: change settings in goatty's settings window, not here.\n",
    );
    let mut scalar = |name: &str, value: &str| out.push_str(&format!("typeset -g {name}={}\n", quote(value)));
    scalar("_goatty_dir", &d.zsh_dir().to_string_lossy());
    scalar("_goatty_bin", &d.bin_dir().to_string_lossy());
    scalar("_goatty_pkgs", &d.plugins_dir().to_string_lossy());
    scalar("_goatty_cache", &d.cache.to_string_lossy());
    scalar("_goatty_user_zshrc", &d.user_zshrc().to_string_lossy());
    scalar("_goatty_hint_marker", &d.hint_marker().to_string_lossy());
    scalar("_goatty_offer_marker", &d.offer_marker().to_string_lossy());
    scalar("_goatty_daemon", &cfg.daemon_exe.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default());
    scalar("_goatty_user_env", flag(s.shell.zsh.user_env));
    scalar("_goatty_user_rc", flag(s.shell.zsh.user_rc));
    scalar("_goatty_hist_size", &s.history.size.to_string());
    scalar("_goatty_prompt_two_line", flag(s.prompt.two_line));
    scalar("_goatty_prompt_transient", flag(s.prompt.transient));
    scalar("_goatty_transient_ok", &crate::starship::transient_prompt(&s.prompt, false));
    scalar("_goatty_transient_err", &crate::starship::transient_prompt(&s.prompt, true));

    let h = &s.history;
    let mut hist = vec!["extended_history", "hist_expire_dups_first", "hist_reduce_blanks", "hist_verify"];
    if h.ignore_duplicates {
        hist.push("hist_ignore_dups");
    }
    if h.ignore_space {
        hist.push("hist_ignore_space");
    }
    hist.push(if h.share_between_sessions { "share_history" } else { "inc_append_history" });
    out.push_str(&format!("typeset -ga _goatty_hist_opts=({})\n", hist.join(" ")));
    out.push_str(&format!("typeset -ga _goatty_features=({})\n", enabled_features(s).join(" ")));
    // What goatty-setup installs when they're missing (built-in plugins that are on).
    let defaults: Vec<&str> =
        builtins::BUILTINS.iter().filter(|b| b.default_install && s.plugins.get(b.id).is_none_or(|c| c.enabled)).map(|b| b.id).collect();
    out.push_str(&format!("typeset -ga _goatty_setup_defaults=({})\n", defaults.join(" ")));
    out
}

/// Quote for zsh: single quotes, with `'` as `'\''`.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(quote("/a b/c"), "'/a b/c'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote(""), "''");
    }

    #[test]
    fn settings_reflect_toggles() {
        let dirs = Dirs::under(Path::new("/r o/ot's"));
        let mut s = HostSettings::default();
        s.plugins.entry("starship".into()).or_default().enabled = false;
        s.shell.zsh.user_rc = true;
        s.history.size = 5000;
        s.history.share_between_sessions = true;
        let out = render_settings(&ZshConfig { settings: &s, dirs: &dirs, daemon_exe: Some(Path::new("/usr/bin/goattyd")), plugins: &[] });
        assert!(out.contains(r"_goatty_dir='/r o/ot'\''s/data/current/zsh'"), "{out}");
        assert!(out.contains("_goatty_user_rc='1'"));
        assert!(out.contains("_goatty_hist_size='5000'"));
        assert!(out.contains("hist_ignore_space share_history)"), "{out}");
        assert!(out.contains("_goatty_daemon='/usr/bin/goattyd'"));
        assert!(out.contains("_goatty_features=(integration)"));
        assert!(out.contains(r"_goatty_pkgs='/r o/ot'\''s/data/plugins'"), "{out}");
        assert!(out.contains("_goatty_setup_defaults=(fzf fzf-tab autosuggestions syntax-highlighting)"), "starship is off: {out}");
    }

    #[test]
    fn plugins_render_into_their_slots_in_order() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let mut s = HostSettings::default();
        s.plugins.entry("starship".into()).or_default().enabled = false;
        s.plugins.entry("fzf-tab".into()).or_default().settings.insert("previews".into(), false.into());
        let (plugins, issues) = crate::plugins::active(&dirs, &s);
        assert!(issues.is_empty(), "{issues:?}");
        let files = render(&ZshConfig { settings: &s, dirs: &dirs, daemon_exe: None, plugins: &plugins });
        let get = |n: &str| &files.iter().find(|f| f.name == n).unwrap_or_else(|| panic!("no {n}")).contents;
        assert!(get("plugins-early.zsh").contains("_goatty_state[starship]=off"));
        let line = get("plugins-line-editor.zsh");
        let pos = |id: &str| line.find(&format!("# --- {id} ")).unwrap_or_else(|| panic!("no {id}:\n{line}"));
        assert!(pos("fzf") < pos("fzf-tab"), "fzf-tab wraps Tab after fzf's bindings:\n{line}");
        assert!(pos("fzf-tab") < pos("autosuggestions"), "their widgets exist before autosuggestions wraps them:\n{line}");
        assert!(line.contains("goatty_settings=(previews 'false')"), "{line}");
        assert!(line.contains("if (( $+commands[fzf] && ${+_goatty_src[fzf-tab]} )); then"), "fzf-tab needs fzf: {line}");
        assert!(line.contains("_goatty_state[fzf]=\"ok\""), "built-in plugins report under their id: {line}");
        assert!(!line.contains("atuin"), "{line}");
        assert!(!get("plugins-prompt.zsh").contains("_goatty_prompt=starship"), "starship is off");
        assert!(get("plugins-last.zsh").contains("_goatty_src[syntax-highlighting]"));
        assert!(!get("plugins.zsh").contains("# ---"), "nothing in the normal slot");
        assert!(files.iter().any(|f| f.name == "plugins/fzf-tab/zsh/init.zsh"), "the shipped init file");
        let rc = get("plugins/fzf/fzfrc");
        assert!(rc.contains("--color=fg:-1,bg:-1") && rc.contains("--preview-window='right:60%"), "{rc}");
        assert!(!rc.lines().any(|l| l.trim_start().starts_with('#')), "fzf < 0.55 rejects comments: {rc}");
    }

    /// fzf's key bindings paste goatty's options file into
    /// FZF_DEFAULT_OPTS before their own options (Ctrl+R's `--read0`):
    /// a value fzf can't split would break them all.
    #[test]
    fn fzf_options_survive_the_key_bindings() {
        let Some(fzf) = crate::launch::which("fzf") else {
            eprintln!("fzf not installed; skipping");
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let s = HostSettings::default();
        let (plugins, _) = crate::plugins::active(&dirs, &s);
        let files = render(&ZshConfig { settings: &s, dirs: &dirs, daemon_exe: None, plugins: &plugins });
        let rc = &files.iter().find(|f| f.name == "plugins/fzf/fzfrc").unwrap().contents;
        let mut child = std::process::Command::new(fzf)
            .args(["--filter", ""])
            .env("FZF_DEFAULT_OPTS", format!("--height 40%\n{rc} --read0"))
            .env_remove("FZF_DEFAULT_OPTS_FILE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::Write;
        child.stdin.take().unwrap().write_all(b"one\0two\0").unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "one\ntwo\n", "{}", String::from_utf8_lossy(&out.stderr));
    }
}
