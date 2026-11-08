//! goatty's terminal engine.
//!
//! This crate is the *only* place that talks to `rio-vt` directly. Everything
//! above it (the GPUI renderer, the daemon) goes through [`Terminal`] and the
//! plain data types in this crate, so swapping the VT core (e.g. for
//! `wezterm-term` or `alacritty_terminal`) only touches this crate.
//!
//! Threading model: bytes are fed from a single I/O thread via
//! [`Terminal::feed`]; renderers take the lock briefly once per frame via
//! [`Terminal::with_term`] / [`Terminal::snapshot_into`] and copy out what
//! they need.

pub mod colors;
mod frame;
pub mod input;
mod links;
mod listener;
pub mod paste;
pub mod pty;
pub mod snapshot;
mod terminal;

pub use colors::{Palette, Rgb};
pub use frame::{Cell, CellFlags, Cursor, CursorKind, Frame};
pub use links::{Found, Link, LinkKind, LinkSpan, find_links};
pub use listener::UiEvent;
/// Terminal mode bits (DECCKM, mouse reporting, alt screen, ...).
pub use rio_vt::crosswords::Mode as TermMode;
pub use terminal::{DEFAULT_SCROLLBACK, SelectKind, TermSize, Terminal, TerminalChannels};
