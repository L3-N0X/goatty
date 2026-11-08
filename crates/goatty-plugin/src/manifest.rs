//! `plugin.toml`: what a plugin is, what it installs, which settings it
//! exposes and how they reach the tool. See `docs/plugins.md` for the
//! format with examples.
//!
//! A manifest is data, never code goatty runs to find out what a plugin
//! does: everything security-relevant (downloads, programs on `PATH`,
//! aliases, environment variables, shell code) is visible here, so goatty
//! can show it before installing and derive the plugin's permissions from
//! it ([`crate::permissions`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The manifest format this goatty reads. A newer `schema` is refused with
/// "needs a newer goatty" instead of being half-understood.
pub const SCHEMA: u32 = 1;

/// The file name of a manifest inside a plugin directory.
pub const FILE: &str = "plugin.toml";

/// Download targets goatty knows (`"<arch>-<os>"`).
pub const TARGETS: &[&str] = &["x86_64-linux", "aarch64-linux", "x86_64-macos", "aarch64-macos"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Manifest format version ([`SCHEMA`]).
    pub schema: u32,
    /// Stable identifier: lowercase letters, digits and `-`, 2–40 characters.
    pub id: String,
    /// Display name.
    pub name: String,
    /// The plugin's own version (semver). Independent of the tool version:
    /// bump it whenever the manifest or its files change.
    pub version: String,
    /// One line for lists.
    pub summary: String,
    /// A few sentences for the details view: what the tool is and why you'd want it.
    #[serde(default)]
    pub description: String,
    /// Who maintains this plugin (not necessarily the tool's author).
    pub publisher: String,
    #[serde(default)]
    pub homepage: Option<String>,
    /// The tool's license (SPDX).
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub category: Category,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub compat: Compat,
    #[serde(default)]
    pub requires: Requires,
    #[serde(default, rename = "package")]
    pub packages: Vec<Package>,
    #[serde(default, rename = "setting")]
    pub settings: Vec<Setting>,
    #[serde(default, rename = "alias")]
    pub aliases: Vec<Alias>,
    #[serde(default)]
    pub env: Vec<EnvVar>,
    #[serde(default, rename = "file")]
    pub files: Vec<ConfigFile>,
    #[serde(default)]
    pub zsh: ZshIntegration,
    /// Why the plugin needs each permission it uses: `binaries = "..."`.
    /// See [`crate::permissions`].
    #[serde(default)]
    pub permissions: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Command-line tools, aliases, environment, completions.
    #[default]
    Environment,
    /// Prompt modules and appearance.
    Prompt,
    /// goatty commands and UI features (needs a runtime goatty doesn't have yet).
    Application,
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::Environment => "environment",
            Category::Prompt => "prompt",
            Category::Application => "application",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// Semver requirement on the goatty version, e.g. `">=0.1"`. Empty: any.
    #[serde(default)]
    pub goatty: String,
    /// `linux`, `macos`. Empty: wherever its packages have a build.
    #[serde(default)]
    pub os: Vec<String>,
    /// Shells it integrates with. Empty: `["zsh"]`.
    #[serde(default)]
    pub shells: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Requires {
    /// Commands that must already be installed on the host (goatty doesn't
    /// install them), e.g. `docker`. The integration is skipped without them.
    #[serde(default)]
    pub commands: Vec<String>,
    /// Other plugins that must be installed first.
    #[serde(default)]
    pub plugins: Vec<String>,
}

/// A tool the plugin installs into goatty's data directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    /// Directory name under `tools/` (lowercase, digits, `-`).
    pub id: String,
    /// The tool's version (as upstream names it).
    pub version: String,
    /// Executables to put on the shell's `PATH`, relative to the asset's
    /// `root`. The link is named after the file (`bin/eza`).
    pub bin: Vec<String>,
    /// Use a copy that's already on `PATH` instead of downloading one.
    #[serde(default = "yes")]
    pub prefer_system: bool,
    #[serde(rename = "asset")]
    pub assets: Vec<Asset>,
}

fn yes() -> bool {
    true
}

