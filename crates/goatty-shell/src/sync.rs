//! Bringing this computer's shell setup to another host (#159).
//!
//! goatty's promise is the same shell on every host it connects to: the
//! prompt, aliases, plugins and their settings, and the built-in tools. This
//! module works out what that takes for one host, without doing anything:
//! [`LocalSetup::read`] collects this computer's setup (without network),
//! [`plan`] compares it with what the host reported ([`HostSetup`]) and
//! returns the steps. The GUI asks, then runs them through the host's
//! goattyd (`goatty-app/src/shell_sync.rs`).
//!
//! What a sync does on the host:
//!
//! * **Settings**: the host's `host.json` becomes this computer's, minus
//!   machine-specific values (the host keeps its `shell.program`) and plus
//!   the host's entries for plugins only installed there (they stay as they
//!   are).
//! * **Plugins**: each registry plugin installed here is installed there at
//!   the same version, or updated to it. Never downgraded: a newer version
//!   on the host stays. Only plugins from a registry the host trusts with
//!   the same key are installed without a review, because a signed registry
//!   pins every file of every published version: same registry, same
//!   version, same files the user already approved here. Anything else is
//!   listed as skipped with the reason. Plugins installed from a folder
//!   can't be copied.
//! * **Tools**: the built-in plugins' tools the host is missing and the
//!   synced settings keep on (Starship, fzf, …).
//!
//! Nothing is uninstalled: plugins only the host has stay.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use goatty_config::{HostSettings, profile, store};
use goatty_plugin::api::{self, Catalog};
use serde_json::{Map, Value};

use crate::history::{self, SettingChange};
use crate::plugins::{lock, newer};
use crate::service::HostService;
use crate::setup::{self, Tool};

/// The source of plugins installed from a directory.
const LOCAL_SOURCE: &str = "local";

/// This computer's shell setup: what a sync copies.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalSetup {
    /// The host settings without machine-specific values
    /// ([`profile::shareable_host`]).
    pub settings: Value,
    /// Installed registry and local plugins (not the built-in ones).
    pub plugins: Vec<LocalPlugin>,
    /// The registries plugins come from here, with their keys.
    pub registries: Vec<Registry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Registry name, or `local` for a plugin installed from a directory.
    pub source: String,
    /// Plugins it needs installed first (`requires.plugins`).
    pub requires: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub name: String,
    /// Public keys (hex) trusted to sign its index.
    pub keys: Vec<String>,
    /// goatty's own (the official registry).
    pub builtin: bool,
}

impl LocalSetup {
    /// Read this computer's setup from its files: the stored `host.json`
    /// (not environment overrides, which are for one run), the plugin lock
    /// and the registry list. Never touches the network.
    pub fn read(svc: &HostService) -> Result<Self> {
        let host = svc.settings();
        let settings = profile::shareable_host(&host.file.value);
        let (plugins, registries) = svc.plugins(|p| {
            let lock = lock::Lock::load(p.dirs)?;
            let plugins = lock
                .plugins
                .iter()
                .filter(|(id, _)| crate::builtins::get(id).is_none())
                .map(|(id, e)| {
                    let manifest = lock::manifest_of(p.dirs, id, e).ok();
                    LocalPlugin {
                        id: id.clone(),
                        name: manifest.as_ref().map_or_else(|| id.clone(), |m| m.name.clone()),
                        version: e.version.clone(),
                        source: e.source.clone(),
                        requires: manifest.map(|m| m.requires.plugins).unwrap_or_default(),
                    }
                })
                .collect();
            let registries = p.sources().0.into_iter().map(|s| Registry { name: s.name, keys: s.keys, builtin: s.builtin }).collect();
            Ok((plugins, registries))
        })?;
        Ok(Self { settings, plugins, registries })
    }

    /// Changes whenever something a sync would copy changes here: a
    /// setting, a plugin installed, removed or updated.
    pub fn fingerprint(&self) -> String {
        let plugins: Vec<Value> = self
            .plugins
            .iter()
            .map(|p| Value::from(vec![Value::from(p.id.as_str()), Value::from(p.version.as_str()), Value::from(p.source.as_str())]))
            .collect();
        let doc = Value::from(vec![self.settings.clone(), Value::from(plugins)]);
        goatty_plugin::bundle::sha256_hex(doc.to_string().as_bytes())
    }
}

