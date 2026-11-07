//! Actions and their default key bindings.
//!
//! Shortcuts are GPUI actions bound in the `Terminal` key context, not
//! hard-coded key matches, so the Stage 4 keybinding editor can rebind them
//! and menus/command palettes can trigger them. GPUI dispatches a bound
//! action *before* the view's raw key handler, so anything bound here never
//! reaches the shell.

use gpui::{App, KeyBinding, actions};

/// Key context set by the terminal view.
pub const TERMINAL_CONTEXT: &str = "Terminal";

actions!(
    terminal,
    [
        /// Copy the selection to the clipboard.
        Copy,
        /// Copy if there is a selection, otherwise send Ctrl+C to the shell.
        CopyOrInterrupt,
        /// Paste the clipboard (bracketed if the application asked for it).
        Paste,
        /// Scroll back one screen.
        ScrollPageUp,
        /// Scroll forward one screen.
        ScrollPageDown,
        IncreaseFontSize,
        DecreaseFontSize,
    ]
);

pub fn bind_default_keys(cx: &mut App) {
    cx.bind_keys(default_bindings());
}

fn default_bindings() -> Vec<KeyBinding> {
    let ctx = Some(TERMINAL_CONTEXT);
    vec![
        KeyBinding::new("ctrl-shift-c", Copy, ctx),
        KeyBinding::new("ctrl-c", CopyOrInterrupt, ctx),
        KeyBinding::new("ctrl-shift-v", Paste, ctx),
        KeyBinding::new("ctrl-v", Paste, ctx),
        KeyBinding::new("shift-pageup", ScrollPageUp, ctx),
        KeyBinding::new("shift-pagedown", ScrollPageDown, ctx),
        // Layouts report Shift+= as "=" or "+"; bind both spellings.
        KeyBinding::new("ctrl-shift-=", IncreaseFontSize, ctx),
        KeyBinding::new("ctrl-shift-+", IncreaseFontSize, ctx),
        KeyBinding::new("ctrl-shift--", DecreaseFontSize, ctx),
        KeyBinding::new("ctrl-shift-_", DecreaseFontSize, ctx),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_bindings_parse() {
        // `KeyBinding::new` panics on an invalid keystroke; catch that here
        // instead of at startup.
        assert_eq!(super::default_bindings().len(), 10);
    }
}