/// One downloadable build of a package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    /// `"<arch>-<os>"` (see [`TARGETS`]) or `"any"`.
    pub target: String,
    /// `https://` only.
    pub url: String,
    /// Lowercase hex sha256 of the download. Nothing is unpacked or run
    /// before it matches.
    pub sha256: String,
    /// Download size in bytes (shown before installing).
    pub size: u64,
    #[serde(default)]
    pub format: AssetFormat,
    /// Directory inside the archive holding the files (`""`: the top).
    #[serde(default)]
    pub root: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AssetFormat {
    #[default]
    #[serde(rename = "tar.gz")]
    TarGz,
    /// The download is the executable itself (one `bin` entry).
    #[serde(rename = "binary")]
    Binary,
}

/// A setting shown in goatty's settings window, with an explanation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Setting {
    /// Key in `host.json` (`plugins.<plugin>.settings.<id>`) and in
    /// templates (`{id}`): lowercase letters, digits, `_`.
    pub id: String,
    /// Short label.
    pub label: String,
    /// What it does, in plain words: shown under the label.
    #[serde(default)]
    pub help: String,
    #[serde(rename = "type")]
    pub kind: SettingKind,
    /// Value when the user hasn't set one. Required, except for `string`,
    /// `path` (empty) and `list` (no entries).
    #[serde(default)]
    pub default: Option<Value>,
    /// Heading the setting is grouped under.
    #[serde(default)]
    pub group: Option<String>,
    /// Hidden behind "Show advanced settings".
    #[serde(default)]
    pub advanced: bool,
    /// `choice`: the options.
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// `integer`: bounds and step.
    #[serde(default)]
    pub min: Option<i64>,
    #[serde(default)]
    pub max: Option<i64>,
    #[serde(default)]
    pub step: Option<i64>,
    /// `integer`: unit shown after the number (`ms`, `lines`).
    #[serde(default)]
    pub unit: Option<String>,
    /// `string`, `path`, `list`: hint shown in an empty field.
    #[serde(default)]
    pub placeholder: Option<String>,
    /// `list`: how `{id}` joins the entries (default `" "`).
    #[serde(default)]
    pub separator: Option<String>,
    /// The command-line argument this setting adds (`{id.arg}`, `{args}`).
    /// `bool`: added when on. Others: `{value}` is replaced by the value;
    /// nothing is added for an empty value. `list`: one argument per entry.
    #[serde(default)]
    pub arg: Option<String>,
    /// `bool`: the argument when off.
    #[serde(default)]
    pub arg_off: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingKind {
    Bool,
    Choice,
    String,
    Integer,
    Path,
    List,
}

impl SettingKind {
    pub fn label(self) -> &'static str {
        match self {
            SettingKind::Bool => "bool",
            SettingKind::Choice => "choice",
            SettingKind::String => "string",
            SettingKind::Integer => "integer",
            SettingKind::Path => "path",
            SettingKind::List => "list",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Choice {
    pub value: String,
    pub label: String,
    #[serde(default)]
    pub help: String,
    /// Overrides the setting's `arg` for this choice (`""`: no argument).
    #[serde(default)]
    pub arg: Option<String>,
}

/// `alias name='command args…'`, defined only when `command` is installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alias {
    pub name: String,
    /// One of the plugin's package executables or required commands.
    pub command: String,
    /// Templates (see [`crate::template`]).
    #[serde(default)]
    pub args: Vec<String>,
    /// Condition on settings (see [`Condition`]).
    #[serde(default)]
    pub when: Option<String>,
    /// Shown in the install plan and the settings window.
    #[serde(default)]
    pub description: String,
}

/// An exported environment variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvVar {
    pub name: String,
    /// A template; an empty result unsets nothing and exports nothing.
    pub value: String,
    #[serde(default)]
    pub when: Option<String>,
}

/// A config file goatty generates from the settings (into the shell
/// configuration generation), optionally pointed at by an env variable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// File name (letters, digits, `.`, `_`, `-`).
    pub name: String,
    #[serde(default)]
    pub format: FileFormat,
    /// `lines`: one template per line (whole-item `{id.arg}`/`{args}` allowed).
    #[serde(default)]
    pub lines: Vec<String>,
    /// `text`: the file, with `{id}` replaced by values.
    #[serde(default)]
    pub template: Option<String>,
    /// Export this variable with the file's path.
    #[serde(default)]
    pub env: Option<String>,
    #[serde(default)]
    pub when: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    #[default]
    Lines,
    Text,
}

