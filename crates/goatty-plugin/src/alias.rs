//! A plugin's aliases as the user sees them (#88): the name, what it runs
//! with the current settings, and whether the condition it depends on
//! holds. The settings window's Aliases page, `goattyd alias list` and the
//! install plan show these.

use crate::manifest::{Alias, Condition, Manifest, SettingKind, applies};
use crate::schema::Values;
use crate::template::expand_list;
use crate::zsh::word;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AliasView {
    pub name: String,
    /// What it runs, as the shell gets it: `eza --long --icons=auto`.
    pub expansion: String,
    pub description: String,
    /// The plugin setting it depends on, in words (`"Short aliases" is
    /// on`); `None` when it's always defined.
    pub condition: Option<String>,
    /// The condition holds with these settings.
    pub on: bool,
}

/// What `a` runs with `values`: the command and its expanded arguments,
/// each quoted only where the shell needs it (the alias's value in
/// `plugins.zsh`).
pub fn expansion(m: &Manifest, a: &Alias, values: &Values) -> String {
    std::iter::once(word(&a.command)).chain(expand_list(&a.args, m, values).iter().map(|w| word(w))).collect::<Vec<_>>().join(" ")
}

/// One row per alias name, in manifest order. A name defined for several
/// conditions shows the definition that applies, else the first.
pub fn views(m: &Manifest, values: &Values) -> Vec<AliasView> {
    let mut out: Vec<AliasView> = Vec::new();
    for a in &m.aliases {
        let on = applies(&a.when, values);
        let view = AliasView {
            name: a.name.clone(),
            expansion: expansion(m, a, values),
            description: a.description.clone(),
            condition: a.when.as_deref().map(|w| describe(m, w)),
            on,
        };
        match out.iter_mut().find(|v| v.name == a.name) {
            Some(existing) if on && !existing.on => *existing = view,
            Some(_) => {}
            None => out.push(view),
        }
    }
    out
}

/// A `when` condition in words, with the setting's label and choice labels:
/// `"Short aliases" is on`, `"Icons" is "Always"`.
pub fn describe(m: &Manifest, when: &str) -> String {
    let Ok(c) = Condition::parse(when) else { return when.to_owned() };
    let setting = m.setting(c.setting());
    let label = setting.map_or_else(|| c.setting().to_owned(), |s| s.label.clone());
    let choice = |v: &str| {
        setting
            .filter(|s| s.kind == SettingKind::Choice)
            .and_then(|s| s.choices.iter().find(|ch| ch.value == v))
            .map_or_else(|| v.to_owned(), |ch| ch.label.clone())
    };
    match &c {
        Condition::On(_) => format!("\"{label}\" is on"),
        Condition::Off(_) => format!("\"{label}\" is off"),
        Condition::Is(_, v) => format!("\"{label}\" is \"{}\"", choice(v)),
        Condition::IsNot(_, v) => format!("\"{label}\" isn't \"{}\"", choice(v)),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::manifest::parse;
    use crate::schema::resolve;

    const M: &str = r#"
schema = 1
id = "demo"
name = "Demo"
version = "1.0.0"
summary = "s"
publisher = "p"
[requires]
commands = ["demo"]
[[setting]]
id = "short"
label = "Short aliases"
type = "bool"
default = true
[[setting]]
id = "mode"
label = "Mode"
type = "choice"
default = "a"
choices = [{ value = "a", label = "Plain" }, { value = "b", label = "Fancy", arg = "--fancy" }]
arg = ""
[[alias]]
name = "d"
command = "demo"
args = ["{mode.arg}", "it's"]
when = "short"
description = "Run demo"
[[alias]]
name = "dm"
command = "demo"
when = "mode=a"
[[alias]]
name = "dm"
command = "demo"
args = ["--x"]
when = "mode=b"
"#;

    fn values(m: &Manifest, set: &[(&str, serde_json::Value)]) -> Values {
        let stored: BTreeMap<String, serde_json::Value> = set.iter().map(|(k, v)| ((*k).into(), v.clone())).collect();
        resolve(m, &stored).0
    }

    #[test]
    fn views_follow_the_settings() {
        let m = parse(M, None).unwrap();
        let v = views(&m, &values(&m, &[]));
        assert_eq!(v.len(), 2, "one row per name: {v:?}");
        assert_eq!(v[0].name, "d");
        assert_eq!(v[0].expansion, r"demo 'it'\''s'");
        assert_eq!(v[0].condition.as_deref(), Some("\"Short aliases\" is on"));
        assert!(v[0].on);
        assert_eq!(v[1].expansion, "demo", "the applicable dm");
        assert_eq!(v[1].condition.as_deref(), Some("\"Mode\" is \"Plain\""));

        let v = views(&m, &values(&m, &[("short", false.into()), ("mode", "b".into())]));
        assert!(!v[0].on);
        assert_eq!(v[0].expansion, r"demo --fancy 'it'\''s'");
        assert_eq!(v[1].expansion, "demo --x");
        assert!(v[1].on);
    }
}
