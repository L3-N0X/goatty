//! The plugins that come with goatty: fzf, fzf-tab, autosuggestions,
//! Starship and syntax highlighting (`crates/goatty-shell/plugins/<id>/`).
//!
//! They're ordinary plugin manifests, compiled into goatty: same install
//! plans, settings UI and load-order slots as registry plugins. What's
//! different: they're always there (no lock entry, nothing to verify:
//! they're part of goatty), they're turned off instead of uninstalled, and
//! their status key in `goatty-status` is the bare id (`fzf`, not
//! `plugin:fzf`). Their packages keep the ids the old built-in tool table
//! used, so managed installs from before plugins (`tools/<id>/`, the
//! `bin/` and `plugins/` links) are picked up as they are.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use goatty_plugin::manifest::{self, Manifest};

/// `source` of a built-in plugin in catalogs and plans.
pub const SOURCE: &str = "goatty";

#[derive(Debug)]
pub struct Builtin {
    pub id: &'static str,
    /// `goatty-setup` (and `goattyd shell install` without names) installs
    /// its packages when they're missing. The others are integrated when
    /// their tool is there, and installed on request.
    pub default_install: bool,
    /// Relative path → contents.
    files: &'static [(&'static str, &'static str)],
}

macro_rules! plugin {
    ($id:literal, $default:expr, [$($f:literal),*]) => {
        Builtin {
            id: $id,
            default_install: $default,
            files: &[$(($f, include_str!(concat!("../plugins/", $id, "/", $f)))),*],
        }
    };
}

/// In the order the old `.zshrc` listed them.
pub static BUILTINS: &[Builtin] = &[
    plugin!("fzf", true, ["plugin.toml", "zsh/init.zsh"]),
    plugin!("fzf-tab", true, ["plugin.toml", "zsh/init.zsh"]),
    plugin!("autosuggestions", true, ["plugin.toml", "zsh/init.zsh"]),
    plugin!("starship", true, ["plugin.toml", "zsh/init.zsh"]),
    plugin!("syntax-highlighting", true, ["plugin.toml", "zsh/init.zsh"]),
];

/// Packages of plugins that used to come with goatty: (package id, its
/// executables). goatty's downloads of them stay usable by a registry
/// plugin with the same package (Atuin's, since #165) and are removed by
/// `goattyd shell uninstall NAME|--all`.
pub static RETIRED: &[(&str, &[&str])] = &[("atuin", &["atuin"])];

fn manifests() -> &'static [Manifest] {
    static PARSED: OnceLock<Vec<Manifest>> = OnceLock::new();
    PARSED.get_or_init(|| {
        BUILTINS
            .iter()
            .map(|b| {
                let files: BTreeSet<String> = b.files.iter().map(|(f, _)| (*f).to_owned()).collect();
                let text = b.read(manifest::FILE).expect("every built-in plugin has a plugin.toml");
                // Checked by the tests below: a broken built-in never ships.
                manifest::parse(text, Some(&files)).unwrap_or_else(|e| panic!("built-in plugin {}: {e}", b.id))
            })
            .collect()
    })
}

impl Builtin {
    pub fn manifest(&self) -> &'static Manifest {
        let i = BUILTINS.iter().position(|b| b.id == self.id).expect("one of BUILTINS");
        &manifests()[i]
    }

    pub fn read(&self, rel: &str) -> Option<&'static str> {
        self.files.iter().find(|(f, _)| *f == rel).map(|(_, text)| *text)
    }

    /// Every file with its sha256, like a registry index lists them.
    pub fn checksums(&self) -> BTreeMap<String, String> {
        self.files.iter().map(|(f, text)| ((*f).to_owned(), goatty_plugin::bundle::sha256_hex(text.as_bytes()))).collect()
    }

    /// `starship 1.0.0 (built in)`.
    pub fn label(&self) -> String {
        format!("{} {} (built in)", self.id, self.manifest().version)
    }
}

/// The built-in plugin with this id.
pub fn get(id: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.id == id)
}

/// A built-in plugin by id, display name, package id or upstream name
/// (`autosuggestions`, `zsh-autosuggestions`, `Starship`).
pub fn find(name: &str) -> Option<&'static Builtin> {
    get(name).or_else(|| {
        BUILTINS.iter().find(|b| {
            let m = b.manifest();
            m.name.eq_ignore_ascii_case(name)
                || m.packages.iter().any(|p| p.id == name)
                || m.homepage.as_deref().is_some_and(|h| h.trim_end_matches('/').rsplit('/').next() == Some(name))
        })
    })
}

/// Executables the built-in plugins put on PATH: name → plugin id. No
/// other plugin may provide them.
pub fn executables() -> BTreeMap<&'static str, &'static str> {
    BUILTINS.iter().flat_map(|b| b.manifest().executables().map(move |e| (e, b.id))).collect()
}

/// Package ids of the built-in plugins (`tools/<id>/` is theirs).
pub fn package_ids() -> BTreeMap<&'static str, &'static str> {
    BUILTINS.iter().flat_map(|b| b.manifest().packages.iter().map(move |p| (p.id.as_str(), b.id))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_validates_and_ships_its_files() {
        for b in BUILTINS {
            let m = b.manifest();
            assert_eq!(m.id, b.id);
            assert_eq!(m.publisher, "goatty");
            assert_eq!(b.checksums().len(), b.files.len());
            for f in m.zsh.init.iter() {
                assert!(b.read(f).is_some(), "{}: {f}", b.id);
            }
            for p in &m.packages {
                assert!(!p.assets.is_empty(), "{}", b.id);
                for a in &p.assets {
                    assert!(a.url.contains(&p.version), "{}: url doesn't match version {}", b.id, p.version);
                }
            }
            assert_eq!(find(b.id).map(|f| f.id), Some(b.id));
            assert_eq!(find(&m.name).map(|f| f.id), Some(b.id));
        }
        assert_eq!(find("zsh-autosuggestions").map(|b| b.id), Some("autosuggestions"));
        assert_eq!(find("zsh-syntax-highlighting").map(|b| b.id), Some("syntax-highlighting"));
        assert!(find("nope").is_none());
    }

    #[test]
    fn package_ids_match_the_old_tool_table() {
        // Installs made before plugins live in tools/<these ids>/.
        let ids: Vec<&str> = package_ids().into_keys().collect();
        assert_eq!(ids, ["autosuggestions", "fzf", "fzf-tab", "starship", "syntax-highlighting"]);
        assert_eq!(executables().into_keys().collect::<Vec<_>>(), ["fzf", "starship"]);
        for (id, _) in RETIRED {
            assert!(!ids.contains(id), "{id} is retired");
        }
    }

    #[test]
    fn load_slots() {
        use goatty_plugin::manifest::Load;
        let slot = |id: &str| get(id).unwrap().manifest().zsh.load;
        assert_eq!(slot("fzf"), Load::LineEditor);
        assert_eq!(slot("fzf-tab"), Load::LineEditor);
        assert_eq!(slot("autosuggestions"), Load::LineEditor);
        assert_eq!(slot("starship"), Load::Prompt);
        assert_eq!(slot("syntax-highlighting"), Load::Last);
    }
}
