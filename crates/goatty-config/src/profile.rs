//! Shareable profiles: client + host settings in one JSON file.
//!
//! Exports leave out machine-specific values (the shell's path, debug
//! switches), so a profile can be shared without leaking local details.
//! Importing goes through the same lenient parser as the settings files.

use serde_json::{Map, Value};

use crate::client::ClientSettings;
use crate::host::HostSettings;
use crate::store::{self, Loaded};

pub const PROFILE_VERSION: u32 = 1;

/// Paths (`section.dotted.path`) never exported.
const PRIVATE: &[&str] = &["host.shell.program", "client.debug"];

fn section(json: &str, name: &str) -> Value {
    let mut v: Value = serde_json::from_str(json).expect("our own JSON");
    if let Value::Object(m) = &mut v {
        for p in PRIVATE {
            let Some(rest) = p.strip_prefix(name).and_then(|r| r.strip_prefix('.')) else { continue };
            let parts: Vec<&str> = rest.split('.').collect();
            remove_path(m, &parts);
        }
    }
    v
}

fn remove_path(m: &mut Map<String, Value>, parts: &[&str]) {
    match parts {
        [] => {}
        [last] => {
            m.remove(*last);
        }
        [first, rest @ ..] => {
            if let Some(Value::Object(child)) = m.get_mut(*first) {
                remove_path(child, rest);
                if child.is_empty() {
                    m.remove(*first);
                }
            }
        }
    }
}

/// A profile file with both documents (non-default values only).
pub fn export(client: &ClientSettings, host: &HostSettings) -> String {
    let mut root = Map::new();
    root.insert("goatty_profile".into(), Value::from(PROFILE_VERSION));
    root.insert("client".into(), section(&store::to_json(client, &[]), "client"));
    root.insert("host".into(), shareable_host(host));
    let mut s = serde_json::to_string_pretty(&Value::Object(root)).expect("json");
    s.push('\n');
    s
}

/// The host settings as they travel to another machine (an export's
/// `host` part, and what goatty copies to a host it connects to): non-default
/// values only, without machine-specific ones like the shell's path.
pub fn shareable_host(host: &HostSettings) -> Value {
    section(&store::to_json(host, &[]), "host")
}

/// Client and host settings from a profile, each `None` when the profile
/// doesn't contain that side (which then stays as it is).
pub type Imported = (Option<Loaded<ClientSettings>>, Option<Loaded<HostSettings>>);

/// Read a profile file.
pub fn import(text: &str) -> Result<Imported, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    let obj = v.as_object().ok_or("a profile must be a JSON object")?;
    match obj.get("goatty_profile").and_then(Value::as_u64) {
        Some(n) if n as u32 <= PROFILE_VERSION => {}
        Some(n) => return Err(format!("profile version {n} is newer than this goatty supports ({PROFILE_VERSION})")),
        None => return Err("not a goatty profile (no \"goatty_profile\" field)".into()),
    }
    let client = obj.get("client").map(|c| store::parse::<ClientSettings>(&c.to_string()));
    let host = obj.get("host").map(|h| store::parse::<HostSettings>(&h.to_string()));
    if client.is_none() && host.is_none() {
        return Err("the profile has neither client nor host settings".into());
    }
    Ok((client, host))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_without_private_values() {
        let mut c = ClientSettings::default();
        c.theme.name = "nord".into();
        c.debug.stats = true;
        let mut h = HostSettings::default();
        h.history.size = 5000;
        h.shell.program = Some("/home/me/bin/zsh".into());
        let text = export(&c, &h);
        assert!(!text.contains("/home/me"), "{text}");
        assert!(!text.contains("stats"), "{text}");
        let (ci, hi) = import(&text).unwrap();
        let (ci, hi) = (ci.unwrap(), hi.unwrap());
        assert!(ci.issues.is_empty() && hi.issues.is_empty());
        assert_eq!(ci.value.theme.name, "nord");
        assert!(!ci.value.debug.stats);
        assert_eq!(hi.value.history.size, 5000);
        assert_eq!(hi.value.shell.program, None);
    }

    #[test]
    fn rejects_non_profiles() {
        assert!(import("{}").is_err());
        assert!(import(r#"{"goatty_profile": 99, "client": {}}"#).is_err());
        assert!(import(r#"{"goatty_profile": 1}"#).is_err());
        let (c, h) = import(r#"{"goatty_profile": 1, "host": {"history": {"size": 300}}}"#).unwrap();
        assert!(c.is_none());
        assert_eq!(h.unwrap().value.history.size, 300);
    }
}
