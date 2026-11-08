//! goatty's managed shell environment (Stage 3).
//!
//! A goatty session runs zsh with `ZDOTDIR` pointing at startup files
//! goatty generates, so the user gets autosuggestions, syntax highlighting,
//! completion, Starship, Atuin and shell integration without touching their
//! dotfiles. The user's `.zshenv`/`.zprofile` still run (and `.zshrc` on
//! request); the user's `ZDOTDIR` is restored once startup is done.
//!
//! * [`launch`]: which shell a session runs, and its environment,
//! * [`zsh`]: the startup files, rendered from the host settings,
//! * [`generation`]: build, validate, activate and roll back configurations,
//! * [`tools`]: pinned tool versions + checksums, and where tools are found,
//! * [`install`]: user-space installs of those tools (no sudo),
//! * [`doctor`]: asks a real managed shell what it loaded,
//! * [`service`]: host settings operations (goattyd's API, `goatty --local`),
//! * [`cmd`]: the `goattyd shell ...` commands.
//!
//! goatty never writes outside its own directories ([`dirs`]): no edits to
//! `~/.zshrc`, the global PATH or the login shell.

pub mod cmd;
pub mod dirs;
pub mod doctor;
pub mod generation;
pub mod install;
pub mod launch;
pub mod plugins;
pub mod service;
pub mod tools;
pub mod zsh;

pub use dirs::Dirs;
pub use launch::{Launch, Launcher};
pub use service::HostService;