/// What the host reported about its setup.
#[derive(Debug, Clone, Default)]
pub struct HostSetup {
    /// Its stored `host.json`.
    pub settings_json: String,
    /// Its plugins and registries.
    pub catalog: Catalog,
    /// Its built-in tools (`setup::Status::tools`); empty when its goattyd
    /// can't report them.
    pub tools: Vec<Tool>,
}

/// One plugin to install or update on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginStep {
    pub id: String,
    pub name: String,
    pub version: String,
    /// The registry it must come from.
    pub registry: String,
    /// The older version on the host this replaces.
    pub replaces: Option<String>,
}

/// A plugin installed here that the sync leaves out, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub id: String,
    pub reason: String,
}

/// What syncing a host takes.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    /// The settings document to apply on the host, as of the comparison
    /// (the settings step merges the host's values in again when it runs).
    pub settings_json: String,
    /// This computer's settings it was made from.
    pub local_settings: Value,
    /// How it differs from the host's settings now (empty: same settings).
    pub changes: Vec<SettingChange>,
    pub plugins: Vec<PluginStep>,
    /// Built-in tools to install (ids in `Tool::id`).
    pub tools: Vec<Tool>,
    pub skipped: Vec<Skipped>,
    /// Plugins only the host has: kept, with their settings.
    pub kept: Vec<String>,
}

impl Plan {
    /// The host has this computer's setup already (as far as goatty can
    /// copy it).
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty() && self.plugins.is_empty() && self.tools.is_empty()
    }

    /// What it does, as short phrases: `12 settings (prompt, aliases, eza)`,
    /// `2 plugins (eza, bat)`, `Starship`.
    pub fn parts(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.changes.is_empty() {
            let mut areas: Vec<&str> = Vec::new();
            for c in &self.changes {
                if !areas.contains(&c.area()) {
                    areas.push(c.area());
                }
            }
            let more = areas.len().saturating_sub(3);
            areas.truncate(3);
            let tail = if more > 0 { format!(" +{more}") } else { String::new() };
            let n = self.changes.len();
            out.push(format!("{n} setting{} ({}{tail})", if n == 1 { "" } else { "s" }, areas.join(", ")));
        }
        if !self.plugins.is_empty() {
            let names: Vec<&str> = self.plugins.iter().map(|p| p.name.as_str()).collect();
            let n = names.len();
            out.push(format!("{n} plugin{} ({})", if n == 1 { "" } else { "s" }, names.join(", ")));
        }
        if !self.tools.is_empty() {
            out.push(self.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", "));
        }
        out
    }

    /// Every step and skip, one line each (the details view, the log).
    pub fn lines(&self) -> Vec<String> {
        let mut out: Vec<String> = self.changes.iter().map(SettingChange::text).collect();
        for p in &self.plugins {
            out.push(match &p.replaces {
                Some(old) => format!("update {} {old} → {}", p.name, p.version),
                None => format!("install {} {}", p.name, p.version),
            });
        }
        out.extend(self.tools.iter().map(|t| format!("install {} {}", t.name, t.version)));
        out.extend(self.skipped.iter().map(|s| format!("skip {}: {}", s.id, s.reason)));
        out
    }
}