/// Shell code shipped with the plugin. This is the one place a plugin runs
/// code of its own in every shell (`shell-code` permission).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZshIntegration {
    /// A file in the plugin directory, sourced at top level after goatty's
    /// own features (before the user's additions and syntax highlighting).
    /// `$goatty_settings[<id>]` holds each setting's value while it runs.
    #[serde(default)]
    pub init: Option<String>,
    /// Directories in the plugin directory added to `fpath` before
    /// `compinit` (completion functions).
    #[serde(default)]
    pub fpath: Vec<String>,
    #[serde(default)]
    pub when: Option<String>,
}

impl Manifest {
    pub fn setting(&self, id: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.id == id)
    }

    pub fn semver(&self) -> Option<semver::Version> {
        semver::Version::parse(&self.version).ok()
    }

    /// Every executable the plugin's packages put on `PATH`.
    pub fn executables(&self) -> impl Iterator<Item = &str> {
        self.packages.iter().flat_map(|p| p.bin.iter().map(|b| bin_name(b)))
    }

    /// The shells it integrates with (default zsh).
    pub fn shells(&self) -> Vec<&str> {
        if self.compat.shells.is_empty() { vec!["zsh"] } else { self.compat.shells.iter().map(String::as_str).collect() }
    }

    /// Whether this goatty version, OS and target can use the plugin; the
    /// reason when not.
    pub fn compatible(&self, goatty_version: &str, target: &str) -> Result<(), String> {
        if !self.compat.goatty.is_empty()
            && let (Ok(req), Ok(v)) = (semver::VersionReq::parse(&self.compat.goatty), semver::Version::parse(goatty_version))
            && !req.matches(&v)
        {
            return Err(format!("needs goatty {} (this is {goatty_version})", self.compat.goatty));
        }
        let os = target.rsplit('-').next().unwrap_or("");
        if !self.compat.os.is_empty() && !self.compat.os.iter().any(|o| o == os) {
            return Err(format!("supports {} only", self.compat.os.join(", ")));
        }
        if !self.shells().contains(&"zsh") {
            return Err(format!("integrates with {} only; goatty's managed shell is zsh", self.shells().join(", ")));
        }
        for p in &self.packages {
            if p.asset_for(target).is_none() {
                return Err(format!("{} has no build for {target}", p.id));
            }
        }
        Ok(())
    }
}

impl Package {
    pub fn asset_for(&self, target: &str) -> Option<&Asset> {
        self.assets.iter().find(|a| a.target == target).or_else(|| self.assets.iter().find(|a| a.target == "any"))
    }
}

/// `bin/eza` → `eza`.
pub fn bin_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A problem in a manifest: where (`setting.icons.default`) and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() { f.write_str(&self.message) } else { write!(f, "{}: {}", self.path, self.message) }
    }
}

/// Errors from [`parse`]: the manifest is unusable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub Vec<Problem>);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let lines: Vec<String> = self.0.iter().map(ToString::to_string).collect();
        write!(f, "invalid plugin manifest:\n  {}", lines.join("\n  "))
    }
}

impl std::error::Error for Invalid {}

/// Parse and validate a manifest. `files`: the files of the plugin
/// directory (relative paths with `/`), to check that referenced files
/// exist; `None` skips that check.
pub fn parse(text: &str, files: Option<&BTreeSet<String>>) -> Result<Manifest, Invalid> {
    let one = |path: &str, message: String| Invalid(vec![Problem { path: path.into(), message }]);
    let raw: toml::Table = toml::from_str(text).map_err(|e| one("", format!("not valid TOML: {}", e.message())))?;
    match raw.get("schema").and_then(toml::Value::as_integer) {
        None => return Err(one("schema", format!("missing; this goatty reads schema = {SCHEMA}"))),
        Some(n) if n > i64::from(SCHEMA) => {
            return Err(one("schema", format!("schema {n} needs a newer goatty (this one reads schema {SCHEMA})")));
        }
        Some(n) if n < 1 => return Err(one("schema", format!("{n} is not a schema version"))),
        Some(_) => {}
    }
    let m: Manifest = toml::from_str(text).map_err(|e| one("", e.message().to_string()))?;
    let problems = validate(&m, files);
    if problems.is_empty() { Ok(m) } else { Err(Invalid(problems)) }
}

