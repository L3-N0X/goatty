//! The terminal pane: a view that turns GPUI input into terminal input, and
//! a custom element that paints the grid.

mod element;
mod glyphs;
mod keys;
mod stats;
mod view;

pub use view::{TerminalEvent, TerminalView};
