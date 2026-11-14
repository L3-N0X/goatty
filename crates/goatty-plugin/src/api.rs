//! Plugin operations as JSON documents, shared by goattyd, `goatty
//! --local` and the GUI. They travel inside one protocol message pair
//! (`ClientMsg::Plugin` / `ServerMsg::Plugin`), so adding fields or
//! operations doesn't need a protocol bump.
//!
//! Installing is two steps: [`Request::Plan`] returns what would happen
//! (downloads, permissions, shell code) with a [`Plan::digest`];
//! [`Request::Install`] carries that digest back. goattyd recomputes the
//! plan and refuses when anything security-relevant changed in between, so
//! what gets installed is exactly what the user approved.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::manifest::{Category, Manifest};
use crate::permissions::{Granted, Permission};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// What's installed and what the registries offer. `refresh`: fetch
    /// the registries first (otherwise only when never fetched).
    Catalog {
        refresh: bool,
    },
    /// What installing (or updating to) `id` would do.
    Plan {
        id: String,
        version: Option<String>,
    },
    /// Install or update `id` as planned; `approved` is the plan's digest.
    Install {
        id: String,
        version: Option<String>,
        approved: String,
    },
    Uninstall {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Catalog(Catalog),
    Plan(Box<Plan>),
    Changed(Changed),
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Catalog {
    pub installed: Vec<Installed>,
    pub available: Vec<Available>,
    pub registries: Vec<RegistryStatus>,
    /// Problems: broken installs, unreadable registries, bad settings.
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Installed {
    pub id: String,
    pub version: String,
    /// Registry name, `local` for a plugin installed from a directory, or
    /// `goatty` for one that comes with goatty.
    pub source: String,
    /// Came from a registry with a valid signature.
    pub verified: bool,
    /// The full manifest (the settings window renders its settings).
    pub manifest: Manifest,
    pub enabled: bool,
    /// `None`: usable here; else why not.
    pub incompatible: Option<String>,
    /// A newer version in its registry.
    pub update: Option<String>,
    pub installed_unix: u64,
    /// Comes with goatty: can be turned off, not uninstalled.
    #[serde(default)]
    pub builtin: bool,
    /// Where each of its packages is on this host.
    #[serde(default)]
    pub packages: Vec<PackageStatus>,
}

/// One package of an installed plugin, as found on the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageStatus {
    pub id: String,
    /// The version the plugin pins.
    pub version: String,
    pub state: PackageState,
    /// goatty has a build of it for this host (it can install it).
    pub available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PackageState {
    /// The system's copy (preferred over goatty's own).
    System {
        path: String,
    },
    /// goatty's copy; an older `version` than the pinned one can be updated.
    Managed {
        version: String,
    },
    Missing,
}

impl PackageStatus {
    /// Installing the plugin would download something.
    pub fn needs_install(&self) -> bool {
        match &self.state {
            PackageState::Missing => true,
            PackageState::Managed { version } => *version != self.version,
            PackageState::System { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Available {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub publisher: String,
    pub category: Category,
    pub keywords: Vec<String>,
    pub homepage: Option<String>,
    pub registry: String,
    pub version: String,
    pub permissions: Vec<Permission>,
    /// `None`: can be installed here; else why not.
    pub incompatible: Option<String>,
    /// The installed version, if any.
    pub installed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryStatus {
    pub name: String,
    pub url: String,
    /// Built into goatty (can't be removed).
    pub builtin: bool,
    pub sequence: Option<u64>,
    pub fetched_unix: Option<u64>,
    pub plugins: usize,
    pub error: Option<String>,
    /// The public keys (hex) trusted to sign its index. Empty from a
    /// goattyd that predates this field.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
}

/// What installing a plugin will do, for the user to approve.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub description: String,
    pub publisher: String,
    pub homepage: Option<String>,
    pub license: Option<String>,
    /// Registry name, or the directory for a local plugin.
    pub source: String,
    /// From a signed registry (`false`: a local directory, unreviewed).
    pub verified: bool,
    /// Comes with goatty: the plan only concerns its packages.
    #[serde(default)]
    pub builtin: bool,
    /// The installed version this replaces (an update).
    pub replaces: Option<String>,
    pub downloads: Vec<Download>,
    pub permissions: Vec<Granted>,
    /// `ls → eza --icons=auto` with the current settings.
    pub aliases: Vec<String>,
    pub env: Vec<String>,
    pub files: Vec<String>,
    /// The plugin's shell code, to read before approving: (file, contents).
    pub shell_code: Option<(String, String)>,
    /// Commands the plugin needs that goatty won't install.
    pub requires: Vec<String>,
    pub warnings: Vec<String>,
    /// Over everything security-relevant; [`Request::Install`] sends it back.
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    pub package: String,
    pub version: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    pub bins: Vec<String>,
    /// A zsh script in the package that the shell sources.
    #[serde(default)]
    pub script: Option<String>,
    /// Already installed on the system: goatty uses that copy and downloads nothing.
    pub system: Option<String>,
    /// goatty's copy of exactly this version is already installed: nothing to download.
    #[serde(default)]
    pub current: bool,
}

impl Download {
    /// Something will be downloaded.
    pub fn fetches(&self) -> bool {
        self.system.is_none() && !self.current
    }
}

impl Plan {
    /// Bytes to download.
    pub fn download_size(&self) -> u64 {
        self.downloads.iter().filter(|d| d.fetches()).map(|d| d.size).sum()
    }
}

impl Plan {
    /// The digest over what the user approves: identity, source, every
    /// download and file checksum, permissions, shell code.
    pub fn compute_digest(&self, files: &std::collections::BTreeMap<String, String>) -> String {
        #[derive(Serialize)]
        struct Approved<'a> {
            id: &'a str,
            version: &'a str,
            source: &'a str,
            verified: bool,
            files: &'a std::collections::BTreeMap<String, String>,
            downloads: Vec<(&'a str, &'a str, &'a str, bool)>,
            permissions: Vec<Permission>,
            shell_code: Option<&'a (String, String)>,
        }
        let a = Approved {
            id: &self.id,
            version: &self.version,
            source: &self.source,
            verified: self.verified,
            files,
            downloads: self.downloads.iter().map(|d| (d.package.as_str(), d.url.as_str(), d.sha256.as_str(), d.system.is_some())).collect(),
            permissions: self.permissions.iter().map(|g| g.permission).collect(),
            shell_code: self.shell_code.as_ref(),
        };
        let json = serde_json::to_vec(&a).expect("serializes");
        Sha256::digest(&json).iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Runs code as the user (programs or shell code).
    pub fn runs_code(&self) -> bool {
        self.permissions.iter().any(|g| g.permission.runs_code())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Changed {
    /// What happened, for the user.
    pub lines: Vec<String>,
    /// The shell configuration generation now active.
    pub generation: Option<u32>,
    /// The shell configuration changed: running shells can reload.
    pub reload: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_tagged_json() {
        let r = Request::Install { id: "eza".into(), version: None, approved: "ab".into() };
        let json = serde_json::to_string(&r).unwrap();
        assert_eq!(json, r#"{"op":"install","id":"eza","version":null,"approved":"ab"}"#);
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), r);
        let c = Reply::Changed(Changed { lines: vec!["ok".into()], generation: Some(3), reload: true });
        let back: Reply = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);
    }
}