/// Read `dir/plugin.toml` and check it against the directory's files.
pub fn load_dir(dir: &Path) -> anyhow::Result<Manifest> {
    let text = std::fs::read_to_string(dir.join(FILE)).map_err(|e| anyhow::anyhow!("read {}: {e}", dir.join(FILE).display()))?;
    let files = crate::bundle::list_files(dir)?;
    Ok(parse(&text, Some(&files.keys().cloned().collect()))?)
}

pub fn is_id(s: &str) -> bool {
    (2..=40).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && s.as_bytes()[0].is_ascii_lowercase()
        && !s.ends_with('-')
}

fn is_setting_id(s: &str) -> bool {
    (1..=40).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
        && s.as_bytes()[0].is_ascii_lowercase()
        // Reserved by the templates.
        && s != "args"
        && s != "value"
}

/// A word that's safe as a file or directory name and in a version string.
fn is_plain(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
        && s != "."
        && s != ".."
        && !s.starts_with('.')
}

/// Alias names: what zsh takes without quoting surprises.
fn is_alias_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.+-".contains(&b))
        && !s.starts_with('-')
        && !s.starts_with('.')
}

pub fn is_env_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
        && !s.as_bytes()[0].is_ascii_digit()
}

/// Variables a plugin may not set: they'd change how every program or the
/// shell itself behaves, or goatty's own startup.
const PROTECTED_ENV: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "SHELL",
    "ZDOTDIR",
    "FPATH",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "DYLD_INSERT_LIBRARIES",
    "DYLD_LIBRARY_PATH",
    "IFS",
    "PS1",
    "PROMPT",
    "TERM",
    "SSH_AUTH_SOCK",
];

/// A relative path inside the plugin directory, without `..` or hidden parts.
pub fn is_bundle_path(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('/') && s.split('/').all(is_plain)
}