/// Compare this computer's setup with the host's.
pub fn plan(local: &LocalSetup, host: &HostSetup) -> Plan {
    let local_ids: BTreeSet<&str> = local.plugins.iter().map(|p| p.id.as_str()).collect();
    let remote_installed: BTreeMap<&str, &api::Installed> =
        host.catalog.installed.iter().filter(|i| !i.builtin).map(|i| (i.id.as_str(), i)).collect();
    let kept: Vec<String> = remote_installed.keys().filter(|id| !local_ids.contains(*id)).map(|id| (*id).to_owned()).collect();

    let settings_json = settings_document(&local.settings, &host.settings_json, &kept);
    let changes = history::settings_changes(&host.settings_json, &settings_json);

    let mut plugins = Vec::new();
    let mut skipped = Vec::new();
    for lp in &local.plugins {
        let skip = |reason: String| Skipped { id: lp.id.clone(), reason };
        if lp.source == LOCAL_SOURCE {
            skipped.push(skip("installed from a folder on this computer; install it there the same way".into()));
            continue;
        }
        let replaces = match remote_installed.get(lp.id.as_str()) {
            Some(r) if r.version == lp.version => continue,
            Some(r) if newer(&r.version, &lp.version) => {
                skipped.push(skip(format!("{} there is newer than {} here; goatty doesn't downgrade", r.version, lp.version)));
                continue;
            }
            Some(r) => Some(r.version.clone()),
            None => None,
        };
        if let Err(reason) = registry_trusted(local, host, &lp.source) {
            skipped.push(skip(reason));
            continue;
        }
        // The catalog only describes the newest version; another one is
        // checked by the host's install plan.
        if let Some(why) = host
            .catalog
            .available
            .iter()
            .find(|a| a.id == lp.id && a.registry == lp.source && a.version == lp.version)
            .and_then(|a| a.incompatible.as_ref())
        {
            skipped.push(skip(format!("not for that host ({why})")));
            continue;
        }
        plugins.push(PluginStep {
            id: lp.id.clone(),
            name: lp.name.clone(),
            version: lp.version.clone(),
            registry: lp.source.clone(),
            replaces,
        });
    }

    let requires: BTreeMap<&str, &[String]> = local.plugins.iter().map(|p| (p.id.as_str(), p.requires.as_slice())).collect();
    let plugins = dependencies_first(plugins, &requires);

    let synced = store::parse::<HostSettings>(&settings_json).value;
    let tools = host
        .tools
        .iter()
        .filter(|t| t.default_install && t.installable() && synced.plugins.get(&t.id).is_none_or(|c| c.enabled))
        .cloned()
        .collect();

    Plan { settings_json, local_settings: local.settings.clone(), changes, plugins, tools, skipped, kept }
}

/// Order `steps` so a plugin comes after the ones it requires (a host
/// refuses to plan it before); otherwise as given. A cycle keeps the rest
/// in their order.
fn dependencies_first(mut steps: Vec<PluginStep>, requires: &BTreeMap<&str, &[String]>) -> Vec<PluginStep> {
    let mut out: Vec<PluginStep> = Vec::with_capacity(steps.len());
    while !steps.is_empty() {
        let pending: BTreeSet<String> = steps.iter().map(|s| s.id.clone()).collect();
        let ready = steps
            .iter()
            .position(|s| requires.get(s.id.as_str()).is_none_or(|deps| deps.iter().all(|d| d == &s.id || !pending.contains(d))));
        out.push(steps.remove(ready.unwrap_or(0)));
    }
    out
}

/// The document a settings step applies: this computer's settings merged
/// with the host's current values (read just before, so a change made
/// there meanwhile isn't overwritten).
fn settings_document(local: &Value, remote_json: &str, kept: &[String]) -> String {
    let remote: Value = serde_json::from_str(remote_json).unwrap_or_else(|_| Value::Object(Map::new()));
    let target = merge_settings(local, &remote, kept);
    store::to_json(&store::parse::<HostSettings>(&target.to_string()).value, &[])
}

/// The settings the host gets: this computer's, with the host's shell
/// program and the host's entries for the plugins only it has.
fn merge_settings(local: &Value, remote: &Value, kept: &[String]) -> Value {
    let mut target = local.clone();
    let Some(obj) = target.as_object_mut() else { return target };
    if let Some(program) = remote.pointer("/shell/program") {
        let shell = obj.entry("shell").or_insert_with(|| Value::Object(Map::new()));
        if let Some(shell) = shell.as_object_mut() {
            shell.insert("program".into(), program.clone());
        }
    }
    for id in kept {
        let Some(entry) = remote.get("plugins").and_then(|p| p.get(id)) else { continue };
        let plugins = obj.entry("plugins").or_insert_with(|| Value::Object(Map::new()));
        if let Some(plugins) = plugins.as_object_mut() {
            plugins.insert(id.clone(), entry.clone());
        }
    }
    target
}

