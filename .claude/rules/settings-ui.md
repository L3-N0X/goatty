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
