//! Plugin registries on a host: which ones are trusted, and verified local
//! snapshots of them.
//!
//! A registry is a URL (a `.tar.gz` of the registry repository, e.g.
//! GitHub's branch archive) or an absolute directory, plus the public keys
//! allowed to sign its index. goatty ships the official registry and its
//! key; others are added explicitly with their key (there's no "trust on
//! first use"). Refreshing downloads the registry, checks the signature,
//! the sequence (no going back to an older index) and every file, and only
//! then replaces `registries/<name>/` with the files the index lists.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use goatty_plugin::fetch::Fetch;
use goatty_plugin::index::{self, Index};
use serde::{Deserialize, Serialize};

use crate::dirs::Dirs;

pub const OFFICIAL_NAME: &str = "official";
/// The official registry: <https://github.com/goatty-dev/goatty-registry>.
pub const OFFICIAL_URL: &str = "https://github.com/goatty-dev/goatty-registry/archive/refs/heads/main.tar.gz";
/// Keys allowed to sign the official registry's index (made offline by
/// its maintainers; see the registry's README for rotation).
pub const OFFICIAL_KEYS: &[&str] = &["8b70a3acaf8193884b5e34f8341768d42d4f54ddcbe6e650178f6b458e409901"];

/// A trusted registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub name: String,
    /// `https://….tar.gz`, or an absolute directory path.
    pub url: String,
    /// Public keys (hex) allowed to sign its index.
    pub keys: Vec<String>,
    #[serde(skip)]
    pub builtin: bool,
}

impl Source {
    pub fn official() -> Self {
        Source {
            name: OFFICIAL_NAME.into(),
            url: OFFICIAL_URL.into(),
            keys: OFFICIAL_KEYS.iter().map(|k| (*k).to_owned()).collect(),
            builtin: true,
        }
    }

