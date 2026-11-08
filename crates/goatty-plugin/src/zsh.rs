//! A plugin's zsh integration, rendered from its manifest and settings.
//!
//! Two parts go into the shell configuration generation:
//!
//! * **early** (`plugins-early.zsh`, before `compinit`): `fpath` entries and
//!   exported variables, so completion and the init code see them;
//! * **main** (in the file of the plugin's [`Load`] slot, e.g.
//!   `plugins.zsh`): aliases, the plugin's init code and its packages' zsh
//!   scripts, only when its commands and scripts are installed.
//!
//! Everything goatty writes is quoted here; the only code a plugin brings
//! is its `zsh.init` file, which is sourced as is (`shell-code` permission),
//! and the scripts of its packages (`binaries`, checked like programs).

use serde_json::Value;

use crate::manifest::{FileFormat, Load, Manifest, SettingKind, applies};
use crate::schema::{Values, plain_string};
use crate::template::{expand_inline, expand_list};

/// Quote for zsh: single quotes, `'` as `'\''`.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Quote only when needed (readable aliases: `eza --icons=auto`).
pub fn word(s: &str) -> String {
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_./=:,+@%-".contains(&b)) && !s.starts_with('=') {
        s.to_owned()
    } else {
        quote(s)
    }
}

/// Where things are, as seen from the generated files.
pub struct Ctx<'a> {
    /// zsh expression (safe inside double quotes) for the plugin's
    /// directory in the generation, e.g. `$_goatty_dir/plugins/eza`.
    pub dir: String,
    /// `$HOME` on the host, for `~` in path settings.
    pub home: Option<&'a str>,
    /// Shown in the generated comment: `eza 1.0.0 from official`.
    pub label: String,
    /// The key in `goatty-status` (`plugin:eza`; built-in plugins use their id).
    pub feature: String,
    /// zsh expression (safe inside double quotes) for the directory holding
    /// goatty's copies of zsh-plugin packages (`plugins/<package>/`).
    pub packages: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rendered {
    pub early: String,
    pub main: String,
    /// Generated config files: (name inside the plugin's directory, contents).
    pub files: Vec<(String, String)>,
    /// Files of the plugin directory the shell needs (init file, fpath dirs'
    /// contents are copied by the caller: these are the referenced paths).
    pub uses_init: Option<String>,
    pub fpath: Vec<String>,
    /// The slot `main` goes into.
    pub load: Load,
}

/// `~` and `~/x` in path settings and list entries become `$HOME`/`$HOME/x`
/// (the value as it's used: quoted values never see tilde expansion).
pub fn expand_home(m: &Manifest, values: &mut Values, home: Option<&str>) {
    let Some(home) = home else { return };
    let expand = |p: &mut String| {
        if p == "~" {
            *p = home.to_owned();
        } else if let Some(rest) = p.strip_prefix("~/") {
            *p = format!("{}/{rest}", home.trim_end_matches('/'));
        }
    };
    for s in &m.settings {
        match (s.kind, values.get_mut(&s.id)) {
            (SettingKind::Path, Some(Value::String(p))) => expand(p),
            (SettingKind::List, Some(Value::Array(items))) => items
                .iter_mut()
                .filter_map(|i| match i {
                    Value::String(p) => Some(p),
                    _ => None,
                })
                .for_each(expand),
            _ => {}
        }
    }
}

/// The feature name in `goatty-status` (`plugin:eza`).
pub fn feature(id: &str) -> String {
    format!("plugin:{id}")
}

