//! Actions and their key bindings.
//!
//! Shortcuts are GPUI actions bound in the `Terminal` key context, not
//! hard-coded key matches, so the settings window can rebind them and menus
//! can trigger them. GPUI dispatches a bound action *before* the view's raw
//! key handler, so anything bound here never reaches the shell.
//!
//! The table of bindable actions and their default keys is data in
//! `goatty_config::keys`; this module maps names to GPUI actions and loads
//! the user's bindings (invalid keystrokes are reported, not fatal).

use std::collections::BTreeMap;
use std::rc::Rc;

use gpui::{Action, App, KeyBinding, KeyBindingContextPredicate, actions};

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
        /// Back to the font size from the settings.
        ResetFontSize,
        /// Open (or focus) the settings window.
        OpenSettings,
        /// Show the command palette (or close it).
        ToggleCommandPalette,
        /// The command palette with only the hosts to connect to.
        ConnectToHost,
        /// Ask the host's running managed shells to reload their configuration.
        ReloadShells,
    ]
);

actions!(
    workspace,
    [
        NewTab,
        /// Close the focused pane (the tab with its last pane).
        ClosePane,
        CloseTab,
        NextTab,
        PreviousTab,
        MoveTabLeft,
        MoveTabRight,
        SelectTab1,
        SelectTab2,
        SelectTab3,
        SelectTab4,
        SelectTab5,
        SelectTab6,
        SelectTab7,
        SelectTab8,
        SelectLastTab,
        RenameTab,
        SplitRight,
        SplitDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
        ResizePaneLeft,
        ResizePaneRight,
        ResizePaneUp,
        ResizePaneDown,
        /// Let the focused pane fill the tab, or go back to the split.
        TogglePaneZoom,
        EqualizePanes,
    ]
);

/// The GPUI action for a settings name (`goatty_config::keys::ACTIONS`).
pub fn action_by_name(name: &str) -> Option<Box<dyn Action>> {
    Some(match name {
        "copy" => Box::new(Copy),
        "copy_or_interrupt" => Box::new(CopyOrInterrupt),
        "paste" => Box::new(Paste),
        "scroll_page_up" => Box::new(ScrollPageUp),
        "scroll_page_down" => Box::new(ScrollPageDown),
        "increase_font_size" => Box::new(IncreaseFontSize),
        "decrease_font_size" => Box::new(DecreaseFontSize),
        "reset_font_size" => Box::new(ResetFontSize),
        "open_settings" => Box::new(OpenSettings),
        "command_palette" => Box::new(ToggleCommandPalette),
        "connect_to_host" => Box::new(ConnectToHost),
        "reload_shells" => Box::new(ReloadShells),
        "new_tab" => Box::new(NewTab),
        "close_pane" => Box::new(ClosePane),
        "close_tab" => Box::new(CloseTab),
        "next_tab" => Box::new(NextTab),
        "previous_tab" => Box::new(PreviousTab),
        "move_tab_left" => Box::new(MoveTabLeft),
        "move_tab_right" => Box::new(MoveTabRight),
        "select_tab_1" => Box::new(SelectTab1),
        "select_tab_2" => Box::new(SelectTab2),
        "select_tab_3" => Box::new(SelectTab3),
        "select_tab_4" => Box::new(SelectTab4),
        "select_tab_5" => Box::new(SelectTab5),
        "select_tab_6" => Box::new(SelectTab6),
        "select_tab_7" => Box::new(SelectTab7),
        "select_tab_8" => Box::new(SelectTab8),
        "select_last_tab" => Box::new(SelectLastTab),
        "rename_tab" => Box::new(RenameTab),
        "split_right" => Box::new(SplitRight),
        "split_down" => Box::new(SplitDown),
        "focus_pane_left" => Box::new(FocusPaneLeft),
        "focus_pane_right" => Box::new(FocusPaneRight),
        "focus_pane_up" => Box::new(FocusPaneUp),
        "focus_pane_down" => Box::new(FocusPaneDown),
        "resize_pane_left" => Box::new(ResizePaneLeft),
        "resize_pane_right" => Box::new(ResizePaneRight),
        "resize_pane_up" => Box::new(ResizePaneUp),
        "resize_pane_down" => Box::new(ResizePaneDown),
        "toggle_pane_zoom" => Box::new(TogglePaneZoom),
        "equalize_panes" => Box::new(EqualizePanes),
        _ => return None,
    })
}

/// Terminal bindings for the given overrides, plus problems found.
pub fn terminal_bindings(overrides: &BTreeMap<String, Option<String>>, cx: &App) -> (Vec<KeyBinding>, Vec<String>) {
    let ctx = KeyBindingContextPredicate::parse(TERMINAL_CONTEXT).ok().map(Rc::new);
    let mut bindings = Vec::new();
    let mut errors = Vec::new();
    for (key, name) in goatty_config::keys::effective_bindings(overrides) {
        let Some(action) = action_by_name(&name) else {
            errors.push(format!("keybindings.{key}: unknown action {name:?}"));
            continue;
        };
        match KeyBinding::load(&key, action, ctx.clone(), false, None, cx) {
            Ok(b) => bindings.push(b),
            Err(e) => errors.push(format!("keybindings.{key}: {e}")),
        }
    }
    (bindings, errors)
}

/// Replace every key binding: the terminal's (from settings) plus the
/// fixed ones of goatty's own UI. Returns problems with user bindings.
pub fn apply_keybindings(overrides: &BTreeMap<String, Option<String>>, cx: &mut App) -> Vec<String> {
    let (terminal, errors) = terminal_bindings(overrides, cx);
    for e in &errors {
        log::warn!("{e}");
    }
    cx.clear_key_bindings();
    cx.bind_keys(terminal);
    cx.bind_keys(crate::ui::text_input::bindings());
    cx.bind_keys(crate::ui::color_picker::bindings());
    cx.bind_keys(crate::settings_window::bindings());
    cx.bind_keys(crate::welcome::bindings());
    cx.bind_keys(crate::paste_confirm::bindings());
    cx.bind_keys(crate::close_confirm::bindings());
    let palette_keys = goatty_config::keys::keys_for("command_palette", overrides);
    let palette = crate::command_palette::bindings(&palette_keys, cx);
    cx.bind_keys(palette);
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_gpui_action() {
        for a in goatty_config::keys::ACTIONS {
            assert!(action_by_name(a.name).is_some(), "{}", a.name);
        }
    }

    #[test]
    fn default_keys_parse() {
        for (key, _) in goatty_config::keys::DEFAULT_BINDINGS {
            assert!(gpui::Keystroke::parse(key).is_ok(), "{key}");
        }
    }
}
