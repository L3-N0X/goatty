//! goatty plugins on this host (Stage 5): registries, install plans,
//! installs, and the plugins that go into the shell configuration.
//!
//! The format and the trust machinery are `goatty-plugin`; this module
//! knows where things live ([`Dirs`]) and how they reach a shell:
//!
//! 1. [`Plugins::find`] a plugin in a verified registry snapshot (or a
//!    local directory, [`Candidate::local`]),
//! 2. [`Plugins::plan`] what installing it does (downloads, permissions,
//!    shell code) and hand that to the user,
//! 3. [`Plugins::install`] exactly that plan (its digest must match),
//!    packages into `tools/` + `bin/`, a verified copy of the plugin's files
//!    into `installed/<id>/`, an entry in `installed/lock.json`,
//! 4. the caller enables it in `host.json` and builds a new generation;
//!    [`active`] is what the generation renders.
//!
//! Nothing outside goatty's own directories is touched, and nothing runs
//! as root.

pub mod cmd;
pub mod lock;
pub mod registry;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use goatty_config::HostSettings;
use goatty_plugin::api::{Available, Catalog, Download, Installed, Plan, RegistryStatus};
use goatty_plugin::fetch::Fetch;
use goatty_plugin::manifest::{self, Manifest};
use goatty_plugin::schema::{self, Values};
use goatty_plugin::{GOATTY_VERSION, current_target, permissions, template};

use crate::dirs::Dirs;
use crate::install::{self, PackageInstall};
use crate::tools::{self, Kind};
use lock::Lock;
use registry::{Snapshot, Source};

/// Registries are fetched again by [`Plugins::catalog`] when older than this.
pub const STALE_AFTER_SECS: u64 = 24 * 3600;

/// A plugin that can be planned and installed.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub manifest: Manifest,
    /// Every file, relative path → sha256 (from the signed index, or hashed
    /// from the local directory).
    pub files: BTreeMap<String, String>,
    /// Where the files are.
    pub dir: PathBuf,
    /// Registry name, or the local directory.
    pub source: String,
    pub verified: bool,
}

impl Candidate {
    /// A plugin directory on this host (development, private plugins).
    /// Not verified: nobody reviewed or signed it.
    pub fn local(dir: &Path) -> Result<Candidate> {
        let dir = dir.canonicalize().with_context(|| format!("{} doesn't exist", dir.display()))?;
        let files = goatty_plugin::bundle::list_files(&dir)?;
        let manifest = manifest::load_dir(&dir)?;
        Ok(Candidate { manifest, files, source: dir.display().to_string(), dir, verified: false })
    }

    fn is_local(&self) -> bool {
        !self.verified && Path::new(&self.source).is_absolute()
    }
}

/// Plugin operations on one host.
pub struct Plugins<'a> {
    pub dirs: &'a Dirs,
    pub fetch: &'a dyn Fetch,
    /// Built-in registries (normally just the official one).
    pub builtin: &'a [Source],
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Serializes installs and uninstalls across goattyd, `goatty --local` and the CLI.
fn op_lock(dirs: &Dirs) -> Result<std::fs::File> {
    std::fs::create_dir_all(dirs.installed_dir())?;
    let f = std::fs::File::create(dirs.installed_dir().join(".lock"))?;
    f.lock()?;
    Ok(f)
}

