---
paths:
  - "crates/goatty-app/src/settings_window/**"
  - "crates/goatty-app/src/ui/**"
---
# Settings window and goatty's UI look

One design language (Material 3 Expressive, the tonal-surface kind, with libadwaita's preference groups). Build from the pieces; don't hand-roll layout.

- **Tokens only.** Colors from `ui::style::Ui` (tonal surfaces derived from the terminal theme: `bg` page, `card`, `field` for filled controls, `accent_soft`/`accent_text` for quiet selection, `muted`/`faint` text). Sizes from `style::text` and `style::radius`. No literal colors, and no font sizes outside the scale.
- **Hierarchy:** page title 24 → section heading 15 semibold → row label 14 → description 12.5 muted. A heading is never smaller than the labels under it. No uppercase headings and no stray horizontal rules: cards and the dividers inside them do the grouping.
- **Page = sections.** `kit::section(ui, "Title")` (optional `.desc`, `.action` on the right, `.footer` note) holding `kit::row`s. One control per row, on the right. Full-width content (text fields, previews, choice cards) goes in `.below(…)`. Navigation rows use `.icon(…).on_click(…)` and get a chevron.
- **Pick the control by the choice:** a switch for on/off; `segmented` for 2–3 short options; `SettingsWindow::select` (dropdown with per-option descriptions) for longer labels or more options; `kit::choice_card` when seeing the result matters (cursor shape, tab style); `slider` for percentages (preview while dragging, save on release); `stepper` for small integers.
- **Don't hide state behind toggles that look like settings.** A filter belongs in its own UI (a `chip`, or a toggle inside a popover), never in a setting row.
- **Unavailable ≠ hidden:** `row.disabled(true)` with a description saying why ("Needs Starship"), so the layout doesn't jump.
- **Long lists get search, filters and folding:** `kit::search_field`, `kit::chip` filters with counts, collapsible groups, an `kit::empty` state that says what to do.
- **Text is short.** One-line descriptions; longer explanations go in a section `.footer` or a dropdown option's description. Write labels as plain statements ("Show the tab bar"), not questions.
- **Previews over prose:** when a setting changes how something looks (theme, cursor, prompt, tabs), show it.
- **Plugins:** the list only shows name, summary, status badges and the switch. Every plugin setting lives on the plugin's own page, grouped by the manifest's `group`, with `advanced` ones behind an expander. Pages like Prompt and History may show a built-in plugin's main switch where people look for it.
- **Icons:** Lucide-style SVGs in `assets/icons/` (24 grid, 2px stroke), listed in `ui/icons.rs`. `svg()` doesn't inherit the text color: set `text_color` on the svg (`icon_button` does, including hover).
- **Narrow windows:** below `RAIL_BELOW` the sidebar becomes a navigation rail with a drawer. Check pages at the 760 px minimum width; give fixed-width columns a `min_w` and let them shrink.
- **New settings:** add the row, then an entry in `search.rs`'s `INDEX` with the row's exact label, so search can find it and jump to it.
- **Check it by eye.** Screenshot the page, light and dark theme, wide and at minimum width. `cargo test` can't see layout.
- **Keyboard and screen readers.** Everything is reachable and usable without a mouse.
  - Controls in `ui/controls.rs` and `kit.rs` get their tab stop, keyboard-only focus ring and scroll-into-view from `ui::focus::tab` (the element needs a unique id). It does **not** name the control: set `.role(…)` and `.aria_label(…)` yourself, on the element that takes focus (a switch outside a `kit::Row` control, like a plugin or alias row's, needs its own `.aria_label`; so does a dropdown trigger, which gets the setting's name through `select`'s `name`). A new clickable element built by hand must call `focus::tab` too, and `.on_click` gives Enter/Space for free.
  - Controls are given a tab stop while the page is built. Code that renders later, while the window lays out (a `RenderOnce` like `kit::Row`, a child view like `ColorPicker`), must wrap its `focus::tab` calls in `focus::enter(window)` … `focus::leave(scope, window)`; otherwise they stay mouse-only.
  - Text fields expose a role and a label to screen readers: pass `TextInput::label` when the placeholder is an example ("gst") rather than a name.
  - Radio-like groups (`segmented`) have one tab stop and arrow keys. Sliders take arrows, Home/End and PageUp/Down. The sidebar is one tab stop (Up/Down change the page), and Ctrl+PageUp/Down step through pages. Dropdowns open with Enter, Space or Down, and arrows move inside the menu; Esc closes it and gives focus back.
  - Don't `.invisible()` a control keyboard users need: hidden elements aren't tab stops. For actions that show on hover, use `.opacity(0.)` with `group_hover(… opacity(1.))` and `.in_focus(|s| s.opacity(1.))` (`focus::show_when_focused` for a group of them).
  - A dialog, an open menu and the narrow window's drawer trap Tab: give its frame a `FocusHandle` and add it to `SettingsWindow::move_focus`. When one closes, move focus somewhere that still exists (the opener or the page): focus left on a removed element drops every key. A button that removes itself or its row (Delete, Reset) calls `focus::hand_over(page_root, Handover::Control(row_id), …)` so the user keeps their place; `focus::rescue` (run after layout) is only the net that moves focus to the page when a focused control vanished anyway.