    pub fn check(&self) -> Result<()> {
        if !goatty_plugin::manifest::is_id(&self.name) {
            bail!("registry name {:?}: lowercase letters, digits and '-'", self.name);
        }
        if !(self.url.starts_with("https://") || Path::new(&self.url).is_absolute()) {
            bail!("registry URL {:?}: an https:// URL of a .tar.gz, or an absolute directory", self.url);
        }
        if self.keys.is_empty() {
            bail!("a registry needs the public key that signs its index");
        }
        for k in &self.keys {
            if k.len() != 64 || !k.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("{k:?} is not a public key (64 hex digits)");
            }
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SourcesFile {
    #[serde(default)]
    version: u32,
    /// `false`: don't use the built-in registries (e.g. an organization
    /// that only allows its own).
    #[serde(default = "yes")]
    builtin: bool,
    #[serde(default)]
    registries: Vec<Source>,
}

fn yes() -> bool {
    true
}

impl Default for SourcesFile {
    fn default() -> Self {
        Self { version: 1, builtin: true, registries: Vec::new() }
    }
}

/// The built-in registries plus the ones in `registries.json`, in order
/// (earlier ones win when two offer the same plugin). Problems with the
/// file are returned, not fatal.
pub fn sources(dirs: &Dirs, builtin: &[Source]) -> (Vec<Source>, Vec<String>) {
    let mut out: Vec<Source> = builtin.to_vec();
    let mut issues = Vec::new();
    match std::fs::read(dirs.registries_file()) {
        Ok(bytes) => match serde_json::from_slice::<SourcesFile>(&bytes) {
            Ok(f) => {
                if !f.builtin {
                    out.clear();
                }
                for s in f.registries {
                    match s.check() {
                        Ok(()) if out.iter().any(|o| o.name == s.name) => {
                            issues.push(format!("registries.json: {} is defined twice; the first one is used", s.name))
                        }
                        Ok(()) => out.push(s),
                        Err(e) => issues.push(format!("registries.json: {e:#}")),
                    }
                }
            }
            Err(e) => issues.push(format!("registries.json is unreadable ({e}); only the official registry is used")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => issues.push(format!("can't read registries.json: {e}")),
    }
    (out, issues)
}

fn save_user_sources(dirs: &Dirs, list: Vec<Source>) -> Result<()> {
    let builtin = read_file(dirs).map(|f| f.builtin).unwrap_or(true);
    let f = SourcesFile { version: 1, builtin, registries: list };
    let mut text = serde_json::to_string_pretty(&f)?;
    text.push('\n');
    goatty_config::store::write_text(&dirs.registries_file(), &text).context("write registries.json")?;
    Ok(())
}

fn read_file(dirs: &Dirs) -> Option<SourcesFile> {
    std::fs::read(dirs.registries_file()).ok().and_then(|b| serde_json::from_slice::<SourcesFile>(&b).ok())
}

fn user_sources(dirs: &Dirs) -> Vec<Source> {
    read_file(dirs).map(|f| f.registries).unwrap_or_default()
}

/// Trust another registry.
pub fn add_source(dirs: &Dirs, builtin: &[Source], source: Source) -> Result<()> {
    source.check()?;
    if builtin.iter().any(|b| b.name == source.name) {
        bail!("{} is built in", source.name);
    }
    let mut list = user_sources(dirs);
    list.retain(|s| s.name != source.name);
    list.push(source);
    save_user_sources(dirs, list)
}

/// Stop trusting a registry and drop its snapshot. Installed plugins stay.
pub fn remove_source(dirs: &Dirs, name: &str) -> Result<bool> {
    let mut list = user_sources(dirs);
    let before = list.len();
    list.retain(|s| s.name != name);
    if list.len() == before {
        return Ok(false);
    }
    save_user_sources(dirs, list)?;
    let _ = std::fs::remove_dir_all(dirs.registries_dir().join(name));
    Ok(true)
}

/// A verified local copy of a registry.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub source: Source,
    pub dir: PathBuf,
    pub index: Index,
    pub fetched: u64,
}

impl Snapshot {
    /// `plugins/<id>/<version>/`.
    pub fn plugin_dir(&self, id: &str, version: &str) -> PathBuf {
        self.dir.join("plugins").join(id).join(version)
    }
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// The stored snapshot of `source`, verified again (signature + files).
/// `None` when it was never fetched.
pub fn load(dirs: &Dirs, source: &Source) -> Result<Option<Snapshot>> {
    let dir = dirs.registries_dir().join(&source.name);
    if !dir.join(index::INDEX).exists() {
        return Ok(None);
    }
    let index = index::verify_snapshot(&dir, &source.keys, 0).with_context(|| format!("stored copy of registry {}", source.name))?;
    let fetched = std::fs::read_to_string(dir.join(".fetched")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0);
    Ok(Some(Snapshot { source: source.clone(), dir, index, fetched }))
}

/// Fetch `source`, verify it, and replace the stored snapshot. On any
/// failure the old snapshot stays.
pub fn refresh(dirs: &Dirs, source: &Source, fetch: &dyn Fetch) -> Result<Snapshot> {
    let min_sequence = load(dirs, source).ok().flatten().map(|s| s.index.sequence).unwrap_or(0);
    let root = dirs.registries_dir();
    std::fs::create_dir_all(&root)?;
    let lock = std::fs::File::create(root.join(".lock"))?;
    lock.lock()?;

    let unpacked = dirs.cache.join(format!("registry-{}-{}", source.name, std::process::id()));
    let _ = std::fs::remove_dir_all(&unpacked);
    let result = (|| -> Result<Snapshot> {
        let src = if source.url.starts_with("https://") {
            let bytes = fetch.fetch(&source.url).with_context(|| format!("download registry {}", source.name))?;
            std::fs::create_dir_all(&unpacked)?;
            let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
            ar.set_preserve_permissions(false);
            ar.unpack(&unpacked).context("unpack the registry")?;
            archive_root(&unpacked)?
        } else {
            PathBuf::from(&source.url)
        };
        let index = index::verify_snapshot(&src, &source.keys, min_sequence).with_context(|| format!("registry {}", source.name))?;
        if index.registry != source.name {
            log::info!("registry {} calls itself {:?}", source.name, index.registry);
        }

        let staging = root.join(format!(".{}.partial-{}", source.name, std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let copied = (|| -> Result<()> {
            for f in [index::INDEX, index::SIGNATURE] {
                std::fs::copy(src.join(f), staging.join(f)).with_context(|| format!("copy {f}"))?;
            }
            for (id, e) in &index.plugins {
                for (v, ver) in &e.versions {
                    let rel = Path::new("plugins").join(id).join(v);
                    goatty_plugin::bundle::copy_verified(&src.join(&rel), &staging.join(&rel), &ver.files)?;
                }
            }
            std::fs::write(staging.join(".fetched"), now().to_string())?;
            Ok(())
        })();
        if let Err(e) = copied {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
        let dest = root.join(&source.name);
        let old = root.join(format!(".{}.old-{}", source.name, std::process::id()));
        if dest.exists() {
            std::fs::rename(&dest, &old)?;
        }
        std::fs::rename(&staging, &dest)?;
        let _ = std::fs::remove_dir_all(&old);
        load(dirs, source)?.context("the new snapshot vanished")
    })();
    let _ = std::fs::remove_dir_all(&unpacked);
    result
}

/// The registry inside an unpacked archive: the top level, or its only
/// directory (GitHub archives wrap everything in `<repo>-<branch>/`).
fn archive_root(dir: &Path) -> Result<PathBuf> {
    if dir.join(index::INDEX).exists() {
        return Ok(dir.to_path_buf());
    }
    let entries: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    match entries.as_slice() {
        [only] if only.join(index::INDEX).exists() => Ok(only.clone()),
        _ => bail!("the download doesn't look like a goatty registry (no index.json)"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A plugin directory for [`make_registry`]: id, version, plugin.toml, extra files.
    pub type TestPlugin<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, &'a str)]);

    /// A signed registry directory with `plugins`. Returns its public key.
    pub fn make_registry(dir: &Path, plugins: &[TestPlugin<'_>]) -> (String, ed25519_dalek::SigningKey) {
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let public = goatty_plugin::sign::public_of(&key);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(index::META), format!("name = \"test\"\nkeys = [\"{public}\"]\n")).unwrap();
        for (id, v, manifest, files) in plugins {
            let p = dir.join("plugins").join(id).join(v);
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("plugin.toml"), manifest).unwrap();
            for (rel, text) in *files {
                std::fs::create_dir_all(p.join(rel).parent().unwrap()).unwrap();
                std::fs::write(p.join(rel), text).unwrap();
            }
        }
        publish(dir, &key);
        (public, key)
    }

    pub fn publish(dir: &Path, key: &ed25519_dalek::SigningKey) {
        let idx = index::build(dir).unwrap();
        std::fs::write(dir.join(index::INDEX), idx.to_bytes()).unwrap();
        index::sign_dir(dir, key).unwrap();
    }

    pub const DEMO: &str = r#"
schema = 1
id = "demo"
name = "Demo"
version = "1.0.0"
summary = "A demo tool"
publisher = "tests"

[[package]]
id = "demo"
version = "2.0"
bin = ["demo"]
[[package.asset]]
target = "any"
url = "https://example.com/demo.tar.gz"
sha256 = "SHA"
size = 10

[[setting]]
id = "loud"
label = "Loud"
type = "bool"
default = false
arg = "--loud"

[[alias]]
name = "d"
command = "demo"
args = ["{args}"]

[permissions]
binaries = "demo is the program this plugin is about"
"#;

    #[test]
    fn refresh_verifies_and_keeps_old_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(&tmp.path().join("host"));
        let reg = tmp.path().join("reg");
        let manifest = DEMO.replace("SHA", &"a".repeat(64));
        let (public, key) = make_registry(&reg, &[("demo", "1.0.0", &manifest, &[])]);
        let source = Source { name: "test".into(), url: reg.to_string_lossy().into_owned(), keys: vec![public.clone()], builtin: false };
        let none = goatty_plugin::fetch::Fixed::default();

        assert!(load(&dirs, &source).unwrap().is_none());
        let snap = refresh(&dirs, &source, &none).unwrap();
        assert_eq!(snap.index.latest("demo").unwrap().0, "1.0.0");
        assert!(snap.plugin_dir("demo", "1.0.0").join("plugin.toml").is_file());
        assert!(!snap.dir.join("registry.toml").exists(), "only signed files are copied");

        // Tampered upstream: refused, the old snapshot stays.
        std::fs::write(reg.join("plugins/demo/1.0.0/plugin.toml"), manifest.replace("A demo", "An evil")).unwrap();
        assert!(refresh(&dirs, &source, &none).unwrap_err().to_string().contains("registry test"));
        assert_eq!(load(&dirs, &source).unwrap().unwrap().index.plugins["demo"].summary, "A demo tool");

        // A wrong key: refused.
        let wrong = Source { keys: vec!["0".repeat(64)], ..source.clone() };
        assert!(refresh(&dirs, &wrong, &none).is_err());

        // Re-published properly: accepted, sequence 2. Replaying sequence 1 is refused.
        let old_index = std::fs::read(reg.join(index::INDEX)).unwrap();
        let old_sig = std::fs::read(reg.join(index::SIGNATURE)).unwrap();
        publish(&reg, &key);
        assert_eq!(refresh(&dirs, &source, &none).unwrap().index.sequence, 2);
        std::fs::write(reg.join(index::INDEX), old_index).unwrap();
        std::fs::write(reg.join(index::SIGNATURE), old_sig).unwrap();
        std::fs::write(reg.join("plugins/demo/1.0.0/plugin.toml"), &manifest).unwrap();
        let err = refresh(&dirs, &source, &none).unwrap_err();
        assert!(format!("{err:#}").contains("older index"), "{err:#}");
    }

    #[test]
    fn refresh_from_a_tarball() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(&tmp.path().join("host"));
        let reg = tmp.path().join("goatty-registry-main");
        let (public, _) = make_registry(&reg, &[("demo", "1.0.0", &DEMO.replace("SHA", &"a".repeat(64)), &[])]);
        let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        b.append_dir_all("goatty-registry-main", &reg).unwrap();
        let tgz = b.into_inner().unwrap().finish().unwrap();
        let url = "https://example.com/registry.tar.gz";
        let fetch = goatty_plugin::fetch::Fixed([(url.to_string(), tgz)].into());
        let source = Source { name: "test".into(), url: url.into(), keys: vec![public], builtin: false };
        assert_eq!(refresh(&dirs, &source, &fetch).unwrap().index.plugins.len(), 1);
    }

    #[test]
    fn user_sources_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::under(tmp.path());
        let builtin = [Source::official()];
        let acme =
            Source { name: "acme".into(), url: "https://acme.example/reg.tar.gz".into(), keys: vec!["ab".repeat(32)], builtin: false };
        add_source(&dirs, &builtin, acme.clone()).unwrap();
        assert!(add_source(&dirs, &builtin, Source { name: "official".into(), ..acme.clone() }).is_err());
        assert!(add_source(&dirs, &builtin, Source { name: "x".into(), ..acme.clone() }).is_err(), "bad name");
        assert!(add_source(&dirs, &builtin, Source { url: "http://plain".into(), ..acme.clone() }).is_err());
        let (list, issues) = sources(&dirs, &builtin);
        assert_eq!(list.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["official", "acme"]);
        assert!(issues.is_empty());
        assert!(remove_source(&dirs, "acme").unwrap());
        assert!(!remove_source(&dirs, "acme").unwrap());

        std::fs::write(dirs.registries_file(), r#"{"builtin": false, "registries": []}"#).unwrap();
        assert!(sources(&dirs, &builtin).0.is_empty(), "built-in registries can be turned off");
        add_source(&dirs, &builtin, acme).unwrap();
        assert_eq!(sources(&dirs, &builtin).0.len(), 1, "and stay off when adding one");
    }
}