impl Plugins<'_> {
    pub fn sources(&self) -> (Vec<Source>, Vec<String>) {
        registry::sources(self.dirs, self.builtin)
    }

    /// Every registry's snapshot: fetched when `refresh`, when never
    /// fetched, or (with `refresh_stale`) when older than a day.
    pub fn snapshots(&self, refresh: bool, refresh_stale: bool) -> (Vec<Snapshot>, Vec<RegistryStatus>, Vec<String>) {
        let (sources, issues) = self.sources();
        let mut snaps = Vec::new();
        let mut status = Vec::new();
        for s in sources {
            let stored = registry::load(self.dirs, &s);
            let mut error = None;
            let mut snap = match stored {
                Ok(snap) => snap,
                Err(e) => {
                    error = Some(format!("{e:#}"));
                    None
                }
            };
            let stale = snap.as_ref().is_none_or(|x| refresh_stale && now().saturating_sub(x.fetched) > STALE_AFTER_SECS);
            if refresh || stale {
                match registry::refresh(self.dirs, &s, self.fetch) {
                    Ok(fresh) => {
                        snap = Some(fresh);
                        error = None;
                    }
                    Err(e) => error = Some(format!("{e:#}")),
                }
            }
            status.push(RegistryStatus {
                name: s.name.clone(),
                url: s.url.clone(),
                builtin: s.builtin,
                sequence: snap.as_ref().map(|x| x.index.sequence),
                fetched_unix: snap.as_ref().map(|x| x.fetched),
                plugins: snap.as_ref().map_or(0, |x| x.index.plugins.len()),
                error,
            });
            snaps.extend(snap);
        }
        (snaps, status, issues)
    }

    /// `id` (at `version`, default the newest) from the first registry
    /// that has it.
    pub fn find(&self, id: &str, version: Option<&str>) -> Result<Candidate> {
        let (snaps, status, _) = self.snapshots(false, false);
        for snap in &snaps {
            let Some(entry) = snap.index.plugins.get(id) else { continue };
            let (v, ver) = match version {
                Some(v) => (v, entry.versions.get(v).with_context(|| format!("{id} has no version {v} in registry {}", snap.source.name))?),
                None => snap.index.latest(id).with_context(|| format!("every version of {id} is yanked"))?,
            };
            let dir = snap.plugin_dir(id, v);
            let text = std::fs::read_to_string(dir.join(manifest::FILE))?;
            let files: std::collections::BTreeSet<String> = ver.files.keys().cloned().collect();
            let manifest = manifest::parse(&text, Some(&files)).map_err(|e| anyhow!("{id}@{v}: {e}"))?;
            return Ok(Candidate { manifest, files: ver.files.clone(), dir, source: snap.source.name.clone(), verified: true });
        }
        let errors: Vec<String> = status.iter().filter_map(|s| s.error.as_ref().map(|e| format!("{}: {e}", s.name))).collect();
        if errors.is_empty() {
            bail!("no registry has a plugin called {id:?} (try `goattyd plugin search`)")
        } else {
            bail!("no registry has a plugin called {id:?}; some couldn't be read:\n  {}", errors.join("\n  "))
        }
    }

    /// What installing `c` would do.
    pub fn plan(&self, c: &Candidate, settings: &HostSettings) -> Result<Plan> {
        let m = &c.manifest;
        let target = current_target();
        m.compatible(GOATTY_VERSION, &target).map_err(|why| anyhow!("{} can't be installed here: {why}", m.name))?;
        let lock = Lock::load(self.dirs)?;
        for dep in &m.requires.plugins {
            if !lock.plugins.contains_key(dep) {
                bail!("{} needs the {dep} plugin: install that first", m.name);
            }
        }
        let owners = lock.executables();
        for exe in m.executables() {
            if let Some(t) = tools::TOOLS.iter().find(|t| matches!(t.kind, Kind::Binary { exe: e } if e == exe)) {
                bail!("{exe} is goatty's built-in {} tool; a plugin can't replace it", t.name);
            }
            if let Some(owner) = owners.get(exe).filter(|o| **o != m.id) {
                bail!("{exe} is already provided by the {owner} plugin");
            }
            if install::link_owner(self.dirs, exe).as_deref() == Some("?") {
                bail!("{} already exists and isn't goatty's; not replacing it", self.dirs.bin_dir().join(exe).display());
            }
        }

        let path = std::env::var("PATH").ok();
        let managed_bin = self.dirs.bin_dir();
        let on_path = |exe: &str| tools::which(exe, path.as_deref(), &managed_bin);
        let downloads = m
            .packages
            .iter()
            .map(|p| {
                let a = p.asset_for(&target).expect("checked by compatible()");
                let names: Vec<String> = p.bin.iter().map(|b| manifest::bin_name(b).to_owned()).collect();
                let system = if p.prefer_system {
                    let found: Vec<PathBuf> = names.iter().filter_map(|n| on_path(n)).collect();
                    (found.len() == names.len()).then(|| found[0].display().to_string())
                } else {
                    None
                };
                Download {
                    package: p.id.clone(),
                    version: p.version.clone(),
                    url: a.url.clone(),
                    size: a.size,
                    sha256: a.sha256.clone(),
                    bins: names,
                    system,
                }
            })
            .collect();

        let stored = settings.plugins.get(&m.id).map(|c| c.settings.clone()).unwrap_or_default();
        let (values, _) = schema::resolve(m, &stored);
        let aliases = m
            .aliases
            .iter()
            .map(|a| {
                let mut s = format!("{} → {}", a.name, goatty_plugin::alias::expansion(m, a, &values));
                if let Some(w) = &a.when {
                    let on = manifest::applies(&a.when, &values);
                    let when = goatty_plugin::alias::describe(m, w);
                    s.push_str(&format!("  (while {when}: {})", if on { "on now" } else { "off now" }));
                }
                s
            })
            .collect();
        let env = m
            .env
            .iter()
            .map(|e| {
                let value = template::expand_inline(&e.value, m, &values, None);
                let mut s = if value.is_empty() { format!("{} (not set while empty)", e.name) } else { format!("{}={value}", e.name) };
                if let Some(w) = &e.when {
                    s.push_str(&format!("  (when {w}: {})", if manifest::applies(&e.when, &values) { "on now" } else { "off now" }));
                }
                s
            })
            .chain(m.files.iter().filter_map(|f| f.env.as_ref().map(|v| format!("{v}=<goatty's generated {} file>", f.name))))
            .collect();
        let files = m.files.iter().map(|f| f.name.clone()).collect();
        let shell_code = match &m.zsh.init {
            Some(init) => {
                let bytes = std::fs::read(c.dir.join(init)).with_context(|| format!("read {init}"))?;
                if c.files.get(init).is_none_or(|sha| goatty_plugin::bundle::sha256_hex(&bytes) != *sha) {
                    bail!("{init} doesn't match its checksum");
                }
                Some((init.clone(), String::from_utf8_lossy(&bytes).into_owned()))
            }
            None => None,
        };
        let requires = m
            .requires
            .commands
            .iter()
            .map(|cmd| match on_path(cmd) {
                Some(p) => format!("{cmd}: found at {}", p.display()),
                None => format!("{cmd}: not installed here (goatty doesn't install it); the integration stays off until it is"),
            })
            .collect();

        let mut warnings = Vec::new();
        if c.is_local() {
            warnings.push(
                "Not from a registry: nobody reviewed this plugin and nothing signed it. Install it only if you wrote it or trust whoever did."
                    .into(),
            );
        }
        for a in &m.aliases {
            if on_path(&a.name).is_some() {
                warnings.push(format!(
                    "`{}` is an existing command; in goatty shells the alias replaces it (turn it off in the plugin's settings, or `unalias {}` in ~/.config/goatty/zshrc).",
                    a.name, a.name
                ));
            }
        }
        let other_aliases: BTreeMap<String, String> = lock
            .plugins
            .iter()
            .filter(|(id, _)| **id != m.id)
            .filter_map(|(id, e)| lock::manifest_of(self.dirs, id, e).ok().map(|om| (id.clone(), om)))
            .flat_map(|(id, om)| om.aliases.into_iter().map(move |a| (a.name, id.clone())))
            .collect();
        for a in &m.aliases {
            if let Some(other) = other_aliases.get(&a.name) {
                warnings.push(format!("The {other} plugin also defines `{}`; the one loaded last wins.", a.name));
            }
        }
        let replaces = lock.plugins.get(&m.id).map(|e| e.version.clone());
        let mut plan = Plan {
            id: m.id.clone(),
            name: m.name.clone(),
            version: m.version.clone(),
            summary: m.summary.clone(),
            description: m.description.clone(),
            publisher: m.publisher.clone(),
            homepage: m.homepage.clone(),
            license: m.license.clone(),
            source: c.source.clone(),
            verified: c.verified,
            replaces,
            downloads,
            permissions: permissions::describe(m),
            aliases,
            env,
            files,
            shell_code,
            requires,
            warnings,
            digest: String::new(),
        };
        plan.digest = plan.compute_digest(&c.files);
        Ok(plan)
    }

    /// Install `c` if its plan still has the digest the user approved.
    /// Returns the plan that was carried out. The caller enables the plugin
    /// in the host settings and builds the shell configuration.
    pub fn install(&self, c: &Candidate, approved: &str, settings: &HostSettings) -> Result<Plan> {
        let _guard = op_lock(self.dirs)?;
        let plan = self.plan(c, settings)?;
        if plan.digest != approved {
            bail!("{} changed since you reviewed it (files, downloads or permissions); review the new plan before installing", plan.name);
        }
        let m = &c.manifest;
        let target = current_target();
        let mut lock = Lock::load(self.dirs)?;
        let previous = lock.plugins.get(&m.id).cloned();

        let mut packages = Vec::new();
        let mut fresh: Vec<(String, Vec<String>)> = Vec::new();
        let result = (|| -> Result<()> {
            for (p, d) in m.packages.iter().zip(&plan.downloads) {
                if let Some(sys) = &d.system {
                    packages.push(lock::Package {
                        id: p.id.clone(),
                        version: p.version.clone(),
                        bins: p.bin.clone(),
                        system: Some(sys.clone()),
                    });
                    continue;
                }
                let a = p.asset_for(&target).context("no build for this host")?;
                let bytes = self.fetch.fetch(&a.url).with_context(|| format!("download {}", a.url))?;
                let pi =
                    PackageInstall { id: &p.id, version: &p.version, sha256: &a.sha256, format: a.format, root: &a.root, bins: &p.bin };
                install::install_package(&pi, &bytes, self.dirs).with_context(|| format!("install {} {}", p.id, p.version))?;
                fresh.push((p.id.clone(), p.bin.clone()));
                packages.push(lock::Package { id: p.id.clone(), version: p.version.clone(), bins: p.bin.clone(), system: None });
            }

            // The plugin's files: staged, checked, then swapped in.
            let dest = lock::bundle_dir(self.dirs, &m.id);
            let staging = self.dirs.installed_dir().join(format!(".{}.partial-{}", m.id, std::process::id()));
            let _ = std::fs::remove_dir_all(&staging);
            if let Err(e) = goatty_plugin::bundle::copy_verified(&c.dir, &staging, &c.files) {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(e);
            }
            if dest.exists() {
                std::fs::remove_dir_all(&dest)?;
            }
            std::fs::rename(&staging, &dest)?;
            Ok(())
        })();
        if let Err(e) = result {
            // Undo packages this attempt added that the old version didn't have.
            let keep: Vec<&str> = previous.iter().flat_map(|p| p.packages.iter().map(|x| x.id.as_str())).collect();
            for (id, bins) in fresh.iter().filter(|(id, _)| !keep.contains(&id.as_str())) {
                let _ = install::uninstall_package(id, bins, self.dirs);
            }
            return Err(e);
        }
        // Packages the old version had and the new one doesn't.
        if let Some(old) = &previous {
            for p in old.packages.iter().filter(|p| p.system.is_none() && !packages.iter().any(|n| n.id == p.id)) {
                let _ = install::uninstall_package(&p.id, &p.bins, self.dirs);
            }
        }
        lock.plugins.insert(
            m.id.clone(),
            lock::Entry {
                version: m.version.clone(),
                source: if c.is_local() { "local".into() } else { c.source.clone() },
                path: c.is_local().then(|| c.source.clone()),
                verified: c.verified,
                files: c.files.clone(),
                packages,
                permissions: permissions::all(m),
                digest: plan.digest.clone(),
                installed: now(),
            },
        );
        lock.save(self.dirs)?;
        log::info!("installed plugin {} {} from {}", m.id, m.version, c.source);
        Ok(plan)
    }

    /// Remove an installed plugin: its packages, files and lock entry.
    /// The caller drops it from the host settings.
    pub fn uninstall(&self, id: &str) -> Result<lock::Entry> {
        let _guard = op_lock(self.dirs)?;
        let mut lock = Lock::load(self.dirs)?;
        let entry = lock.plugins.remove(id).with_context(|| format!("{id} isn't installed"))?;
        for (other, e) in &lock.plugins {
            if let Ok(m) = lock::manifest_of(self.dirs, other, e)
                && m.requires.plugins.iter().any(|d| d == id)
            {
                bail!("the {other} plugin needs {id}; uninstall {other} first");
            }
        }
        for p in entry.packages.iter().filter(|p| p.system.is_none()) {
            install::uninstall_package(&p.id, &p.bins, self.dirs)?;
        }
        let dir = lock::bundle_dir(self.dirs, id);
        if dir.exists() {
            std::fs::remove_dir_all(&dir).with_context(|| format!("remove {}", dir.display()))?;
        }
        lock.save(self.dirs)?;
        log::info!("uninstalled plugin {id}");
        Ok(entry)
    }

    /// Installed plugins and what the registries offer.
    pub fn catalog(&self, refresh: bool, settings: &HostSettings) -> Catalog {
        let (snaps, registries, mut issues) = self.snapshots(refresh, true);
        let lock = match Lock::load(self.dirs) {
            Ok(l) => l,
            Err(e) => {
                issues.push(format!("{e:#}"));
                Lock::default()
            }
        };
        let target = current_target();
        let mut installed = Vec::new();
        for (id, e) in &lock.plugins {
            let m = match lock::manifest_of(self.dirs, id, e) {
                Ok(m) => m,
                Err(err) => {
                    issues.push(format!("{err:#}"));
                    continue;
                }
            };
            let update = snaps
                .iter()
                .find(|s| s.source.name == e.source)
                .and_then(|s| s.index.latest(id))
                .filter(|(v, _)| newer(v, &e.version))
                .map(|(v, _)| v.to_owned());
            installed.push(Installed {
                id: id.clone(),
                version: e.version.clone(),
                source: e.source.clone(),
                verified: e.verified,
                enabled: settings.plugins.get(id).is_none_or(|c| c.enabled),
                incompatible: m.compatible(GOATTY_VERSION, &target).err(),
                manifest: m,
                update,
                installed_unix: e.installed,
            });
        }
        let mut available: Vec<Available> = Vec::new();
        for s in &snaps {
            for (id, entry) in &s.index.plugins {
                if available.iter().any(|a| a.id == *id) {
                    continue;
                }
                let Some((v, ver)) = s.index.latest(id) else { continue };
                let incompatible = if !ver.targets.is_empty() && !ver.targets.iter().any(|t| *t == target || t == "any") {
                    Some(format!("no build for {target}"))
                } else if !ver.goatty.is_empty()
                    && semver::VersionReq::parse(&ver.goatty)
                        .is_ok_and(|r| !r.matches(&semver::Version::parse(GOATTY_VERSION).expect("semver")))
                {
                    Some(format!("needs goatty {}", ver.goatty))
                } else {
                    None
                };
                available.push(Available {
                    id: id.clone(),
                    name: entry.name.clone(),
                    summary: entry.summary.clone(),
                    publisher: entry.publisher.clone(),
                    category: entry.category,
                    keywords: entry.keywords.clone(),
                    homepage: entry.homepage.clone(),
                    registry: s.source.name.clone(),
                    version: v.to_owned(),
                    permissions: ver.permissions.clone(),
                    incompatible,
                    installed: lock.plugins.get(id).map(|e| e.version.clone()),
                });
            }
        }
        available.sort_by_key(|a| a.name.to_lowercase());
        issues.extend(active(self.dirs, settings).1);
        Catalog { installed, available, registries, issues }
    }
}

