//! Templates in manifests: how setting values reach aliases, environment
//! variables and config files.
//!
//! * `{id}`: the value of setting `id` as text (bools `true`/`false`,
//!   lists joined with the setting's `separator`).
//! * `{id.arg}`, as a whole list item: the setting's command-line
//!   argument(s) (see `Setting::arg`); may expand to nothing.
//! * `{args}`, as a whole list item: every setting's arguments, in manifest order.
//! * Inside a string (env values, text files), `{id.arg}` and `{args}` are
//!   the arguments joined with spaces, each quoted for a POSIX shell where
//!   needed (for variables like `FZF_DEFAULT_COMMAND` that hold a command).
//! * `{value}`: inside a setting's own `arg`, its value.
//! * `{{` and `}}`: literal braces.
//!
//! goatty quotes the results for the shell itself: a value can never turn
//! into shell syntax.

use crate::manifest::{Manifest, Setting, SettingKind};
use crate::schema::{Values, plain_string};

#[derive(Debug, PartialEq, Eq)]
enum Part<'a> {
    Text(String),
    Var(&'a str),
}

fn parts(t: &str) -> Result<Vec<Part<'_>>, String> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = t;
    while let Some(i) = rest.find(['{', '}']) {
        text.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            text.push(tail.as_bytes()[0] as char);
            rest = &tail[2..];
        } else if tail.starts_with('}') {
            return Err(format!("{t:?}: unmatched '}}' (write }}}} for a literal one)"));
        } else {
            let end = tail.find('}').ok_or_else(|| format!("{t:?}: unclosed '{{' (write {{{{ for a literal one)"))?;
            if !text.is_empty() {
                out.push(Part::Text(std::mem::take(&mut text)));
            }
            out.push(Part::Var(&tail[1..end]));
            rest = &tail[end + 1..];
        }
    }
    text.push_str(rest);
    if !text.is_empty() {
        out.push(Part::Text(text));
    }
    Ok(out)
}

/// Check a template used inside a single string. `allow_value`: it's a
/// setting's own `arg` (`{value}` allowed, other settings' arguments not).
pub fn check_inline(t: &str, m: &Manifest, allow_value: bool) -> Result<(), String> {
    for p in parts(t)? {
        if let Part::Var(v) = p {
            if v == "value" && allow_value {
                continue;
            }
            if allow_value && (v == "args" || v.ends_with(".arg")) {
                return Err(format!("{{{v}}}: a setting's arg can't contain other arguments"));
            }
            if v == "args" {
                continue;
            }
            if let Some(id) = v.strip_suffix(".arg") {
                check_item(&format!("{{{id}.arg}}"), m)?;
                continue;
            }
            if m.setting(v).is_none() {
                return Err(format!("{{{v}}}: no setting {v:?}"));
            }
        }
    }
    Ok(())
}

/// Check a list item (alias args, `lines`): `{args}`, `{id.arg}`, or an
/// inline template.
pub fn check_item(t: &str, m: &Manifest) -> Result<(), String> {
    if t == "{args}" {
        return Ok(());
    }
    if let Some(id) = t.strip_prefix('{').and_then(|r| r.strip_suffix(".arg}")) {
        return match m.setting(id) {
            Some(s) if s.arg.is_some() || s.arg_off.is_some() || s.kind == SettingKind::Choice => Ok(()),
            Some(_) => Err(format!("{t}: setting {id:?} has no arg")),
            None => Err(format!("{t}: no setting {id:?}")),
        };
    }
    check_inline(t, m, false)
}

/// `{id}` → value text.
fn value_text(m: &Manifest, values: &Values, id: &str) -> String {
    let Some(v) = values.get(id) else { return String::new() };
    match (m.setting(id), v) {
        (Some(s), serde_json::Value::Array(items)) => {
            let sep = s.separator.as_deref().unwrap_or(" ");
            items.iter().map(plain_string).collect::<Vec<_>>().join(sep)
        }
        _ => plain_string(v),
    }
}

/// Expand an inline template (`{value}` → `value`, when given).
pub fn expand_inline(t: &str, m: &Manifest, values: &Values, value: Option<&str>) -> String {
    let Ok(parts) = parts(t) else { return String::new() };
    parts
        .into_iter()
        .map(|p| match p {
            Part::Text(s) => s,
            Part::Var("value") if value.is_some() => value.unwrap_or_default().to_owned(),
            Part::Var(v) if v == "args" || v.ends_with(".arg") => {
                expand_item(&format!("{{{v}}}"), m, values).iter().map(|a| crate::zsh::word(a)).collect::<Vec<_>>().join(" ")
            }
            Part::Var(id) => value_text(m, values, id),
        })
        .collect()
}