/// The host trusts registry `name` the way this computer does: both use
/// goatty's built-in one, or the host's copy is signed by a key trusted
/// here. A goattyd from before registries reported their keys can only be
/// matched on the built-in registry.
fn registry_trusted(local: &LocalSetup, host: &HostSetup, name: &str) -> Result<(), String> {
    let Some(theirs) = host.catalog.registries.iter().find(|r| r.name == name) else {
        return Err(format!("that host doesn't use the {name} registry (add it there with `goattyd plugin registry add`)"));
    };
    let Some(ours) = local.registries.iter().find(|r| r.name == name) else {
        return Err(format!("the {name} registry isn't configured on this computer any more"));
    };
    let same_key = theirs.keys.iter().any(|k| ours.keys.contains(k));
    if same_key || (theirs.builtin && ours.builtin) {
        Ok(())
    } else {
        Err(format!("the {name} registry there isn't signed by a key this computer trusts; install it there after reviewing it"))
    }
}

/// Check the host's install plan for `step` before installing it without
/// another review: it must be exactly the plugin approved here (same
/// signed registry, same version).
pub fn check_plan(step: &PluginStep, plan: &api::Plan) -> Result<()> {
    let ok = !plan.builtin && plan.verified && plan.id == step.id && plan.source == step.registry && plan.version == step.version;
    ok.then_some(()).with_context(|| {
        format!(
            "the host offers {} {} from {} ({}), not {} from {}; review it on that host",
            plan.id,
            plan.version,
            plan.source,
            if plan.verified { "signed" } else { "unsigned" },
            step.version,
            step.registry
        )
    })
}

/// A host's goattyd as a sync needs it: the GUI's link to it (local or
/// remote), or a [`HostService`] in-process.
pub trait Remote {
    fn plugin(&self, req: api::Request) -> Result<api::Reply>;
    fn shell(&self, req: setup::Request) -> Result<setup::Reply>;
    /// The stored `host.json`.
    fn settings_json(&self) -> Result<String>;
    /// Apply a settings document as a new generation; `false`: not applied.
    fn apply(&self, json: String) -> Result<bool>;
}

impl Remote for HostService {
    fn plugin(&self, req: api::Request) -> Result<api::Reply> {
        self.plugin_request(req)
    }

    fn shell(&self, req: setup::Request) -> Result<setup::Reply> {
        self.shell_request(req)
    }

    fn settings_json(&self) -> Result<String> {
        Ok(self.snapshot().json)
    }

    fn apply(&self, json: String) -> Result<bool> {
        Ok(self.set(&json, true)?.applied)
    }
}

/// Ask the host for what [`plan`] compares. Blocks (the host may fetch its
/// registries and starts a shell for the tool status).
pub fn fetch(host: &dyn Remote) -> Result<HostSetup> {
    let settings_json = host.settings_json()?;
    let catalog = match host.plugin(api::Request::Catalog { refresh: false })? {
        api::Reply::Catalog(c) => c,
        other => bail!("unexpected reply from the host: {other:?}"),
    };
    // A goattyd from before the setup API (protocol v9) can't report its tools.
    let tools = match host.shell(setup::Request::Status) {
        Ok(setup::Reply::Status(s)) => s.tools,
        Ok(_) => Vec::new(),
        Err(e) => {
            log::info!("the host can't report its shell tools: {e:#}");
            Vec::new()
        }
    };
    Ok(HostSetup { settings_json, catalog, tools })
}

/// One thing a sync does on the host, in the order [`Plan::steps`] gives:
/// plugins, tools, then the settings (which name the plugins).
#[derive(Debug, Clone)]
pub enum Step {
    Plugin(PluginStep),
    Tool(Tool),
    /// This computer's settings, and the plugins whose entries the host
    /// keeps.
    Settings {
        local: Value,
        kept: Vec<String>,
    },
}

impl Step {
    /// "Installing eza", "Applying your settings".
    pub fn label(&self) -> String {
        match self {
            Step::Plugin(p) if p.replaces.is_some() => format!("Updating {}", p.name),
            Step::Plugin(p) => format!("Installing {}", p.name),
            Step::Tool(t) => format!("Installing {}", t.name),
            Step::Settings { .. } => "Applying your settings".into(),
        }
    }

