//! What a plugin is allowed to do, derived from its manifest.
//!
//! goatty doesn't take a plugin's word for what it does: the permissions
//! come from the manifest's contents (a `[[package]]` *is* the `binaries`
//! permission). The author has to explain the powerful ones in
//! `[permissions]`, and a manifest that declares one it doesn't use, or
//! uses one without explaining it, doesn't validate. The install plan
//! shows each permission with that explanation and the concrete details.
//!
//! Installed programs and shell code run with the user's privileges: they
//! can read the user's files and credentials. A registry signature proves
//! where a plugin came from and that it wasn't changed, not that it's
//! harmless; that's what the review and these prompts are for.

use serde::{Deserialize, Serialize};

use crate::manifest::{Manifest, Problem, bin_name};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    /// Downloads programs and puts them on the shell's `PATH`.
    Binaries,
    /// Runs its own shell code in every goatty shell.
    ShellCode,
    /// The tool talks to the network on its own (declared by the author).
    Network,
    /// Defines aliases (possibly replacing commands like `ls`).
    Aliases,
    /// Exports environment variables.
    Env,
    /// Generates config files for the tool.
    ConfigFiles,
}

impl Permission {
    pub const ALL: [Permission; 6] =
        [Permission::Binaries, Permission::ShellCode, Permission::Network, Permission::Aliases, Permission::Env, Permission::ConfigFiles];

    /// The key in `[permissions]`.
    pub fn key(self) -> &'static str {
        match self {
            Permission::Binaries => "binaries",
            Permission::ShellCode => "shell-code",
            Permission::Network => "network",
            Permission::Aliases => "aliases",
            Permission::Env => "env",
            Permission::ConfigFiles => "config-files",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.key() == key)
    }

    pub fn title(self) -> &'static str {
        match self {
            Permission::Binaries => "Installs and runs programs",
            Permission::ShellCode => "Runs its own shell code in every goatty shell",
            Permission::Network => "The tool uses the network",
            Permission::Aliases => "Adds aliases",
            Permission::Env => "Sets environment variables",
            Permission::ConfigFiles => "Generates config files",
        }
    }

    /// What it means, for the install plan.
    pub fn meaning(self) -> &'static str {
        match self {
            Permission::Binaries => {
                "Downloaded programs run with your permissions: they can read and change your files. goatty checks every download against the checksum in the manifest."
            }
            Permission::ShellCode => {
                "This code runs every time a goatty shell starts, with your permissions. Read it below before installing."
            }
            Permission::Network => "The tool itself connects to the internet (goatty doesn't control what it sends).",
            Permission::Aliases => "Typing these names runs the plugin's command instead.",
            Permission::Env => "These variables are set in every goatty shell.",
            Permission::ConfigFiles => "goatty writes these files into its own shell configuration; nothing in your home directory.",
        }
    }

    /// Can run arbitrary code as the user.
    pub fn runs_code(self) -> bool {
        matches!(self, Permission::Binaries | Permission::ShellCode)
    }

    /// The author has to say why (`[permissions]`).
    pub fn needs_reason(self) -> bool {
        matches!(self, Permission::Binaries | Permission::ShellCode | Permission::Network)
    }

    /// Only the author can know it's needed (not derivable from the manifest).
    fn declared_only(self) -> bool {
        self == Permission::Network
    }
}

/// The permissions the manifest's contents use, sorted.
pub fn derived(m: &Manifest) -> Vec<Permission> {
    let mut v = Vec::new();
    if !m.packages.is_empty() {
        v.push(Permission::Binaries);
    }
    if m.zsh.init.is_some() {
        v.push(Permission::ShellCode);
    }
    if !m.aliases.is_empty() {
        v.push(Permission::Aliases);
    }
    if !m.env.is_empty() || m.files.iter().any(|f| f.env.is_some()) {
        v.push(Permission::Env);
    }
    if !m.files.is_empty() {
        v.push(Permission::ConfigFiles);
    }
    v.sort();
    v
}

/// Everything the plugin may do: derived plus declared-only ones.
pub fn all(m: &Manifest) -> Vec<Permission> {
    let mut v = derived(m);
    for p in Permission::ALL.into_iter().filter(|p| p.declared_only()) {
        if m.permissions.contains_key(p.key()) {
            v.push(p);
        }
    }
    v.sort();
    v.dedup();
    v
}

