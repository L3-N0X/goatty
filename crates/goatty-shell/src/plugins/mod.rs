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
//! The plugins that come with goatty ([`crate::builtins`]) take part in
//! all of this except the lock: they're always there, so planning and
//! installing one only concerns its packages.
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
use goatty_plugin::api::{Available, Catalog, Download, Installed, PackageState, PackageStatus, Plan, RegistryStatus};
use goatty_plugin::fetch::Fetch;
use goatty_plugin::manifest::{self, Load, Manifest};
use goatty_plugin::schema::{self, Values};
use goatty_plugin::{GOATTY_VERSION, current_target, permissions, template};

use crate::builtins::{self, Builtin};
use crate::dirs::Dirs;
use crate::install::{self, PackageInstall};
use crate::packages::{self, Location};
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
    /// Registry name, the local directory, or [`builtins::SOURCE`].
    pub source: String,
    pub verified: bool,
    /// One of the plugins that come with goatty (its files are compiled in).
    pub builtin: Option<&'static Builtin>,
}

impl Candidate {
    /// A plugin that comes with goatty.
    pub fn builtin(b: &'static Builtin) -> Candidate {
        Candidate {
            manifest: b.manifest().clone(),
            files: b.checksums(),
            dir: PathBuf::new(),
            source: builtins::SOURCE.into(),
            verified: true,
            builtin: Some(b),
        }
    }

    /// One of the plugin's files.
    fn read(&self, rel: &str) -> Result<Vec<u8>> {
        match self.builtin {
            Some(b) => b.read(rel).map(|t| t.as_bytes().to_vec()).with_context(|| format!("{} has no {rel}", b.id)),
            None => std::fs::read(self.dir.join(rel)).with_context(|| format!("read {rel}")),
        }
    }

    /// A plugin directory on this host (development, private plugins).
    /// Not verified: nobody reviewed or signed it.
    pub fn local(dir: &Path) -> Result<Candidate> {
        let dir = dir.canonicalize().with_context(|| format!("{} doesn't exist", dir.display()))?;
        let files = goatty_plugin::bundle::list_files(&dir)?;
        let manifest = manifest::load_dir(&dir)?;
        Ok(Candidate { manifest, files, source: dir.display().to_string(), dir, verified: false, builtin: None })
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

    /// `id` (at `version`, default the newest): a built-in plugin, or from
    /// the first registry that has it.
    pub fn find(&self, id: &str, version: Option<&str>) -> Result<Candidate> {
        if let Some(b) = builtins::get(id) {
            let have = &b.manifest().version;
            if version.is_some_and(|v| v != have) {
                bail!("{id} comes with goatty (version {have}); other versions come with other goatty releases");
            }
            return Ok(Candidate::builtin(b));
        }
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
            return Ok(Candidate {
                manifest,
                files: ver.files.clone(),
                dir,
                source: snap.source.name.clone(),
                verified: true,
                builtin: None,
            });
        }
        let errors: Vec<String> = status.iter().filter_map(|s| s.error.as_ref().map(|e| format!("{}: {e}", s.name))).collect();
        if errors.is_empty() {
            bail!("no registry has a plugin called {id:?} (try `goattyd plugin search`)")
        } else {
            bail!("no registry has a plugin called {id:?}; some couldn't be read:\n  {}", errors.join("\n  "))
        }
    }