    /// Do it on `host`. Blocks (downloads, a generation build).
    pub fn run(&self, host: &dyn Remote) -> Result<()> {
        match self {
            Step::Plugin(p) => install_plugin(host, p),
            Step::Tool(t) => match host.shell(setup::Request::Install { tools: vec![t.id.clone()] })? {
                setup::Reply::Installed(r) if r.failed == 0 => Ok(()),
                setup::Reply::Installed(r) => bail!("{}", r.lines.join(" ")),
                other => bail!("unexpected reply from the host: {other:?}"),
            },
            Step::Settings { local, kept } => {
                // Read the host's settings again: the offer may have waited,
                // and installs take a while.
                let json = settings_document(local, &host.settings_json()?, kept);
                if host.apply(json)? { Ok(()) } else { bail!("the host didn't apply them") }
            }
        }
    }
}

impl Plan {
    pub fn steps(&self) -> Vec<Step> {
        let mut steps: Vec<Step> = self.plugins.iter().cloned().map(Step::Plugin).collect();
        steps.extend(self.tools.iter().cloned().map(Step::Tool));
        // Installing a plugin switches it on in the host's settings, so
        // after installs the settings are applied even when they matched.
        if !self.changes.is_empty() || !self.plugins.is_empty() {
            steps.push(Step::Settings { local: self.local_settings.clone(), kept: self.kept.clone() });
        }
        steps
    }
}