pub fn render(m: &Manifest, values: &Values, ctx: &Ctx<'_>) -> Rendered {
    let mut values = values.clone();
    expand_home(m, &mut values, ctx.home);
    let values = &values;
    let mut r = Rendered::default();

    // --- early: fpath, variables, config files -----------------------------
    let mut early = Vec::new();
    if applies(&m.zsh.when, values) {
        for d in &m.zsh.fpath {
            early.push(format!("fpath=(\"{}/{d}\" $fpath)", ctx.dir));
            r.fpath.push(d.clone());
        }
    }
    for e in m.env.iter().filter(|e| applies(&e.when, values)) {
        let v = expand_inline(&e.value, m, values, None);
        if !v.is_empty() {
            early.push(format!("builtin export {}={}", e.name, quote(&v)));
        }
    }
    for f in m.files.iter().filter(|f| applies(&f.when, values)) {
        let contents = match f.format {
            FileFormat::Lines => {
                let mut s = expand_list(&f.lines, m, values).join("\n");
                s.push('\n');
                s
            }
            FileFormat::Text => expand_inline(f.template.as_deref().unwrap_or_default(), m, values, None),
        };
        r.files.push((f.name.clone(), contents));
        if let Some(var) = &f.env {
            early.push(format!("builtin export {var}=\"{}/{}\"", ctx.dir, f.name));
        }
    }
    if !early.is_empty() {
        r.early = format!("# {}\n{}\n", ctx.label, early.join("\n"));
    }

    // --- main: aliases, init code and package scripts, when they're there ---
    // Unquoted on purpose: zsh keeps quotes in an assoc subscript as part of
    // the key. Ids are `[a-z0-9-]`, which needs no quoting there.
    r.load = m.zsh.load;
    let feature = &ctx.feature;
    let commands: Vec<&str> = m.executables().chain(m.requires.commands.iter().map(String::as_str)).collect();
    // Each package script: the first of the system's copies and goatty's own.
    let mut find = Vec::new();
    for (p, src) in m.sources() {
        let mut candidates: Vec<String> = if p.prefer_system { p.system_paths.iter().map(|s| quote(s)).collect() } else { Vec::new() };
        candidates.push(format!("\"{}/{}/{src}\"", ctx.packages, p.id));
        find.push(format!("_goatty_find {} && _goatty_src[{}]=$REPLY", candidates.join(" "), p.id));
    }
    let mut body = Vec::new();
    for a in m.aliases.iter().filter(|a| applies(&a.when, values)) {
        body.push(format!("alias -- {}={}", a.name, quote(&crate::alias::expansion(m, a, values))));
    }
    if let Some(init) = m.zsh.init.as_ref().filter(|_| applies(&m.zsh.when, values)) {
        let pairs: Vec<String> =
            m.settings.iter().map(|s| format!("{} {}", s.id, quote(&values.get(&s.id).map(plain_string).unwrap_or_default()))).collect();
        body.push(format!("typeset -gA goatty_settings=({})", pairs.join(" ")));
        body.push(format!("typeset -g goatty_plugin_dir=\"{}\"", ctx.dir));
        body.push(format!("builtin source -- \"$goatty_plugin_dir/{init}\""));
        body.push("builtin unset goatty_settings goatty_plugin_dir".into());
        r.uses_init = Some(init.clone());
    }
    for (p, _) in m.sources() {
        body.push(format!("builtin source -- \"${{_goatty_src[{}]}}\"", p.id));
    }
    if m.zsh.load == Load::Prompt {
        body.push(format!("typeset -g _goatty_prompt={}", m.id));
    }
    let detail = match (m.sources().next(), commands.first()) {
        (Some((p, _)), _) => format!("\"${{_goatty_src[{}]}}\"", p.id),
        (None, Some(cmd)) => format!("\"$commands[{cmd}]\""),
        (None, None) => quote(&m.version),
    };
    body.push(format!("_goatty_state[{feature}]=\"ok\"$'\\t'{detail}"));
    let mut main = format!("# --- {} ---\n", ctx.label);
    for line in &find {
        main.push_str(line);
        main.push('\n');
    }
    let tests: Vec<String> = commands
        .iter()
        .map(|c| format!("$+commands[{c}]"))
        .chain(m.sources().map(|(p, _)| format!("${{+_goatty_src[{}]}}", p.id)))
        .collect();
    if tests.is_empty() {
        main.push_str(&body.join("\n"));
        main.push('\n');
    } else {
        main.push_str(&format!("if (( {} )); then\n", tests.join(" && ")));
        for line in &body {
            main.push_str(&format!("  {line}\n"));
        }
        main.push_str(&format!("else\n  _goatty_state[{feature}]=missing\nfi\n"));
    }
    r.main = main;
    r
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::manifest::{parse, tests::EXAMPLE};
    use crate::schema::resolve;

    fn ctx() -> Ctx<'static> {
        Ctx {
            dir: "$_goatty_dir/plugins/eza".into(),
            home: Some("/home/u"),
            label: "eza 1.0.0".into(),
            feature: feature("eza"),
            packages: "$_goatty_pkgs".into(),
        }
    }

    #[test]
    fn renders_aliases_env_and_status() {
        let m = parse(EXAMPLE, None).unwrap();
        let (v, _) = resolve(&m, &BTreeMap::new());
        let r = render(&m, &v, &ctx());
        assert_eq!(r.early, "# eza 1.0.0\nbuiltin export EZA_ICON_SPACING='2'\n");
        assert!(r.main.contains("if (( $+commands[eza] )); then\n  alias -- ls='eza --icons=auto'\n"), "{}", r.main);
        assert!(r.main.contains("_goatty_state[plugin:eza]=\"ok\"$'\\t'\"$commands[eza]\""), "{}", r.main);
        assert!(r.main.contains("else\n  _goatty_state[plugin:eza]=missing\nfi"));

        let off: BTreeMap<String, Value> = serde_json::from_str(r#"{"replace_ls": false, "icons": "never"}"#).unwrap();
        let r = render(&m, &resolve(&m, &off).0, &ctx());
        assert!(!r.main.contains("alias"));
        assert!(r.early.is_empty(), "env only when icons != never");
    }

    #[test]
    fn values_cannot_escape_quotes() {
        let text = EXAMPLE
            .replace("[[alias]]", "[[setting]]\nid = \"extra\"\nlabel = \"Extra\"\ntype = \"string\"\narg = \"--x={value}\"\n\n[[alias]]");
        let m = parse(&text, None).unwrap();
        let stored: BTreeMap<String, Value> = serde_json::from_str(r#"{"extra": "'; rm -rf ~; '"}"#).unwrap();
        let r = render(&m, &resolve(&m, &stored).0, &ctx());
        let line = r.main.lines().find(|l| l.contains("alias")).unwrap().trim();
        // Ask zsh what the alias expands to: three words, the value intact.
        let Ok(out) = std::process::Command::new("zsh")
            .args(["-f", "-c", &format!("{line}; eval \"words=(${{aliases[ls]}})\"; print -rl -- $words")])
            .output()
        else {
            eprintln!("zsh not installed; skipping");
            return;
        };
        assert_eq!(String::from_utf8_lossy(&out.stdout), "eza\n--icons=auto\n--x='; rm -rf ~; '\n", "{line}");
    }

    #[test]
    fn init_code_and_files() {
        let text = EXAMPLE.replace("[permissions]", "[zsh]\ninit = \"zsh/init.zsh\"\nfpath = [\"zsh/completions\"]\n\n[[file]]\nname = \"config\"\nenv = \"EZA_CONFIG\"\nlines = [\"{args}\", \"# icons: {icons}\"]\n\n[permissions]\nshell-code = \"sets up the eza helper function\"");
        let m = parse(&text, None).unwrap();
        let r = render(&m, &resolve(&m, &BTreeMap::new()).0, &ctx());
        assert!(r.early.contains("fpath=(\"$_goatty_dir/plugins/eza/zsh/completions\" $fpath)"));
        assert!(r.early.contains("builtin export EZA_CONFIG=\"$_goatty_dir/plugins/eza/config\""));
        assert_eq!(r.files, [("config".to_string(), "--icons=auto\n# icons: auto\n".to_string())]);
        assert!(r.main.contains("typeset -gA goatty_settings=(replace_ls 'true' icons 'auto' git 'false')"), "{}", r.main);
        assert!(r.main.contains("builtin source -- \"$goatty_plugin_dir/zsh/init.zsh\""));
        assert_eq!(r.uses_init.as_deref(), Some("zsh/init.zsh"));
    }

    #[test]
    fn package_scripts_are_found_and_sourced_after_init() {
        let text = r#"
schema = 1
id = "suggest"
name = "Suggest"
version = "1.0.0"
summary = "zsh plugin"
publisher = "goatty"

[[package]]
id = "zsh-suggest"
version = "0.1"
source = "suggest.zsh"
system_paths = ["/usr/share/zsh/plugins/suggest/suggest.zsh"]
[[package.asset]]
target = "any"
url = "https://example.com/s.tar.gz"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
size = 1

[zsh]
init = "init.zsh"
load = "prompt"

[permissions]
binaries = "the zsh plugin itself, from its release tarball"
shell-code = "sets the plugin's options before it loads"
"#;
        let m = parse(text, None).unwrap();
        let r = render(&m, &BTreeMap::new(), &Ctx { feature: "suggest".into(), ..ctx() });
        assert_eq!(r.load, Load::Prompt);
        let main = &r.main;
        assert!(
            main.contains("_goatty_find '/usr/share/zsh/plugins/suggest/suggest.zsh' \"$_goatty_pkgs/zsh-suggest/suggest.zsh\" && _goatty_src[zsh-suggest]=$REPLY\nif (( ${+_goatty_src[zsh-suggest]} )); then"),
            "{main}"
        );
        let init = main.find("init.zsh").unwrap();
        let source = main.find("builtin source -- \"${_goatty_src[zsh-suggest]}\"").unwrap();
        assert!(init < source, "init runs first, so it can set the plugin's variables: {main}");
        assert!(main.contains("typeset -g _goatty_prompt=suggest"), "{main}");
        assert!(main.contains("_goatty_state[suggest]=\"ok\"$'\\t'\"${_goatty_src[zsh-suggest]}\""), "{main}");
        assert!(main.contains("_goatty_state[suggest]=missing"), "{main}");
    }

    #[test]
    fn words() {
        assert_eq!(word("--icons=auto"), "--icons=auto");
        assert_eq!(word("a b"), "'a b'");
        assert_eq!(word("$HOME"), "'$HOME'");
        assert_eq!(word(""), "''");
    }
}
