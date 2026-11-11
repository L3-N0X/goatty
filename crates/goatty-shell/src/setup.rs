//! Shell setup and diagnostics as JSON documents, for the GUI's first-run
//! setup and its Diagnostics page. They travel inside one protocol message
//! pair (`ClientMsg::Shell` / `ServerMsg::Shell`, v9) like plugin
//! operations do, so adding fields or operations doesn't need another
//! protocol bump. `goattyd shell status` prints the same [`Status`].
//!
//! Everything here runs on the host whose shells it describes: for a remote
//! goattyd, the remote host.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use goatty_config::{Effective, HostSettings};
use serde::{Deserialize, Serialize};

use crate::builtins::{self, BUILTINS, Builtin};
use crate::dirs::Dirs;
use crate::doctor::{self, State};
use crate::generation;
use crate::history;
use crate::packages;
use crate::plugins::{Candidate, Plugins};
use crate::service::HostService;

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Set in a session's environment when its client is a goatty window that
/// offers the setup itself: the managed zsh then keeps its missing-tools
/// hint to itself (and unsets the variable, so programs don't inherit it).
pub const GUI_SETUP_ENV: &str = "GOATTY_GUI_SETUP";

/// Mark `launch` as started for a goatty window that offers the setup.
pub fn gui_offers_setup(launch: &mut crate::Launch) {
    if launch.managed {
        launch.env.push((GUI_SETUP_ENV.into(), "1".into()));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// The configuration, what a real managed shell loads (started just
    /// now), how long it took, and goatty's built-in tools.
    Status,
    /// Download the packages of these built-in plugins (ids). The GUI sends
    /// one at a time to show progress.
    Install { tools: Vec<String> },
    /// The GUI offered to install the missing tools: the hint in new shells
    /// stays quiet from now on.
    Offered,
    /// "Don't ask again": no offers in the GUI, no hint in the shell.
    Dismiss,
    /// The generations with names and what each changed. Reply: `Generations`.
    Generations,
    /// Name a generation (`None` or blank: remove its name). Reply: `Done`.
    NameGeneration { number: u32, name: Option<String> },
    /// The generated files a generation changed, as unified diffs. Reply: `Diff`.
    GenerationDiff { number: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Reply {
    Status(Box<Status>),
    Installed(Installed),
    Done,
    Generations { list: Vec<history::Entry> },
    Diff { files: Vec<generation::FileDiff> },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Status {
    /// The shell a session starts.
    pub program: String,
    /// Sessions run goatty's managed configuration; else `unmanaged` says why.
    pub managed: bool,
    pub unmanaged: Option<String>,
    /// The active configuration's directory (`~/…`).
    pub config_dir: String,
    pub generation: Option<u32>,
    /// Problems with `host.json` (goatty used fallbacks).
    pub settings_issues: Vec<String>,
    /// `GOATTY_*` variables overriding settings in goattyd's environment.
    pub env_overrides: Vec<String>,
    /// The user's own startup files and whether goatty sources them.
    pub user_files: Vec<UserFile>,
    /// `~/.config/goatty/zshrc`: the user's additions.
    pub additions: String,
    pub additions_exist: bool,
    /// A real shell's startup, measured just now (managed shells only).
    pub startup: Option<Startup>,
    /// Why the shell couldn't be asked.
    pub startup_error: Option<String>,
    /// goatty's built-in plugins and where their tools are.
    pub tools: Vec<Tool>,
    /// The user chose "Don't ask again" (`goatty-setup --dismiss`).
    pub dismissed: bool,
    /// A GUI already offered the install.
    pub offered: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserFile {
    pub path: String,
    /// goatty's shell sources it (when it's there).
    pub sourced: bool,
    #[serde(default)]
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Startup {
    /// Login + interactive startup plus `goatty-status`.
    pub ms: u64,
    pub features: Vec<Feature>,
    /// What the startup printed to stderr (errors in startup files).
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feature {
    /// `goatty-status`'s key: `starship`, `integration`, `plugin:<id>`.
    pub id: String,
    /// For people: `Starship`, `Shell integration`.
    pub name: String,
    pub state: FeatureState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum FeatureState {
    /// Loaded. `detail`: the file or binary used (`~/…`), or what it does.
    /// `managed`: goatty's own copy, at this version.
    Loaded { detail: String, managed: Option<String> },
    /// Turned off in the settings.
    Off,
    /// On, but its tool isn't installed.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tool {
    /// The built-in plugin's id.
    pub id: String,
    pub name: String,
    pub summary: String,
    pub enabled: bool,
    /// The first-run setup (and `goatty-setup`) installs it when missing.
    pub default_install: bool,
    pub location: ToolLocation,
    /// What installing downloads (0: nothing to download).
    pub download: u64,
    /// The pinned version goatty would install.
    pub version: String,
    /// goatty can't install it here (no build for this system); install it
    /// with the package manager instead.
    pub unavailable: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolLocation {
    /// The system's copy (preferred).
    System {
        path: String,
    },
    /// goatty's copy.
    Managed {
        version: String,
    },
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Installed {
    /// One line per tool: what happened.
    pub lines: Vec<String>,
    /// How many failed.
    pub failed: usize,
}

impl Status {
    /// What the first-run setup offers: tools that are on, installed by
    /// default, missing, and that goatty can install here.
    pub fn offer(&self) -> impl Iterator<Item = &Tool> {
        self.tools.iter().filter(|t| t.default_install && t.enabled && t.installable())
    }

    /// Ask the user (no "don't ask again", something to install).
    pub fn should_offer(&self) -> bool {
        self.managed && !self.dismissed && self.offer().next().is_some()
    }

    pub fn tool(&self, id: &str) -> Option<&Tool> {
        self.tools.iter().find(|t| t.id == id)
    }
}

impl Tool {
    /// Missing, and goatty has a build to install.
    pub fn installable(&self) -> bool {
        self.location == ToolLocation::Missing && self.unavailable.is_none()
    }
}

/// `~/x` for paths under `$HOME`.
pub fn tilde(p: &Path) -> String {
    match std::env::var_os("HOME").and_then(|h| p.strip_prefix(h).ok().map(Path::to_path_buf)) {
        Some(rest) if rest.as_os_str().is_empty() => "~".into(),
        Some(rest) => format!("~/{}", rest.display()),
        None => p.display().to_string(),
    }
}

/// `12 kB`, `24.5 MB`.
pub fn size(bytes: u64) -> String {
    if bytes < 1_000_000 { format!("{} kB", bytes.div_ceil(1000)) } else { format!("{:.1} MB", bytes as f64 / 1e6) }
}

fn dirs_of(svc: &HostService) -> Result<&Dirs> {
    svc.launcher.dirs().context("$HOME is not set, so goatty has nowhere to keep its shell environment")
}

impl HostService {
    /// Answer a shell request (the protocol's `Shell` message, the GUI).
    /// Status starts a shell and installs download: call off the UI thread.
    pub fn shell_request(&self, req: Request) -> Result<Reply> {
        match req {
            Request::Status => Ok(Reply::Status(Box::new(status(self)?))),
            Request::Install { tools } => Ok(Reply::Installed(install(self, &tools)?)),
            Request::Offered => {
                touch(&dirs_of(self)?.offer_marker())?;
                Ok(Reply::Done)
            }
            Request::Dismiss => {
                touch(&dirs_of(self)?.hint_marker())?;
                Ok(Reply::Done)
            }
            Request::Generations => Ok(Reply::Generations { list: history::list(dirs_of(self)?) }),
            Request::NameGeneration { number, name } => {
                history::set_name(dirs_of(self)?, number, name.as_deref())?;
                Ok(Reply::Done)
            }
            Request::GenerationDiff { number } => Ok(Reply::Diff { files: history::file_diff(dirs_of(self)?, number)? }),
        }
    }
}

fn touch(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path.parent().context("marker has no parent")?)?;
    std::fs::write(path, "").with_context(|| format!("write {}", path.display()))
}

/// The diagnostics: configuration, a real shell's startup, the tools.
pub fn status(svc: &HostService) -> Result<Status> {
    let dirs = dirs_of(svc)?;
    let host = svc.settings();
    let launch = svc.launcher.launch(&host);
    let shell = &host.value.shell;
    let mut s = Status {
        program: launch.program.clone(),
        managed: launch.managed,
        config_dir: tilde(&dirs.zsh_dir()),
        generation: generation::current(dirs),
        settings_issues: host.file.issues.iter().map(ToString::to_string).collect(),
        env_overrides: host.env_overrides.iter().map(|s| s.to_string()).collect(),
        additions: tilde(&dirs.user_zshrc()),
        additions_exist: dirs.user_zshrc().exists(),
        dismissed: dirs.hint_marker().exists(),
        offered: dirs.offer_marker().exists(),
        ..Status::default()
    };
    if !launch.managed {
        s.unmanaged = Some(if !shell.managed {
            "Managed zsh is off: sessions use your own dotfiles only.".into()
        } else if crate::launch::is_zsh(&launch.program) {
            "goatty couldn't write its shell configuration (see goattyd's log): sessions use your own dotfiles only.".into()
        } else {
            "goatty's managed configuration is for zsh only: this shell starts as is.".into()
        });
    }
    let zdotdir = std::env::var_os("ZDOTDIR").or_else(|| std::env::var_os("HOME")).map(std::path::PathBuf::from).unwrap_or_default();
    let z = &shell.zsh;
    s.user_files = [(".zshenv", z.user_env), (".zprofile", z.user_env), (".zshrc", z.user_rc)]
        .into_iter()
        .map(|(name, sourced)| {
            let path = zdotdir.join(name);
            UserFile { path: tilde(&path), sourced, exists: path.is_file() }
        })
        .collect();

    let probe = if launch.managed {
        match doctor::probe(&launch, PROBE_TIMEOUT) {
            Ok(p) => Some(p),
            Err(e) => {
                s.startup_error = Some(format!("{e:#}"));
                None
            }
        }
    } else {
        None
    };
    if let Some(p) = &probe {
        s.startup = Some(Startup {
            ms: u64::try_from(p.elapsed.as_millis()).unwrap_or(u64::MAX),
            features: p.features.iter().map(|f| feature(&f.name, &f.state, dirs)).collect(),
            stderr: p.stderr.trim().to_owned(),
        });
    }
    s.tools = svc.plugins(|plugins| Ok(BUILTINS.iter().map(|b| tool(b, plugins, &host, probe.as_ref())).collect()))?;
    Ok(s)
}

fn feature(id: &str, state: &State, dirs: &Dirs) -> Feature {
    let name = match id {
        "integration" => "Shell integration".to_owned(),
        _ => match builtins::get(id) {
            Some(b) => b.manifest().name.clone(),
            None => id.strip_prefix("plugin:").unwrap_or(id).to_owned(),
        },
    };
    let state = match state {
        State::Ok(_) if id == "integration" => {
            FeatureState::Loaded { detail: "Prompt marks (OSC 133) and the working directory (OSC 7)".into(), managed: None }
        }
        State::Ok(detail) => {
            let path = Path::new(detail);
            let is_managed = path.starts_with(dirs.bin_dir()) || path.starts_with(dirs.plugins_dir());
            let managed = builtins::get(id)
                .filter(|_| is_managed)
                .and_then(|b| b.manifest().packages.iter().find_map(|p| packages::managed(dirs, p)).map(|(v, _)| v));
            FeatureState::Loaded { detail: tilde(path), managed }
        }
        State::Off => FeatureState::Off,
        State::Missing => FeatureState::Missing,
    };
    Feature { id: id.to_owned(), name, state }
}

/// A built-in plugin's tool: where the shell found it (when it could be
/// asked), else where goattyd finds it, and what installing would take.
fn tool(b: &'static Builtin, plugins: &Plugins<'_>, host: &Effective<HostSettings>, probe: Option<&doctor::Probe>) -> Tool {
    let m = b.manifest();
    let mut t = Tool {
        id: b.id.into(),
        name: m.name.clone(),
        summary: m.summary.clone(),
        enabled: host.value.plugin_enabled(b.id),
        default_install: b.default_install,
        location: ToolLocation::Missing,
        download: 0,
        version: m.packages.first().map(|p| p.version.clone()).unwrap_or_default(),
        unavailable: None,
    };
    let path = std::env::var("PATH").ok();
    let found: Vec<_> = m.packages.iter().map(|p| packages::locate(plugins.dirs, p, path.as_deref())).collect();
    let mut location = match found.first() {
        _ if found.contains(&packages::Location::Missing) => ToolLocation::Missing,
        Some(packages::Location::System(p)) => ToolLocation::System { path: tilde(p) },
        Some(packages::Location::Managed { version, .. }) => ToolLocation::Managed { version: version.clone() },
        _ => ToolLocation::Missing,
    };
    // The shell sees more of PATH than goattyd (the user's .zprofile ran).
    if let Some(State::Ok(detail)) = probe.and_then(|p| p.get(b.id))
        && location == ToolLocation::Missing
    {
        location = ToolLocation::System { path: tilde(Path::new(detail)) };
    }
    t.location = location;
    match plugins.plan(&Candidate::builtin(b), &host.value) {
        Ok(plan) => t.download = plan.downloads.iter().filter(|d| d.fetches()).map(|d| d.size).sum(),
        Err(e) => {
            if t.location == ToolLocation::Missing {
                t.unavailable = Some(format!("{e:#}"));
            }
        }
    }
    if t.location != ToolLocation::Missing {
        t.download = 0;
    }
    t
}

/// Install the packages of built-in plugins `ids` (pinned, checksummed,
/// into goatty's data directory). Already installed ones are skipped. The
/// plugins are on by default, so new shells use the tools at once.
pub fn install(svc: &HostService, ids: &[String]) -> Result<Installed> {
    let settings = svc.settings().value;
    let mut out = Installed::default();
    svc.plugins(|plugins| {
        for id in ids {
            let Some(b) = builtins::find(id) else {
                out.lines.push(format!("{id} isn't one of goatty's built-in tools."));
                out.failed += 1;
                continue;
            };
            let c = Candidate::builtin(b);
            let name = &b.manifest().name;
            let result = plugins.plan(&c, &settings).and_then(|plan| {
                if plan.downloads.iter().any(|d| d.fetches()) {
                    plugins.install(&c, &plan.digest, &settings).map(|p| Some(p.version))
                } else {
                    Ok(None)
                }
            });
            match result {
                Ok(Some(version)) => out.lines.push(format!("Installed {name} {version}.")),
                Ok(None) => out.lines.push(format!("{name} is already installed.")),
                Err(e) => {
                    out.lines.push(format!("{name}: {e:#}"));
                    out.failed += 1;
                }
            }
        }
        Ok(())
    })?;
    // The plugins find their tools at shell startup: no new generation.
    log::info!("shell tools: {}", out.lines.join(" "));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::launch::Launcher;

    #[test]
    fn requests_and_replies_round_trip_as_json() {
        let req = Request::Install { tools: vec!["starship".into()] };
        let text = serde_json::to_string(&req).unwrap();
        assert_eq!(text, r#"{"op":"install","tools":["starship"]}"#);
        assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
        let reply = Reply::Installed(Installed { lines: vec!["x".into()], failed: 0 });
        assert_eq!(serde_json::from_str::<Reply>(&serde_json::to_string(&reply).unwrap()).unwrap(), reply);
    }

    #[test]
    fn offered_and_dismissed_markers() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let svc = HostService::new(Launcher::new(Some(dirs.clone()), None), Some(tmp.path().join("config/host.json")));
        assert_eq!(svc.shell_request(Request::Offered).unwrap(), Reply::Done);
        assert!(dirs.offer_marker().exists() && !dirs.hint_marker().exists());
        svc.shell_request(Request::Dismiss).unwrap();
        assert!(dirs.hint_marker().exists());
    }

    #[test]
    fn generation_requests() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let svc = HostService::new(Launcher::new(Some(dirs.clone()), None), Some(tmp.path().join("config/host.json")));
        svc.set(r#"{"history": {"size": 5000}}"#, true).unwrap();
        svc.set(r#"{"history": {"size": 6000}}"#, true).unwrap();
        let name = Request::NameGeneration { number: 1, name: Some("small history".into()) };
        assert_eq!(svc.shell_request(name).unwrap(), Reply::Done);
        let json = serde_json::to_string(&svc.shell_request(Request::Generations).unwrap()).unwrap();
        let Ok(Reply::Generations { list }) = serde_json::from_str(&json) else { panic!("{json}") };
        assert_eq!(list[0].title(), "small history");
        assert_eq!(list[1].summary(), "history.size 5000 → 6000");
        let Reply::Diff { files } = svc.shell_request(Request::GenerationDiff { number: 2 }).unwrap() else { panic!() };
        assert_eq!(files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["settings.zsh"]);
    }

    #[test]
    fn offer_picks_missing_default_tools_goatty_can_install() {
        let t = |id: &str, default_install: bool, location: ToolLocation, unavailable: Option<&str>| Tool {
            id: id.into(),
            name: id.into(),
            summary: String::new(),
            enabled: true,
            default_install,
            location,
            download: 1,
            version: "1".into(),
            unavailable: unavailable.map(Into::into),
        };
        let mut s = Status {
            managed: true,
            tools: vec![
                t("starship", true, ToolLocation::Missing, None),
                t("atuin", true, ToolLocation::System { path: "/usr/bin/atuin".into() }, None),
                t("fzf", false, ToolLocation::Missing, None),
                t("autosuggestions", true, ToolLocation::Missing, Some("no build")),
            ],
            ..Status::default()
        };
        assert_eq!(s.offer().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["starship"]);
        assert!(s.should_offer());
        s.dismissed = true;
        assert!(!s.should_offer());
        s.dismissed = false;
        s.tools[0].enabled = false;
        assert!(!s.should_offer());
    }

    #[test]
    fn unknown_tools_fail_without_touching_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let svc = HostService::new(Launcher::new(Some(Dirs::under(tmp.path())), None), None);
        let r = install(&svc, &["nope".into()]).unwrap();
        assert_eq!(r.failed, 1);
        assert!(r.lines[0].contains("nope"));
    }
}