    /// What installing `c` would do. For a built-in plugin: downloading the
    /// packages it's missing.
    pub fn plan(&self, c: &Candidate, settings: &HostSettings) -> Result<Plan> {
        let m = &c.manifest;
        let target = current_target();
        // A built-in plugin works with system copies anywhere; only a
        // download needs a build for this host (checked below).
        if c.builtin.is_none() {
            m.compatible(GOATTY_VERSION, &target).map_err(|why| anyhow!("{} can't be installed here: {why}", m.name))?;
        }
        let lock = Lock::load(self.dirs)?;
        for dep in &m.requires.plugins {
            if !lock.plugins.contains_key(dep) && builtins::get(dep).is_none() {
                bail!("{} needs the {dep} plugin: install that first", m.name);
            }
        }
        if c.builtin.is_none() {
            if builtins::get(&m.id).is_some() {
                bail!("{} is the id of a plugin that comes with goatty; give this one another id", m.id);
            }
            let reserved = builtins::executables();
            let reserved_pkgs = builtins::package_ids();
            for p in &m.packages {
                if let Some(b) = reserved_pkgs.get(p.id.as_str()) {
                    bail!("package {} belongs to goatty's built-in {b} plugin; give it another id", p.id);
                }
            }
            for exe in m.executables() {
                if let Some(b) = reserved.get(exe) {
                    bail!("{exe} comes with goatty's built-in {b} plugin; a plugin can't replace it");
                }
            }
        }
        let owners = lock.executables();
        for exe in m.executables() {
            if let Some(owner) = owners.get(exe).filter(|o| **o != m.id) {
                bail!("{exe} is already provided by the {owner} plugin");
            }
            if install::link_owner(self.dirs, exe).as_deref() == Some("?") {
                bail!("{} already exists and isn't goatty's; not replacing it", self.dirs.bin_dir().join(exe).display());
            }
        }

        let path = std::env::var("PATH").ok();
        let managed_bin = self.dirs.bin_dir();
        let on_path = |exe: &str| packages::which(exe, path.as_deref(), &managed_bin);
        let downloads = m
            .packages
            .iter()
            .map(|p| {
                let names: Vec<String> = p.bin.iter().map(|b| manifest::bin_name(b).to_owned()).collect();
                let system = match packages::locate(self.dirs, p, path.as_deref()) {
                    Location::System(found) => Some(found.display().to_string()),
                    _ => None,
                };
                let current = packages::managed(self.dirs, p).is_some_and(|(v, _)| v == p.version);
                let a = p.asset_for(&target);
                if a.is_none() && system.is_none() && !current {
                    bail!("{} has no goatty-managed build for {target}: install it with your package manager", p.id);
                }
                Ok(Download {
                    package: p.id.clone(),
                    version: p.version.clone(),
                    url: a.map(|a| a.url.clone()).unwrap_or_default(),
                    size: a.map_or(0, |a| a.size),
                    sha256: a.map(|a| a.sha256.clone()).unwrap_or_default(),
                    bins: names,
                    script: p.source.clone(),
                    system,
                    current,
                })
            })
            .collect::<Result<Vec<_>>>()?;

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
                let bytes = c.read(init)?;
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
        let replaces = match c.builtin {
            Some(_) => None,
            None => lock.plugins.get(&m.id).map(|e| e.version.clone()),
        };
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
            builtin: c.builtin.is_some(),
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
                if !d.current {
                    let a = p.asset_for(&target).context("no build for this host")?;
                    let bytes = self.fetch.fetch(&a.url).with_context(|| format!("download {}", a.url))?;
                    let pi = PackageInstall {
                        id: &p.id,
                        version: &p.version,
                        sha256: &a.sha256,
                        format: a.format,
                        root: &a.root,
                        bins: &p.bin,
                        source: p.source.as_deref(),
                    };
                    install::install_package(&pi, &bytes, self.dirs).with_context(|| format!("install {} {}", p.id, p.version))?;
                    fresh.push((p.id.clone(), p.bin.clone()));
                }
                packages.push(lock::Package { id: p.id.clone(), version: p.version.clone(), bins: p.bin.clone(), system: None });
            }
            if c.builtin.is_some() {
                // Its files are part of goatty: nothing to copy.
                return Ok(());
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
        if c.builtin.is_some() {
            log::info!("installed the packages of built-in plugin {}", m.id);
            return Ok(plan);
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
        if let Some(b) = builtins::get(id) {
            bail!(
                "{} comes with goatty: turn it off instead (goattyd plugin disable {id}); `goattyd shell uninstall {id}` removes goatty's download of it",
                b.manifest().name
            );
        }
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

    /// Remove goatty's downloaded packages of a built-in plugin (the plugin
    /// stays; system copies are used if there are any). Returns the
    /// packages that were removed.
    pub fn remove_packages(&self, b: &Builtin) -> Result<Vec<String>> {
        let _guard = op_lock(self.dirs)?;
        let mut removed = Vec::new();
        for p in &b.manifest().packages {
            if install::uninstall_package(&p.id, &p.bin, self.dirs)? {
                removed.push(p.id.clone());
            }
        }
        Ok(removed)
    }

    /// Remove goatty's download of a package that used to come with
    /// goatty ([`builtins::RETIRED`]), unless an installed plugin uses it
    /// now. `Ok(None)`: an installed plugin owns it (its id); `Ok(Some(removed))`
    /// otherwise.
    pub fn remove_retired(&self, package: &str, bins: &[&str]) -> Result<Option<bool>> {
        let _guard = op_lock(self.dirs)?;
        let lock = Lock::load(self.dirs)?;
        if lock.plugins.values().any(|e| e.packages.iter().any(|p| p.id == package)) {
            return Ok(None);
        }
        let bins: Vec<String> = bins.iter().map(|b| (*b).to_owned()).collect();
        install::uninstall_package(package, &bins, self.dirs).map(Some)
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
        let path = std::env::var("PATH").ok();
        let package_status = |m: &Manifest| -> Vec<PackageStatus> {
            m.packages
                .iter()
                .map(|p| PackageStatus {
                    id: p.id.clone(),
                    version: p.version.clone(),
                    state: match packages::locate(self.dirs, p, path.as_deref()) {
                        Location::System(found) => PackageState::System { path: found.display().to_string() },
                        Location::Managed { version, .. } => PackageState::Managed { version },
                        Location::Missing => PackageState::Missing,
                    },
                    available: p.asset_for(&target).is_some(),
                })
                .collect()
        };
        let mut installed = Vec::new();
        for b in builtins::BUILTINS {
            let m = b.manifest().clone();
            installed.push(Installed {
                id: b.id.into(),
                version: m.version.clone(),
                source: builtins::SOURCE.into(),
                verified: true,
                enabled: settings.plugins.get(b.id).is_none_or(|c| c.enabled),
                incompatible: None,
                packages: package_status(&m),
                builtin: true,
                manifest: m,
                update: None,
                installed_unix: 0,
            });
        }
        for (id, e) in lock.plugins.iter().filter(|(id, _)| builtins::get(id).is_none()) {
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
                packages: package_status(&m),
                builtin: false,
                manifest: m,
                update,
                installed_unix: e.installed,
            });
        }
        let mut available: Vec<Available> = Vec::new();
        for s in &snaps {
            for (id, entry) in &s.index.plugins {
                if available.iter().any(|a| a.id == *id) || builtins::get(id).is_some() {
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

/// An installed (or built-in), enabled, usable plugin, ready to render.
#[derive(Debug, Clone)]
pub struct Active {
    pub manifest: Manifest,
    pub values: Values,
    pub origin: Origin,
    /// `eza 1.0.0 (official)`.
    pub label: String,
}

/// Where an active plugin's files come from.
#[derive(Debug, Clone)]
pub enum Origin {
    /// Compiled into goatty.
    Builtin(&'static Builtin),
    /// `installed/<id>/`, checked against this lock entry.
    Installed(lock::Entry),
}

impl Active {
    /// One of the plugin's files; an installed plugin's is checked against the lock.
    pub fn read(&self, dirs: &Dirs, rel: &str) -> Result<Vec<u8>> {
        match &self.origin {
            Origin::Builtin(b) => b.read(rel).map(|t| t.as_bytes().to_vec()).with_context(|| format!("{}: no {rel}", b.id)),
            Origin::Installed(entry) => lock::read_checked(dirs, &self.manifest.id, entry, rel),
        }
    }

    /// Every file of the plugin (relative paths).
    pub fn file_names(&self) -> Vec<String> {
        match &self.origin {
            Origin::Builtin(b) => b.checksums().into_keys().collect(),
            Origin::Installed(entry) => entry.files.keys().cloned().collect(),
        }
    }

    /// The files the shell sources: the init file and the `fpath` directories' contents.
    pub fn shell_files(&self) -> Vec<String> {
        let m = &self.manifest;
        let mut v: Vec<String> = m.zsh.init.iter().cloned().collect();
        v.extend(self.file_names().into_iter().filter(|f| m.zsh.fpath.iter().any(|d| f.starts_with(&format!("{d}/")))));
        v
    }

    /// Its key in `goatty-status`: the bare id for built-in plugins
    /// (`atuin`), `plugin:<id>` for the others.
    pub fn feature(&self) -> String {
        match self.origin {
            Origin::Builtin(_) => self.manifest.id.clone(),
            Origin::Installed(_) => goatty_plugin::zsh::feature(&self.manifest.id),
        }
    }
}

/// The plugins that go into the shell configuration for `settings`, in
/// load order ([`load_order`]), and the problems found on the way (bad
/// values, missing or changed files, settings for plugins that aren't
/// installed).
pub fn active(dirs: &Dirs, settings: &HostSettings) -> (Vec<Active>, Vec<String>) {
    let mut out = Vec::new();
    let mut issues = Vec::new();
    let resolve = |m: &mut Manifest, issues: &mut Vec<String>| {
        let config = settings.plugins.get(&m.id);
        let stored = config.map(|c| c.settings.clone()).unwrap_or_default();
        let (values, problems) = schema::resolve(m, &stored);
        for (k, msg) in problems {
            issues.push(format!("plugins.{}.settings.{k}: {msg}", m.id));
        }
        // Aliases the user switched off stay out of the shell (#88).
        if let Some(off) = config.map(|c| &c.disabled_aliases).filter(|d| !d.is_empty()) {
            m.aliases.retain(|a| !off.contains(&a.name));
        }
        values
    };
    let enabled = |id: &str| settings.plugins.get(id).is_none_or(|c| c.enabled);
    for b in builtins::BUILTINS.iter().filter(|b| enabled(b.id)) {
        let mut m = b.manifest().clone();
        let values = resolve(&mut m, &mut issues);
        out.push(Active { manifest: m, values, origin: Origin::Builtin(b), label: b.label() });
    }
    let lock = match Lock::load(dirs) {
        Ok(l) => l,
        Err(e) => {
            issues.push(format!("{e:#}"));
            return (load_order(out), issues);
        }
    };
    for id in settings.plugins.keys().filter(|id| !lock.plugins.contains_key(*id) && builtins::get(id).is_none()) {
        issues.push(format!("plugins.{id}: not installed on this host; ignored (goattyd plugin install {id})"));
    }
    let target = current_target();
    for (id, entry) in lock.plugins.iter().filter(|(id, _)| builtins::get(id).is_none() && enabled(id)) {
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
        let label = format!("{} {} ({})", m.id, m.version, entry.source);
        let a = Active { manifest: m.clone(), values: Values::new(), origin: Origin::Installed(entry.clone()), label };
        // Everything the shell will source must still be what was approved.
        if let Some(e) = a.shell_files().iter().find_map(|rel| a.read(dirs, rel).err()) {
            issues.push(format!("plugin {id} is off: {e:#}"));
            continue;
        }
        let values = resolve(&mut m, &mut issues);
        out.push(Active { manifest: m, values, ..a });
    }
    (load_order(out), issues)
}

/// Sort plugins into the order they load: by [`Load`] slot, then within a
/// slot each after the plugins its `zsh.after` names (when they're there),
/// otherwise in the order given (built-in plugins first, then by id). A
/// cycle is broken by taking the first remaining plugin.
pub fn load_order(list: Vec<Active>) -> Vec<Active> {
    let mut out = Vec::with_capacity(list.len());
    let mut rest = list;
    for slot in Load::ALL {
        let (mut left, others): (Vec<Active>, Vec<Active>) = rest.into_iter().partition(|a| a.manifest.zsh.load == slot);
        rest = others;
        while !left.is_empty() {
            let ready =
                left.iter().position(|a| !a.manifest.zsh.after.iter().any(|dep| left.iter().any(|o| o.manifest.id == *dep))).unwrap_or(0);
            out.push(left.remove(ready));
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests;
