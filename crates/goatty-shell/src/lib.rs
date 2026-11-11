//! goatty's managed shell environment (Stage 3).
//!
//! A goatty session runs zsh with `ZDOTDIR` pointing at startup files
//! goatty generates, so the user gets autosuggestions, syntax highlighting,
//! completion, Starship, Atuin and shell integration without touching their
//! dotfiles. The user's `.zshenv`/`.zprofile` still run (and `.zshrc` on
//! request); the user's `ZDOTDIR` is restored once startup is done.
//!
//! * [`aliases`]: the user's and the plugins' aliases, `goattyd alias …`,
//! * [`launch`]: which shell a session runs, and its environment,
//! * [`zsh`]: the startup files, rendered from the host settings,
//! * [`generation`]: build, validate, activate and roll back configurations,
//! * [`history`]: what each generation changed, names for them,
//! * [`builtins`]: the plugins that come with goatty (autosuggestions,
//!   syntax highlighting, Starship, Atuin, fzf),
//! * [`plugins`]: registries, install plans, installs, what goes into the shell,
//! * [`packages`]: where a plugin's package is found (system or goatty's copy),
//! * [`install`]: user-space installs of packages (no sudo),
//! * [`doctor`]: asks a real managed shell what it loaded,
//! * [`service`]: host settings operations (goattyd's API, `goatty --local`),
//! * [`setup`]: diagnostics and installing the built-in tools, for the GUI,
//! * [`cmd`]: the `goattyd shell ...` commands.
//!
//! goatty never writes outside its own directories ([`dirs`]): no edits to
//! `~/.zshrc`, the global PATH or the login shell.

pub mod aliases;
pub mod builtins;
pub mod cmd;
pub mod dirs;
pub mod doctor;
pub mod generation;
pub mod history;
pub mod install;
pub mod launch;
pub mod packages;
pub mod plugins;
pub mod service;
pub mod setup;
pub mod zsh;

pub use dirs::Dirs;
pub use launch::{Launch, Launcher};
pub use service::HostService;