fn validate(m: &Manifest, files: Option<&BTreeSet<String>>) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut err = |path: String, message: String| out.push(Problem { path, message });

    if !is_id(&m.id) {
        err("id".into(), format!("{:?}: use 2–40 lowercase letters, digits and '-', starting with a letter", m.id));
    }
    if m.name.trim().is_empty() {
        err("name".into(), "empty".into());
    }
    if semver::Version::parse(&m.version).is_err() {
        err("version".into(), format!("{:?} is not a semver version (like 1.0.0)", m.version));
    }
    if m.summary.trim().is_empty() || m.summary.len() > 120 {
        err("summary".into(), "one line of 1–120 characters".into());
    }
    if m.publisher.trim().is_empty() {
        err("publisher".into(), "empty: say who maintains this plugin".into());
    }
    if let Some(h) = &m.homepage
        && !h.starts_with("https://")
    {
        err("homepage".into(), "use an https:// URL".into());
    }
    if m.category == Category::Application {
        err("category".into(), "application plugins need a runtime goatty doesn't have yet".into());
    }
    if !m.compat.goatty.is_empty() && semver::VersionReq::parse(&m.compat.goatty).is_err() {
        err("compat.goatty".into(), format!("{:?} is not a version requirement (like \">=0.1\")", m.compat.goatty));
    }
    for os in &m.compat.os {
        if !["linux", "macos"].contains(&os.as_str()) {
            err("compat.os".into(), format!("unknown OS {os:?} (linux, macos)"));
        }
    }
    for sh in &m.compat.shells {
        if !["zsh", "bash", "fish"].contains(&sh.as_str()) {
            err("compat.shells".into(), format!("unknown shell {sh:?}"));
        }
    }
    for c in &m.requires.commands {
        if !is_plain(c) {
            err("requires.commands".into(), format!("{c:?} is not a command name"));
        }
    }
    for p in &m.requires.plugins {
        if !is_id(p) || *p == m.id {
            err("requires.plugins".into(), format!("{p:?} is not another plugin's id"));
        }
    }

    // Packages.
    let mut exes = BTreeSet::new();
    let mut pkg_ids = BTreeSet::new();
    for (i, p) in m.packages.iter().enumerate() {
        let at = format!("package[{i}]");
        if !is_id(&p.id) {
            err(format!("{at}.id"), format!("{:?}: lowercase letters, digits and '-'", p.id));
        }
        if !pkg_ids.insert(&p.id) {
            err(format!("{at}.id"), format!("{:?} appears twice", p.id));
        }
        if !is_plain(&p.version) {
            err(format!("{at}.version"), format!("{:?} is not a plain version string", p.version));
        }
        if p.bin.is_empty() {
            err(format!("{at}.bin"), "list the executables to put on PATH".into());
        }
        for b in &p.bin {
            if !is_bundle_path(b) {
                err(format!("{at}.bin"), format!("{b:?}: a relative path inside the archive"));
            } else if !exes.insert(bin_name(b).to_string()) {
                err(format!("{at}.bin"), format!("{:?} is provided twice", bin_name(b)));
            }
        }
        if p.assets.is_empty() {
            err(format!("{at}.asset"), "no downloads".into());
        }
        let mut targets = BTreeSet::new();
        for (j, a) in p.assets.iter().enumerate() {
            let at = format!("{at}.asset[{j}]");
            if a.target != "any" && !TARGETS.contains(&a.target.as_str()) {
                err(format!("{at}.target"), format!("{:?}: one of {} or \"any\"", a.target, TARGETS.join(", ")));
            }
            if !targets.insert(&a.target) {
                err(format!("{at}.target"), format!("{:?} appears twice", a.target));
            }
            if !a.url.starts_with("https://") || a.url.chars().any(char::is_whitespace) {
                err(format!("{at}.url"), "downloads must use https://".into());
            }
            if a.sha256.len() != 64 || !a.sha256.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                err(format!("{at}.sha256"), "64 lowercase hex digits (`goatty-plugin hash URL` prints it)".into());
            }
            if a.size == 0 {
                err(format!("{at}.size"), "the download size in bytes (`goatty-plugin hash URL` prints it)".into());
            }
            if !a.root.is_empty() && !is_bundle_path(&a.root) {
                err(format!("{at}.root"), format!("{:?}: a relative directory inside the archive", a.root));
            }
            if a.format == AssetFormat::Binary && p.bin.len() != 1 {
                err(format!("{at}.format"), "a \"binary\" download is one executable: list exactly one bin".into());
            }
        }
    }

    // Settings.
    let mut ids = BTreeSet::new();
    for (i, s) in m.settings.iter().enumerate() {
        let at = format!("setting.{}", if s.id.is_empty() { i.to_string() } else { s.id.clone() });
        if !is_setting_id(&s.id) {
            err(format!("{at}.id"), format!("{:?}: lowercase letters, digits and '_' (not \"args\"/\"value\")", s.id));
        }
        if !ids.insert(s.id.as_str()) {
            err(format!("{at}.id"), "appears twice".into());
        }
        if s.label.trim().is_empty() {
            err(format!("{at}.label"), "empty".into());
        }
        for e in crate::schema::check_definition(s) {
            err(format!("{at}.{}", e.0), e.1);
        }
        for (field, t) in [("arg", &s.arg), ("arg_off", &s.arg_off)] {
            if let Some(t) = t
                && let Err(e) = crate::template::check_inline(t, m, true)
            {
                err(format!("{at}.{field}"), e);
            }
        }
    }

    // Effects.
    let commands: BTreeSet<&str> = exes.iter().map(String::as_str).chain(m.requires.commands.iter().map(String::as_str)).collect();
    let cond = |path: String, when: &Option<String>, out: &mut Vec<Problem>| {
        if let Some(w) = when
            && let Err(e) = Condition::parse(w).and_then(|c| c.check(m))
        {
            out.push(Problem { path, message: e });
        }
    };
    let mut alias_names = BTreeSet::new();
    for (i, a) in m.aliases.iter().enumerate() {
        let at = format!("alias[{i}]");
        if !is_alias_name(&a.name) {
            out.push(Problem { path: format!("{at}.name"), message: format!("{:?} is not a usable alias name", a.name) });
        }
        if !alias_names.insert((&a.name, &a.when)) {
            out.push(Problem { path: format!("{at}.name"), message: format!("{:?} is defined twice for the same condition", a.name) });
        }
        if !commands.contains(a.command.as_str()) {
            out.push(Problem {
                path: format!("{at}.command"),
                message: format!("{:?} is neither a package executable nor in requires.commands", a.command),
            });
        }
        for t in &a.args {
            if let Err(e) = crate::template::check_item(t, m) {
                out.push(Problem { path: format!("{at}.args"), message: e });
            }
        }
        cond(format!("{at}.when"), &a.when, &mut out);
    }
    for (i, e) in m.env.iter().enumerate() {
        let at = format!("env[{i}]");
        if !is_env_name(&e.name) {
            out.push(Problem { path: format!("{at}.name"), message: format!("{:?}: uppercase letters, digits and '_'", e.name) });
        } else if PROTECTED_ENV.contains(&e.name.as_str()) || e.name.starts_with("GOATTY_") || e.name.starts_with("_GOATTY") {
            out.push(Problem { path: format!("{at}.name"), message: format!("plugins may not set {}", e.name) });
        }
        if let Err(msg) = crate::template::check_inline(&e.value, m, false) {
            out.push(Problem { path: format!("{at}.value"), message: msg });
        }
        cond(format!("{at}.when"), &e.when, &mut out);
    }
    let mut file_names = BTreeSet::new();
    for (i, f) in m.files.iter().enumerate() {
        let at = format!("file[{i}]");
        if !is_plain(&f.name) {
            out.push(Problem { path: format!("{at}.name"), message: format!("{:?} is not a plain file name", f.name) });
        }
        if !file_names.insert(&f.name) {
            out.push(Problem { path: format!("{at}.name"), message: "appears twice".into() });
        }
        let shipped = m.zsh.init.iter().chain(&m.zsh.fpath).any(|p| *p == f.name || p.starts_with(&format!("{}/", f.name)));
        if shipped {
            out.push(Problem {
                path: format!("{at}.name"),
                message: "collides with a file the plugin ships (zsh.init / zsh.fpath)".into(),
            });
        }
        match f.format {
            FileFormat::Lines => {
                if f.template.is_some() {
                    out.push(Problem { path: format!("{at}.template"), message: "\"lines\" files use `lines`, not `template`".into() });
                }
                for t in &f.lines {
                    if let Err(e) = crate::template::check_item(t, m) {
                        out.push(Problem { path: format!("{at}.lines"), message: e });
                    }
                }
            }
            FileFormat::Text => match &f.template {
                None => out.push(Problem { path: format!("{at}.template"), message: "\"text\" files need a `template`".into() }),
                Some(t) => {
                    if let Err(e) = crate::template::check_inline(t, m, false) {
                        out.push(Problem { path: format!("{at}.template"), message: e });
                    }
                }
            },
        }
        if let Some(env) = &f.env
            && (!is_env_name(env) || PROTECTED_ENV.contains(&env.as_str()) || env.starts_with("GOATTY_"))
        {
            out.push(Problem { path: format!("{at}.env"), message: format!("{env:?} can't be set by a plugin") });
        }
        cond(format!("{at}.when"), &f.when, &mut out);
    }
    if let Some(init) = &m.zsh.init {
        if !is_bundle_path(init) || !init.ends_with(".zsh") {
            out.push(Problem { path: "zsh.init".into(), message: format!("{init:?}: a .zsh file inside the plugin directory") });
        } else if let Some(files) = files
            && !files.contains(init)
        {
            out.push(Problem { path: "zsh.init".into(), message: format!("{init} doesn't exist") });
        }
    }
    for d in &m.zsh.fpath {
        if !is_bundle_path(d) {
            out.push(Problem { path: "zsh.fpath".into(), message: format!("{d:?}: a directory inside the plugin directory") });
        } else if let Some(files) = files
            && !files.iter().any(|f| f.starts_with(&format!("{d}/")))
        {
            out.push(Problem { path: "zsh.fpath".into(), message: format!("{d}/ has no files") });
        }
    }
    cond("zsh.when".into(), &m.zsh.when, &mut out);

    for p in crate::permissions::check(m) {
        out.push(p);
    }
    out
}