/// Install `step` on the host as approved here: plan it there, check the
/// plan is the same plugin ([`check_plan`]), install with that plan's
/// digest. A host whose registry copy predates the version gets one
/// refresh.
fn install_plugin(host: &dyn Remote, step: &PluginStep) -> Result<()> {
    let plan_it = || match host.plugin(api::Request::Plan { id: step.id.clone(), version: Some(step.version.clone()) })? {
        api::Reply::Plan(p) => Ok(*p),
        other => bail!("unexpected reply from the host: {other:?}"),
    };
    let plan = match plan_it() {
        Ok(p) => p,
        Err(first) => {
            log::info!("planning {} on the host failed ({first:#}); refreshing its registries", step.id);
            host.plugin(api::Request::Catalog { refresh: true })?;
            plan_it()?
        }
    };
    check_plan(step, &plan)?;
    match host.plugin(api::Request::Install { id: plan.id, version: Some(plan.version), approved: plan.digest })? {
        api::Reply::Changed(_) => Ok(()),
        other => bail!("unexpected reply from the host: {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use goatty_plugin::api::{Installed, RegistryStatus};

    use super::*;
    use crate::setup::ToolLocation;

    fn local(settings: &str, plugins: &[(&str, &str, &str)]) -> LocalSetup {
        LocalSetup {
            settings: serde_json::from_str(settings).unwrap(),
            plugins: plugins
                .iter()
                .map(|(id, v, src)| LocalPlugin {
                    id: (*id).into(),
                    name: id.to_uppercase(),
                    version: (*v).into(),
                    source: (*src).into(),
                    requires: Vec::new(),
                })
                .collect(),
            registries: vec![
                Registry { name: "official".into(), keys: vec!["aa".into()], builtin: true },
                Registry { name: "acme".into(), keys: vec!["bb".into()], builtin: false },
            ],
        }
    }

    fn registry(name: &str, keys: &[&str], builtin: bool) -> RegistryStatus {
        RegistryStatus {
            name: name.into(),
            url: String::new(),
            builtin,
            sequence: Some(1),
            fetched_unix: None,
            plugins: 1,
            error: None,
            keys: keys.iter().map(|k| (*k).into()).collect(),
        }
    }

    fn installed(id: &str, version: &str) -> Installed {
        let manifest = goatty_plugin::manifest::parse(
            &format!("schema = 1\nid = \"{id}\"\nname = \"{id}\"\nversion = \"{version}\"\nsummary = \"s\"\npublisher = \"p\"\n"),
            None,
        )
        .unwrap();
        Installed {
            id: id.into(),
            version: version.into(),
            source: "official".into(),
            verified: true,
            manifest,
            enabled: true,
            incompatible: None,
            update: None,
            installed_unix: 0,
            builtin: false,
            packages: Vec::new(),
        }
    }

    fn tool(id: &str, location: ToolLocation) -> Tool {
        Tool {
            id: id.into(),
            name: id.into(),
            summary: String::new(),
            enabled: true,
            default_install: true,
            location,
            download: 1,
            version: "1".into(),
            unavailable: None,
        }
    }

    fn host(settings: &str, installed: Vec<Installed>, registries: Vec<RegistryStatus>, tools: Vec<Tool>) -> HostSetup {
        HostSetup { settings_json: settings.into(), catalog: Catalog { installed, registries, ..Default::default() }, tools }
    }

    #[test]
    fn same_setup_means_nothing_to_do() {
        let l = local(r#"{"version": 4, "history": {"size": 5000}}"#, &[("eza", "1.0.0", "official")]);
        let h = host(
            r#"{"version": 4, "history": {"size": 5000}, "shell": {"program": "/opt/zsh"}}"#,
            vec![installed("eza", "1.0.0")],
            vec![registry("official", &[], true)],
            vec![tool("starship", ToolLocation::System { path: "/usr/bin/starship".into() })],
        );
        let p = plan(&l, &h);
        assert!(p.is_empty(), "{:?}", p.lines());
        // The host keeps its own shell program.
        assert!(p.settings_json.contains("/opt/zsh"), "{}", p.settings_json);
    }

    #[test]
    fn settings_plugins_and_tools_are_planned() {
        let l = local(
            r#"{"version": 4, "history": {"size": 5000}, "aliases": {"gst": {"command": "git status"}}, "plugins": {"fzf-tab": {"enabled": false}}}"#,
            &[("eza", "1.2.0", "official"), ("bat", "1.0.0", "official"), ("zoxide", "1.0.0", "official"), ("mine", "0.1.0", "local")],
        );
        let h = host(
            r#"{"version": 4, "plugins": {"extra": {"settings": {"x": 1}}}}"#,
            vec![installed("eza", "1.0.0"), installed("zoxide", "2.0.0"), installed("extra", "0.1.0")],
            vec![registry("official", &[], true)],
            vec![tool("starship", ToolLocation::Missing), tool("fzf-tab", ToolLocation::Missing)],
        );
        let p = plan(&l, &h);
        let keys: Vec<&str> = p.changes.iter().map(|c| c.key.as_str()).collect();
        assert!(keys.contains(&"history.size") && keys.contains(&"aliases.gst"), "{keys:?}");
        assert_eq!(
            p.plugins.iter().map(|s| (s.id.as_str(), s.replaces.as_deref())).collect::<Vec<_>>(),
            [("eza", Some("1.0.0")), ("bat", None)]
        );
        // Newer on the host: kept; from a folder: can't be copied.
        let skipped: Vec<&str> = p.skipped.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(skipped, ["zoxide", "mine"]);
        assert!(p.skipped[0].reason.contains("doesn't downgrade"));
        // fzf-tab is off in the synced settings: not installed.
        assert_eq!(p.tools.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["starship"]);
        // A plugin only the host has keeps its settings.
        assert_eq!(p.kept, ["extra"]);
        assert!(p.settings_json.contains("\"extra\""), "{}", p.settings_json);
        assert_eq!(p.parts()[1], "2 plugins (EZA, BAT)");
    }

    #[test]
    fn plugins_only_come_from_a_registry_the_host_trusts_with_the_same_key() {
        let l = local(r#"{"version": 4}"#, &[("a", "1.0.0", "acme"), ("b", "1.0.0", "acme"), ("c", "1.0.0", "other")]);
        let same = host(r#"{"version": 4}"#, vec![], vec![registry("acme", &["bb"], false)], vec![]);
        assert_eq!(plan(&l, &same).plugins.len(), 2);
        let other_key = host(r#"{"version": 4}"#, vec![], vec![registry("acme", &["cc"], false)], vec![]);
        let p = plan(&l, &other_key);
        assert!(p.plugins.is_empty());
        assert!(p.skipped[0].reason.contains("isn't signed by a key"), "{:?}", p.skipped);
        // `other` isn't on the host, and isn't configured here either.
        assert!(p.skipped[2].reason.contains("doesn't use the other registry"), "{:?}", p.skipped);
        // An older goattyd reports no keys: only the built-in registry matches.
        let old = host(r#"{"version": 4}"#, vec![], vec![registry("acme", &[], false)], vec![]);
        assert!(plan(&l, &old).plugins.is_empty());
    }

    #[test]
    fn dependencies_install_first_and_settings_follow_installs() {
        let mut l = local(r#"{"version": 4}"#, &[("addon", "1.0.0", "official"), ("base", "1.0.0", "official")]);
        l.plugins[0].requires = vec!["base".into()];
        let h = host(r#"{"version": 4}"#, vec![], vec![registry("official", &[], true)], vec![]);
        let p = plan(&l, &h);
        assert!(p.changes.is_empty());
        assert_eq!(p.plugins.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["base", "addon"]);
        // Installing enables a plugin there: the settings are applied after.
        assert!(matches!(p.steps().last(), Some(Step::Settings { .. })));
        // A cycle doesn't hang.
        l.plugins[1].requires = vec!["addon".into()];
        assert_eq!(plan(&l, &h).plugins.len(), 2);
    }

    #[test]
    fn only_the_synced_versions_compatibility_counts() {
        let l = local(r#"{"version": 4}"#, &[("eza", "1.0.0", "official")]);
        let mut h = host(r#"{"version": 4}"#, vec![], vec![registry("official", &[], true)], vec![]);
        let newest = |version: &str| api::Available {
            id: "eza".into(),
            name: "eza".into(),
            summary: String::new(),
            publisher: String::new(),
            category: Default::default(),
            keywords: vec![],
            homepage: None,
            registry: "official".into(),
            version: version.into(),
            permissions: vec![],
            incompatible: Some("needs goatty 9".into()),
            installed: None,
        };
        h.catalog.available = vec![newest("2.0.0")];
        assert_eq!(plan(&l, &h).plugins.len(), 1, "2.0.0's needs say nothing about 1.0.0");
        h.catalog.available = vec![newest("1.0.0")];
        assert!(plan(&l, &h).plugins.is_empty());
    }

    #[test]
    fn install_plans_are_checked_against_the_step() {
        let step =
            PluginStep { id: "eza".into(), name: "eza".into(), version: "1.0.0".into(), registry: "official".into(), replaces: None };
        let mut p = api::Plan {
            id: "eza".into(),
            name: "eza".into(),
            version: "1.0.0".into(),
            summary: String::new(),
            description: String::new(),
            publisher: String::new(),
            homepage: None,
            license: None,
            source: "official".into(),
            verified: true,
            builtin: false,
            replaces: None,
            downloads: vec![],
            permissions: vec![],
            aliases: vec![],
            env: vec![],
            files: vec![],
            shell_code: None,
            requires: vec![],
            warnings: vec![],
            digest: String::new(),
        };
        assert!(check_plan(&step, &p).is_ok());
        p.version = "1.1.0".into();
        assert!(check_plan(&step, &p).is_err());
        p.version = "1.0.0".into();
        p.source = "acme".into();
        assert!(check_plan(&step, &p).is_err());
        p.source = "official".into();
        p.verified = false;
        assert!(check_plan(&step, &p).is_err());
    }

    #[test]
    fn fingerprint_follows_settings_and_plugins() {
        let a = local(r#"{"version": 4}"#, &[("eza", "1.0.0", "official")]);
        let mut b = a.clone();
        assert_eq!(a.fingerprint(), b.fingerprint());
        b.plugins[0].version = "1.1.0".into();
        assert_ne!(a.fingerprint(), b.fingerprint());
        let c = local(r#"{"version": 4, "history": {"size": 10}}"#, &[("eza", "1.0.0", "official")]);
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    /// Like a goattyd from before the setup API: no tool status.
    struct NoTools<'a>(&'a HostService);

    impl Remote for NoTools<'_> {
        fn plugin(&self, req: api::Request) -> Result<api::Reply> {
            self.0.plugin(req)
        }
        fn shell(&self, _: setup::Request) -> Result<setup::Reply> {
            bail!("unknown message")
        }
        fn settings_json(&self) -> Result<String> {
            self.0.settings_json()
        }
        fn apply(&self, json: String) -> Result<bool> {
            self.0.apply(json)
        }
    }

    fn greet(version: &str) -> String {
        format!(
            "schema = 1\nid = \"greet\"\nname = \"Greet\"\nversion = \"{version}\"\nsummary = \"s\"\npublisher = \"tests\"\n\n\
             [[setting]]\nid = \"name\"\nlabel = \"Name\"\ntype = \"string\"\ndefault = \"world\"\n\n\
             [zsh]\ninit = \"zsh/init.zsh\"\n\n[permissions]\nshell-code = \"defines greet\"\n"
        )
    }

    fn install(svc: &HostService, id: &str, version: &str) {
        let api::Reply::Plan(p) = svc.plugin(api::Request::Plan { id: id.into(), version: Some(version.into()) }).unwrap() else {
            panic!("plan")
        };
        svc.plugin(api::Request::Install { id: id.into(), version: Some(version.into()), approved: p.digest }).unwrap();
    }

    #[test]
    fn syncs_one_host_to_another_end_to_end() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = tmp.path().join("registry");
        let (g1, g2) = (greet("0.1.0"), greet("0.2.0"));
        let init: &[(&str, &str)] = &[("zsh/init.zsh", "greet() { print hi }\n")];
        let (public, _) =
            crate::plugins::registry::tests::make_registry(&reg, &[("greet", "0.1.0", &g1, init), ("greet", "0.2.0", &g2, init)]);
        let source = crate::plugins::registry::Source {
            name: "test".into(),
            url: reg.to_string_lossy().into_owned(),
            keys: vec![public],
            builtin: true,
        };
        let host = |name: &str| {
            let root = tmp.path().join(name);
            HostService::new(crate::Launcher::new(Some(crate::Dirs::under(&root)), None), Some(root.join("config/host.json")))
                .with_plugin_sources(std::sync::Arc::new(goatty_plugin::fetch::Fixed(BTreeMap::new())), vec![source.clone()])
        };
        let (here, there) = (host("here"), host("there"));

        install(&here, "greet", "0.2.0");
        here.edit(|h| {
            h.history.size = 4242;
            h.plugins.entry("greet".into()).or_default().settings.insert("name".into(), Value::from("ada"));
        })
        .unwrap();
        install(&there, "greet", "0.1.0");
        there.edit(|h| h.shell.program = Some("/bin/zsh".into())).unwrap();

        let local = LocalSetup::read(&here).unwrap();
        let p = plan(&local, &fetch(&NoTools(&there)).unwrap());
        assert_eq!(p.plugins.len(), 1, "{:?}", p.lines());
        assert_eq!(p.plugins[0].replaces.as_deref(), Some("0.1.0"));
        assert!(p.changes.iter().any(|c| c.key == "history.size"), "{:?}", p.lines());
        // Changed on the host while the offer waited: the sync keeps it.
        there.edit(|h| h.shell.program = Some("/usr/local/bin/zsh".into())).unwrap();
        for step in p.steps() {
            step.run(&NoTools(&there)).unwrap_or_else(|e| panic!("{}: {e:#}", step.label()));
        }

        let after = there.settings().file.value;
        assert_eq!(after.history.size, 4242);
        assert_eq!(after.shell.program.as_deref(), Some("/usr/local/bin/zsh"), "the host keeps its shell");
        assert_eq!(after.plugins["greet"].settings["name"], Value::from("ada"));
        let api::Reply::Catalog(cat) = there.plugin(api::Request::Catalog { refresh: false }).unwrap() else { panic!("catalog") };
        assert_eq!(cat.installed.iter().find(|i| i.id == "greet").unwrap().version, "0.2.0");
        // Synced: nothing left to do.
        let again = plan(&local, &fetch(&NoTools(&there)).unwrap());
        assert!(again.is_empty(), "{:?}", again.lines());
    }

    #[test]
    fn reads_this_computers_setup_without_private_values() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::Dirs::under(tmp.path());
        let file = tmp.path().join("config/host.json");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"version": 4, "shell": {"program": "/home/me/zsh"}, "history": {"size": 300}}"#).unwrap();
        let svc = HostService::new(crate::Launcher::new(Some(dirs), None), Some(file));
        let l = LocalSetup::read(&svc).unwrap();
        assert!(l.plugins.is_empty());
        assert_eq!(l.settings.pointer("/history/size"), Some(&Value::from(300)));
        assert_eq!(l.settings.pointer("/shell/program"), None);
        assert!(l.registries.iter().any(|r| r.builtin));
    }
}