/// Manifest problems: unknown keys, unexplained or unused permissions.
pub(crate) fn check(m: &Manifest) -> Vec<Problem> {
    let mut out = Vec::new();
    let used = derived(m);
    for (key, reason) in &m.permissions {
        let at = format!("permissions.{key}");
        match Permission::from_key(key) {
            None => out.push(Problem {
                path: at,
                message: format!("unknown permission (known: {})", Permission::ALL.map(Permission::key).join(", ")),
            }),
            Some(p) if !p.declared_only() && !used.contains(&p) => {
                out.push(Problem { path: at, message: "declared but the manifest doesn't use it; remove it".into() })
            }
            Some(_) if reason.trim().len() < 10 => {
                out.push(Problem { path: at, message: "explain in a sentence why the plugin needs this".into() })
            }
            Some(_) => {}
        }
    }
    for p in used.into_iter().filter(|p| p.needs_reason()) {
        if !m.permissions.contains_key(p.key()) {
            out.push(Problem {
                path: format!("permissions.{}", p.key()),
                message: format!("the manifest {}: say why in [permissions] {} = \"…\"", what_uses(p), p.key()),
            });
        }
    }
    out
}

fn what_uses(p: Permission) -> &'static str {
    match p {
        Permission::Binaries => "installs programs ([[package]])",
        Permission::ShellCode => "runs shell code (zsh.init)",
        _ => "uses it",
    }
}

/// One permission with the author's reason and what exactly it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Granted {
    pub permission: Permission,
    pub title: String,
    pub meaning: String,
    /// The author's explanation (empty when none was needed).
    pub reason: String,
    /// Concrete items: programs, alias names, variable names, files.
    pub details: Vec<String>,
}

/// The plugin's permissions, for the install plan and the details view.
pub fn describe(m: &Manifest) -> Vec<Granted> {
    all(m)
        .into_iter()
        .map(|p| {
            let details = match p {
                Permission::Binaries => m
                    .packages
                    .iter()
                    .map(|pk| format!("{} {}: {}", pk.id, pk.version, pk.bin.iter().map(|b| bin_name(b)).collect::<Vec<_>>().join(", ")))
                    .collect(),
                Permission::ShellCode => m.zsh.init.iter().cloned().collect(),
                Permission::Network => Vec::new(),
                Permission::Aliases => {
                    let mut names: Vec<String> = m.aliases.iter().map(|a| format!("{} → {}", a.name, a.command)).collect();
                    names.dedup();
                    names
                }
                Permission::Env => m.env.iter().map(|e| e.name.clone()).chain(m.files.iter().filter_map(|f| f.env.clone())).collect(),
                Permission::ConfigFiles => m.files.iter().map(|f| f.name.clone()).collect(),
            };
            Granted {
                permission: p,
                title: p.title().into(),
                meaning: p.meaning().into(),
                reason: m.permissions.get(p.key()).cloned().unwrap_or_default(),
                details,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{parse, tests::EXAMPLE};

    #[test]
    fn derives_from_contents() {
        let m = parse(EXAMPLE, None).unwrap();
        assert_eq!(derived(&m), [Permission::Binaries, Permission::Aliases, Permission::Env]);
        let d = describe(&m);
        assert_eq!(d[0].details, ["eza 0.23.4: eza"]);
        assert!(d[0].reason.contains("GitHub"));
        assert_eq!(d[1].details, ["ls → eza"]);
    }

    #[test]
    fn requires_reasons_and_rejects_unused() {
        let p = parse(&EXAMPLE.replace("binaries = ", "network = "), None).unwrap_err().0;
        assert!(p.iter().any(|p| p.path == "permissions.binaries" && p.message.contains("installs programs")), "{p:?}");
        let p = parse(&format!("{EXAMPLE}shell-code = \"runs zoxide init\"\n"), None).unwrap_err().0;
        assert!(p.iter().any(|p| p.path == "permissions.shell-code" && p.message.contains("doesn't use")), "{p:?}");
        let p = parse(&EXAMPLE.replace("eza is a program: goatty downloads it from its GitHub releases.", "x"), None).unwrap_err().0;
        assert!(p[0].message.contains("explain"), "{p:?}");
        let ok = parse(&format!("{EXAMPLE}network = \"eza can show git status of remote repos\"\n"), None).unwrap();
        assert!(all(&ok).contains(&Permission::Network));
    }
}