/// When an alias, variable, file or the zsh code applies: `setting`
/// (a bool that's on), `!setting` (off), `setting=value`, `setting!=value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    On(String),
    Off(String),
    Is(String, String),
    IsNot(String, String),
}

impl Condition {
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        if let Some((k, v)) = s.split_once("!=") {
            return Ok(Condition::IsNot(k.trim().into(), v.trim().into()));
        }
        if let Some((k, v)) = s.split_once('=') {
            return Ok(Condition::Is(k.trim().into(), v.trim().into()));
        }
        if let Some(k) = s.strip_prefix('!') {
            return Ok(Condition::Off(k.trim().into()));
        }
        if s.is_empty() {
            return Err("empty condition".into());
        }
        Ok(Condition::On(s.into()))
    }

    fn setting(&self) -> &str {
        match self {
            Condition::On(k) | Condition::Off(k) | Condition::Is(k, _) | Condition::IsNot(k, _) => k,
        }
    }

    /// Check it against the manifest's settings.
    pub fn check(&self, m: &Manifest) -> Result<(), String> {
        let s = m.setting(self.setting()).ok_or_else(|| format!("no setting {:?}", self.setting()))?;
        match (self, s.kind) {
            (Condition::On(_) | Condition::Off(_), SettingKind::Bool) => Ok(()),
            (Condition::On(_) | Condition::Off(_), _) => Err(format!("{} is not a bool: compare it (`{}=value`)", s.id, s.id)),
            (_, SettingKind::Bool | SettingKind::List) => Err(format!("{} can't be compared to a value", s.id)),
            (Condition::Is(_, v) | Condition::IsNot(_, v), SettingKind::Choice) if !s.choices.iter().any(|c| c.value == *v) => {
                Err(format!("{v:?} is not one of {}'s choices", s.id))
            }
            _ => Ok(()),
        }
    }

    pub fn eval(&self, values: &crate::schema::Values) -> bool {
        let get = |k: &str| values.get(k);
        match self {
            Condition::On(k) => get(k).and_then(Value::as_bool).unwrap_or(false),
            Condition::Off(k) => !get(k).and_then(Value::as_bool).unwrap_or(false),
            Condition::Is(k, v) => get(k).is_some_and(|x| crate::schema::plain_string(x) == *v),
            Condition::IsNot(k, v) => !get(k).is_some_and(|x| crate::schema::plain_string(x) == *v),
        }
    }
}

