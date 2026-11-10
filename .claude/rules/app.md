---
paths:
  - "crates/goatty-app/**"
---
# goatty-app (GPUI client)

- Shortcuts are GPUI actions (`actions.rs`, `Terminal` key context), never `match` arms in `on_key`. Bound actions run before the raw key handler. Unbound keys go to the shell through `terminal/keys.rs`.
- Every new user-facing action gets a `Command` in `command_palette::registry::builtins` with a stable `area.name` id (ids are stored in the recently-used list). Bindable actions also go through `goatty_config::keys::ACTIONS` and `actions::action_by_name` so the palette shows their shortcut.
- Anything that finishes without a visible result reports through `toast`:
  - `toast::show(cx, Toast::success("…"))` for a plain result
  - `Toast::loading` and then `toast::update` for background work
  - `key` for feedback that replaces itself
  - `.action(…)` for buttons
- Panes don't touch the window. `TerminalView` emits `TerminalEvent`s, and `Workspace` owns titles, closing, tabs and splits.
- Read client settings from the `AppSettings` global and change them only through `AppSettings::update`. Host settings go through the daemon (`HostLink`/`HostService`); never write another host's file.
- Reuse the widgets in `ui/` (controls, text_input, color_picker, and icons, whose SVGs are in `assets/icons/`) before writing new ones. Settings pages are built from `settings_window/kit.rs`; the look is in `settings-ui.md`.
- `cargo test` can't check GUI behaviour. Tell the user in your reply what to try by hand; don't file an issue for it.