/// `a` is a newer semver version than `b`.
pub fn newer(a: &str, b: &str) -> bool {
    match (semver::Version::parse(a), semver::Version::parse(b)) {
        (Ok(a), Ok(b)) => a > b,
        _ => false,
    }
}

/// An installed, enabled, usable plugin, ready to render.
#[derive(Debug, Clone)]
pub struct Active {
    pub manifest: Manifest,
    pub values: Values,
    pub entry: lock::Entry,
    /// `eza 1.0.0 (official)`.
    pub label: String,
}

/// The plugins that go into the shell configuration for `settings`, and
/// the problems found on the way (bad values, missing or changed files,
/// settings for plugins that aren't installed).
pub fn active(dirs: &Dirs, settings: &HostSettings) -> (Vec<Active>, Vec<String>) {
    let mut out = Vec::new();
    let mut issues = Vec::new();
    let lock = match Lock::load(dirs) {
        Ok(l) => l,
        Err(e) => return (out, vec![format!("{e:#}")]),
    };
    for id in settings.plugins.keys().filter(|id| !lock.plugins.contains_key(*id)) {
        issues.push(format!("plugins.{id}: not installed on this host; ignored (goattyd plugin install {id})"));
    }
    let target = current_target();
    for (id, entry) in &lock.plugins {
        let config = settings.plugins.get(id);
        if config.is_some_and(|c| !c.enabled) {
            continue;
        }
        let mut m = match lock::manifest_of(dirs, id, entry) {
            Ok(m) => m,
            Err(e) => {
                issues.push(format!("{e:#}"));
                continue;
            }
        };
        if let Err(why) = m.compatible(GOATTY_VERSION, &target) {
            issues.push(format!("plugin {id} is off: {why}"));
            continue;
        }
        // Everything the shell will source must still be what was approved.
        let needed: Vec<&String> =
            m.zsh.init.iter().chain(entry.files.keys().filter(|f| m.zsh.fpath.iter().any(|d| f.starts_with(&format!("{d}/"))))).collect();
        if let Some(e) = needed.iter().find_map(|rel| lock::read_checked(dirs, id, entry, rel).err()) {
            issues.push(format!("plugin {id} is off: {e:#}"));
            continue;
        }
        let stored = config.map(|c| c.settings.clone()).unwrap_or_default();
        let (values, problems) = schema::resolve(&m, &stored);
        for (k, msg) in problems {
            issues.push(format!("plugins.{id}.settings.{k}: {msg}"));
        }
        // Aliases the user switched off stay out of the shell (#88).
        if let Some(off) = config.map(|c| &c.disabled_aliases).filter(|d| !d.is_empty()) {
            m.aliases.retain(|a| !off.contains(&a.name));
        }
        let label = format!("{} {} ({})", m.id, m.version, entry.source);
        out.push(Active { manifest: m, values, entry: entry.clone(), label });
    }
    (out, issues)
}

#[cfg(test)]
pub(crate) mod tests;