/// Evaluate an optional `when` (absent: always). Conditions were checked
/// when the manifest was parsed; a broken one counts as false.
pub fn applies(when: &Option<String>, values: &crate::schema::Values) -> bool {
    match when {
        None => true,
        Some(w) => Condition::parse(w).is_ok_and(|c| c.eval(values)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub const EXAMPLE: &str = r#"
schema = 1
id = "eza"
name = "eza"
version = "1.0.0"
summary = "A modern ls"
publisher = "goatty"
homepage = "https://eza.rocks"

[[package]]
id = "eza"
version = "0.23.4"
bin = ["eza"]
[[package.asset]]
target = "x86_64-linux"
url = "https://example.com/eza.tar.gz"
sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
size = 100

[[setting]]
id = "replace_ls"
label = "Use for ls"
type = "bool"
default = true

[[setting]]
id = "icons"
label = "Icons"
type = "choice"
default = "auto"
arg = "--icons={value}"
choices = [
  { value = "auto", label = "Automatic" },
  { value = "always", label = "Always" },
  { value = "never", label = "Never", arg = "" },
]

[[setting]]
id = "git"
label = "Git status"
type = "bool"
default = false
arg = "--git"

[[alias]]
name = "ls"
command = "eza"
args = ["{args}"]
when = "replace_ls"

[[env]]
name = "EZA_ICON_SPACING"
value = "2"
when = "icons!=never"

[permissions]
binaries = "eza is a program: goatty downloads it from its GitHub releases."
"#;

    #[test]
    fn parses_example() {
        let m = parse(EXAMPLE, None).unwrap();
        assert_eq!(m.id, "eza");
        assert_eq!(m.settings.len(), 3);
        assert_eq!(m.executables().collect::<Vec<_>>(), ["eza"]);
        assert!(m.compatible("0.0.1", "x86_64-linux").is_ok());
        assert!(m.compatible("0.0.1", "aarch64-linux").unwrap_err().contains("no build"));
    }

    fn problems(text: &str) -> Vec<String> {
        parse(text, None).unwrap_err().0.into_iter().map(|p| p.to_string()).collect()
    }

    #[test]
    fn rejects_newer_schema_and_bad_toml() {
        assert!(problems("schema = 2\nid = \"x\"")[0].contains("newer goatty"));
        assert!(problems("id = \"x\"")[0].contains("missing"));
        assert!(problems("schema = 1\nid = ")[0].contains("TOML"));
        // Unknown keys are typos, not silently ignored.
        let p = problems(&EXAMPLE.replace("homepage =", "homepag ="));
        assert!(p[0].contains("homepag"), "{p:?}");
    }

    #[test]
    fn reports_every_problem_with_its_path() {
        let bad = EXAMPLE
            .replace("id = \"eza\"\nname", "id = \"Eza!\"\nname")
            .replace("https://example.com", "http://example.com")
            .replace("when = \"replace_ls\"", "when = \"icons\"")
            .replace("name = \"EZA_ICON_SPACING\"", "name = \"LD_PRELOAD\"")
            .replace("binaries = ", "binaries_ = ");
        let p = problems(&bad);
        let has = |s: &str| p.iter().any(|x| x.contains(s));
        assert!(has("id: \"Eza!\""), "{p:?}");
        assert!(has("asset[0].url"), "{p:?}");
        assert!(has("alias[0].when: icons is not a bool"), "{p:?}");
        assert!(has("may not set LD_PRELOAD"), "{p:?}");
        assert!(has("permissions"), "{p:?}");
    }

    #[test]
    fn alias_commands_must_be_provided() {
        let p = problems(&EXAMPLE.replace("command = \"eza\"", "command = \"rm\""));
        assert!(p[0].contains("neither a package executable"), "{p:?}");
    }

    #[test]
    fn referenced_files_must_exist() {
        let text = EXAMPLE.replace(
            "[permissions]",
            "[zsh]\ninit = \"zsh/init.zsh\"\nfpath = [\"zsh/completions\"]\n\n[permissions]\nshell-code = \"defines the x function\"",
        );
        let p = parse(&text, Some(&BTreeSet::new())).unwrap_err().0;
        assert_eq!(p.len(), 2, "{p:?}");
        let files: BTreeSet<String> = ["plugin.toml", "zsh/init.zsh", "zsh/completions/_eza"].map(String::from).into();
        assert!(parse(&text, Some(&files)).is_ok());
    }

    #[test]
    fn conditions() {
        let m = parse(EXAMPLE, None).unwrap();
        let values = crate::schema::resolve(&m, &BTreeMap::new()).0;
        assert!(applies(&Some("replace_ls".into()), &values));
        assert!(!applies(&Some("!replace_ls".into()), &values));
        assert!(applies(&Some("icons=auto".into()), &values));
        assert!(applies(&Some("icons != never".into()), &values));
        assert!(Condition::parse("icons=nope").unwrap().check(&m).is_err());
        assert!(Condition::parse("missing").unwrap().check(&m).is_err());
    }

    #[test]
    fn ids() {
        assert!(is_id("eza") && is_id("zsh-you-should-use") && is_id("a1"));
        assert!(!is_id("a") && !is_id("Eza") && !is_id("-x") && !is_id("x-") && !is_id("1x") && !is_id("a_b"));
        assert!(is_bundle_path("zsh/init.zsh") && !is_bundle_path("../x") && !is_bundle_path("/etc") && !is_bundle_path("a/.hidden"));
    }
}
