//! Plugin settings: checking definitions, and turning what's stored in
//! `host.json` (`plugins.<id>.settings`) into a value for every setting.
//!
//! Stored values are checked against the definition like the rest of the
//! settings: a bad one falls back to the default and is reported, the
//! others are kept.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::manifest::{Manifest, Setting, SettingKind};

/// A value for every setting of a plugin, by setting id.
pub type Values = BTreeMap<String, Value>;

/// Longest string a setting accepts.
pub const MAX_STRING: usize = 1024;
/// Most entries a list setting accepts.
pub const MAX_LIST: usize = 64;

/// Problems with a setting definition: (field, message).
pub(crate) fn check_definition(s: &Setting) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    let kind = s.kind;
    match kind {
        SettingKind::Choice => {
            if s.choices.len() < 2 {
                out.push(("choices", "a choice needs at least two options".into()));
            }
            let mut seen = std::collections::BTreeSet::new();
            for c in &s.choices {
                if c.value.is_empty() || c.value.len() > 64 || c.value.chars().any(char::is_control) {
                    out.push(("choices", format!("{:?} is not a usable value", c.value)));
                }
                if !seen.insert(&c.value) {
                    out.push(("choices", format!("{:?} appears twice", c.value)));
                }
                if c.label.trim().is_empty() {
                    out.push(("choices", format!("{:?} has no label", c.value)));
                }
            }
        }
        _ if !s.choices.is_empty() => out.push(("choices", format!("only for type = \"choice\", not {}", kind.label()))),
        _ => {}
    }
    if kind != SettingKind::Integer && (s.min.is_some() || s.max.is_some() || s.step.is_some() || s.unit.is_some()) {
        out.push(("min", "min/max/step/unit are for integers".into()));
    }
    if let (Some(min), Some(max)) = (s.min, s.max)
        && min > max
    {
        out.push(("min", format!("{min} is above max {max}")));
    }
    if s.step.is_some_and(|st| st <= 0) {
        out.push(("step", "must be positive".into()));
    }
    if kind != SettingKind::List && s.separator.is_some() {
        out.push(("separator", "only for lists".into()));
    }
    if kind != SettingKind::Bool && s.arg_off.is_some() {
        out.push(("arg_off", "only for bools".into()));
    }
    if kind == SettingKind::Bool && s.arg.as_deref().is_some_and(|a| a.contains("{value}")) {
        out.push(("arg", "a bool's arg is added as is when on; {value} has no meaning".into()));
    }
    match &s.default {
        None if matches!(kind, SettingKind::Bool | SettingKind::Choice | SettingKind::Integer) => {
            out.push(("default", format!("a {} needs a default", kind.label())));
        }
        None => {}
        Some(d) => {
            if let Err(e) = check_value(s, d) {
                out.push(("default", e));
            }
        }
    }
    out
}

/// Whether `v` is a valid value for `s`; why not.
pub fn check_value(s: &Setting, v: &Value) -> Result<(), String> {
    let text_ok = |t: &str| -> Result<(), String> {
        if t.len() > MAX_STRING {
            return Err(format!("longer than {MAX_STRING} characters"));
        }
        if t.chars().any(char::is_control) {
            return Err("contains control characters (like a line break)".into());
        }
        Ok(())
    };
    match (s.kind, v) {
        (SettingKind::Bool, Value::Bool(_)) => Ok(()),
        (SettingKind::Choice, Value::String(c)) => {
            if s.choices.iter().any(|x| x.value == *c) {
                Ok(())
            } else {
                Err(format!("{c:?} is not one of {}", s.choices.iter().map(|c| c.value.as_str()).collect::<Vec<_>>().join(", ")))
            }
        }
        (SettingKind::String | SettingKind::Path, Value::String(t)) => text_ok(t),
        (SettingKind::Integer, Value::Number(n)) => {
            let n = n.as_i64().ok_or_else(|| format!("{n} is not a whole number"))?;
            if s.min.is_some_and(|m| n < m) || s.max.is_some_and(|m| n > m) {
                return Err(format!(
                    "{n} is outside {}–{}",
                    s.min.map_or("…".into(), |m| m.to_string()),
                    s.max.map_or("…".into(), |m| m.to_string())
                ));
            }
            Ok(())
        }
        (SettingKind::List, Value::Array(items)) => {
            if items.len() > MAX_LIST {
                return Err(format!("more than {MAX_LIST} entries"));
            }
            for i in items {
                match i {
                    Value::String(t) => text_ok(t)?,
                    other => return Err(format!("entries must be text, not {other}")),
                }
            }
            Ok(())
        }
        (kind, other) => Err(format!("expected a {}, got {other}", kind.label())),
    }
}