/// The command-line arguments setting `s` adds with its current value.
pub fn setting_args(s: &Setting, m: &Manifest, values: &Values) -> Vec<String> {
    let Some(v) = values.get(&s.id) else { return Vec::new() };
    let fill = |t: &str, value: &str| expand_inline(t, m, values, Some(value));
    match s.kind {
        SettingKind::Bool => {
            let t = if v.as_bool().unwrap_or(false) { &s.arg } else { &s.arg_off };
            t.iter().filter(|t| !t.is_empty()).map(|t| fill(t, "")).collect()
        }
        SettingKind::Choice => {
            let value = plain_string(v);
            let choice = s.choices.iter().find(|c| c.value == value);
            match choice.and_then(|c| c.arg.as_deref()).or(s.arg.as_deref()) {
                Some("") | None => Vec::new(),
                Some(t) => vec![fill(t, &value)],
            }
        }
        SettingKind::List => match &s.arg {
            Some(t) => v.as_array().into_iter().flatten().map(|i| fill(t, &plain_string(i))).collect(),
            None => Vec::new(),
        },
        SettingKind::String | SettingKind::Path | SettingKind::Integer => {
            let value = plain_string(v);
            match &s.arg {
                Some(t) if !value.is_empty() => vec![fill(t, &value)],
                _ => Vec::new(),
            }
        }
    }
}

/// Expand a list item into zero or more items. An inline item that comes
/// out empty is dropped.
pub fn expand_item(t: &str, m: &Manifest, values: &Values) -> Vec<String> {
    if t == "{args}" {
        return m.settings.iter().flat_map(|s| setting_args(s, m, values)).collect();
    }
    if let Some(id) = t.strip_prefix('{').and_then(|r| r.strip_suffix(".arg}")) {
        return m.setting(id).map(|s| setting_args(s, m, values)).unwrap_or_default();
    }
    let s = expand_inline(t, m, values, None);
    if s.is_empty() { Vec::new() } else { vec![s] }
}

/// Expand a whole list (alias args, `lines`).
pub fn expand_list(items: &[String], m: &Manifest, values: &Values) -> Vec<String> {
    items.iter().flat_map(|t| expand_item(t, m, values)).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::manifest::{parse, tests::EXAMPLE};
    use crate::schema::resolve;

    #[test]
    fn splits_parts() {
        assert_eq!(parts("a{b}c").unwrap(), [Part::Text("a".into()), Part::Var("b"), Part::Text("c".into())]);
        assert_eq!(parts("{{x}}").unwrap(), [Part::Text("{x}".into())]);
        assert!(parts("a{b").is_err());
        assert!(parts("a}b").is_err());
    }

    #[test]
    fn expands_args_and_values() {
        let m = parse(EXAMPLE, None).unwrap();
        let mut stored = BTreeMap::new();
        let (v, _) = resolve(&m, &stored);
        assert_eq!(expand_item("{args}", &m, &v), ["--icons=auto"]);
        stored.insert("git".to_string(), serde_json::Value::Bool(true));
        stored.insert("icons".to_string(), serde_json::Value::from("never"));
        let (v, _) = resolve(&m, &stored);
        assert_eq!(expand_item("{args}", &m, &v), ["--git"], "never has arg = \"\"");
        assert_eq!(expand_item("{icons.arg}", &m, &v), Vec::<String>::new());
        assert_eq!(expand_inline("icons={icons} git={git} {{lit}}", &m, &v, None), "icons=never git=true {lit}");
        assert_eq!(expand_item("{icons}", &m, &v), ["never"]);

        assert!(check_item("{args}", &m).is_ok());
        assert!(check_item("{git.arg}", &m).is_ok());
        assert!(check_item("{replace_ls.arg}", &m).is_err(), "no arg");
        assert!(check_inline("{git.arg}", &m, true).is_err(), "not inside a setting's own arg");
        assert!(check_inline("{replace_ls.arg}", &m, false).is_err(), "no arg");
        assert_eq!(expand_inline("eza {args} x", &m, &v, None), "eza --git x");
        assert!(check_inline("{value}", &m, false).is_err());
        assert!(check_inline("{value}", &m, true).is_ok());
    }
}