/// The value a setting has when the user didn't set one.
pub fn default_value(s: &Setting) -> Value {
    s.default.clone().unwrap_or(match s.kind {
        SettingKind::Bool => Value::Bool(false),
        SettingKind::Integer => Value::from(s.min.unwrap_or(0)),
        SettingKind::List => Value::Array(Vec::new()),
        SettingKind::Choice => Value::String(s.choices.first().map(|c| c.value.clone()).unwrap_or_default()),
        SettingKind::String | SettingKind::Path => Value::String(String::new()),
    })
}

/// A value for every setting: the stored one when valid, else the default.
/// Returns the problems as (setting id, message); stored keys the plugin
/// doesn't have are reported too (and ignored).
pub fn resolve(m: &Manifest, stored: &BTreeMap<String, Value>) -> (Values, Vec<(String, String)>) {
    let mut values = Values::new();
    let mut issues = Vec::new();
    for s in &m.settings {
        let v = match stored.get(&s.id) {
            Some(v) => match check_value(s, v) {
                Ok(()) => v.clone(),
                Err(e) => {
                    issues.push((s.id.clone(), format!("{e}; using the default")));
                    default_value(s)
                }
            },
            None => default_value(s),
        };
        values.insert(s.id.clone(), v);
    }
    for k in stored.keys().filter(|k| m.setting(k).is_none()) {
        issues.push((k.clone(), format!("{} has no setting {k:?}; ignored", m.name)));
    }
    (values, issues)
}

/// A value as text for templates and conditions: strings as they are,
/// bools `true`/`false`, numbers in decimal, lists joined with spaces.
pub fn plain_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a.iter().map(plain_string).collect::<Vec<_>>().join(" "),
        Value::Null => String::new(),
        Value::Object(_) => v.to_string(),
    }
}

/// Parse text typed on a command line (`goattyd plugin set eza icons always`)
/// into a value for `s`.
pub fn parse_input(s: &Setting, text: &str) -> Result<Value, String> {
    let v = match s.kind {
        SettingKind::Bool => match text.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => Value::Bool(true),
            "0" | "false" | "no" | "off" => Value::Bool(false),
            _ => return Err(format!("{text:?}: use on or off")),
        },
        SettingKind::Integer => Value::from(text.trim().parse::<i64>().map_err(|_| format!("{text:?} is not a whole number"))?),
        SettingKind::List => match serde_json::from_str::<Value>(text) {
            Ok(v @ Value::Array(_)) => v,
            _ => Value::Array(text.split(',').map(str::trim).filter(|t| !t.is_empty()).map(|t| Value::String(t.to_owned())).collect()),
        },
        SettingKind::Choice | SettingKind::String | SettingKind::Path => Value::String(text.to_owned()),
    };
    check_value(s, &v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{parse, tests::EXAMPLE};

    #[test]
    fn resolves_stored_values_leniently() {
        let m = parse(EXAMPLE, None).unwrap();
        let stored: BTreeMap<String, Value> = serde_json::from_str(r#"{"icons": "always", "git": "yes", "nope": 1}"#).unwrap();
        let (values, issues) = resolve(&m, &stored);
        assert_eq!(values["icons"], "always");
        assert_eq!(values["git"], false, "bad value: default");
        assert_eq!(values["replace_ls"], true);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].1.contains("expected a bool"));
        assert!(issues[1].1.contains("no setting"));
    }

    #[test]
    fn checks_values() {
        let int = Setting {
            id: "n".into(),
            label: "N".into(),
            help: String::new(),
            kind: SettingKind::Integer,
            default: Some(Value::from(5)),
            group: None,
            advanced: false,
            choices: vec![],
            min: Some(1),
            max: Some(10),
            step: None,
            unit: None,
            placeholder: None,
            separator: None,
            arg: None,
            arg_off: None,
        };
        assert!(check_value(&int, &Value::from(10)).is_ok());
        assert!(check_value(&int, &Value::from(11)).unwrap_err().contains("outside 1–10"));
        assert!(check_value(&int, &Value::from(1.5)).is_err());
        assert!(check_definition(&int).is_empty());
        assert_eq!(parse_input(&int, " 7 ").unwrap(), 7);

        let list = Setting { kind: SettingKind::List, default: None, min: None, max: None, ..int.clone() };
        assert_eq!(parse_input(&list, "a, b,,c").unwrap(), serde_json::json!(["a", "b", "c"]));
        assert_eq!(parse_input(&list, r#"["x y"]"#).unwrap(), serde_json::json!(["x y"]));
        assert!(check_value(&list, &serde_json::json!(["a\nb"])).unwrap_err().contains("control"));
        assert_eq!(default_value(&list), serde_json::json!([]));

        let bad = Setting { kind: SettingKind::Bool, default: None, ..int };
        let problems = check_definition(&bad);
        assert!(problems.iter().any(|p| p.0 == "default"), "{problems:?}");
        assert!(problems.iter().any(|p| p.0 == "min"), "{problems:?}");
    }
}
