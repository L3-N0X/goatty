//! The Appearance page: a live preview, the theme gallery and color editor,
//! text, cursor and window settings.

use goatty_config::{ColorSlot, CursorShape, FontSettings, Renderer, TerminalSettings, ThemeColors, ThemeEntry};
use gpui::{
    AnyElement, Context, Focusable, FontWeight, Hsla, IntoElement, MouseButton, ParentElement, SharedString, Styled, Window, div,
    prelude::*, px, rgb,
};

use super::kit::{self, Opt, chip, menu, menu_item, opt, row, search_field, section, select_trigger};
use super::{SettingsWindow, ThemeTarget, client};
use crate::settings::AppSettings;
use crate::ui::controls::{
    ButtonKind, Tone, badge, button, button_with, icon_button, icon_button_tinted, segmented, slider, stepper, toggle,
};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};

fn c(color: goatty_config::Color) -> Hsla {
    rgb(color.0).into()
}

/// Monospace-looking family names, for the default font list.
pub(super) fn looks_monospace(name: &str) -> bool {
    let n = name.to_lowercase();
    [
        "mono",
        "code",
        "consol",
        "courier",
        "term",
        "iosevka",
        "hack",
        "menlo",
        "fixed",
        "jetbrains",
        "cascadia",
        "inconsolata",
        "victor",
        "commit",
        "monaspace",
        "berkeley",
        "input",
        "fira",
        "source han code",
        "sarasa",
    ]
    .iter()
    .any(|k| n.contains(k))
}

/// Opacity in 5 % steps, between the minimum and opaque.
pub(super) fn snap_opacity(v: f32) -> f32 {
    ((v * 20.0).round() / 20.0).clamp(TerminalSettings::MIN_OPACITY, 1.0)
}

/// A run of preview text in one color.
type Span = (&'static str, Hsla);

impl SettingsWindow {
    pub(super) fn appearance(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(self.terminal_preview(ui, cx))
            .child(self.theme_section(ui, cx))
            .child(self.text_section(ui, window, cx))
            .child(self.cursor_section(ui, cx))
            .child(self.window_section(ui, cx))
            .into_any_element()
    }

    // ---- Preview -----------------------------------------------------------------

    /// A small terminal drawn with the current theme, font, cursor and padding.
    fn terminal_preview(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let app = cx.global::<AppSettings>();
        let s = &app.client;
        let t = &app.colors;
        let a = |i: usize| c(t.ansi[i]);
        let (fg, bg) = (c(t.foreground), c(t.background));
        let font: SharedString = s.font.family.clone().unwrap_or_else(|| "monospace".into()).into();
        let size = s.font.size.min(18.0);
        let line_h = (size * s.font.line_height).round();
        let pad = s.terminal.padding;
        let prompt = |cmd: &'static str| -> Vec<Span> { vec![("~/goatty ", a(4)), ("main ", a(5)), ("❯ ", a(2)), (cmd, fg)] };
        let lines: Vec<Vec<Span>> = vec![
            prompt("ls"),
            vec![("crates/  ", a(12)), ("docs/  ", a(12)), ("Cargo.toml  ", fg), ("README.md", fg)],
            prompt("git status --short"),
            vec![(" M ", a(3)), ("crates/goatty-app/src/main.rs", fg)],
            vec![("?? ", a(1)), ("notes.txt", fg)],
        ];
        let line = |spans: Vec<Span>| {
            div().flex().h(px(line_h)).items_center().children(spans.into_iter().map(|(t, col)| div().text_color(col).child(t)))
        };
        // The last line: a prompt with the cursor after it, in the configured shape.
        let cursor = c(t.cursor);
        let glyph_w = size * 0.6;
        let cursor_el = match s.terminal.cursor_shape {
            CursorShape::Block => div().w(px(glyph_w)).h(px(size * 1.25)).bg(cursor),
            CursorShape::Bar => div().w(px(2.)).h(px(size * 1.25)).bg(cursor),
            CursorShape::Underline => div().w(px(glyph_w)).h(px(size * 1.25)).flex().items_end().child(div().w_full().h(px(2.)).bg(cursor)),
        };
        let last = div()
            .flex()
            .h(px(line_h))
            .items_center()
            .children(prompt("").into_iter().map(|(t, col)| div().text_color(col).child(t)))
            .child(cursor_el);
        let selection = div()
            .flex()
            .h(px(line_h))
            .items_center()
            .child(div().text_color(fg).child("Selected: "))
            .child(div().px(px(2.)).bg(c(t.selection_background)).text_color(c(t.selection_foreground)).child("this text"));
        let opacity = s.terminal.opacity;
        div()
            .rounded(px(radius::CARD))
            .overflow_hidden()
            .border_1()
            .border_color(ui.divider)
            // Checks behind a transparent background show what opacity does.
            .bg(if opacity < 1.0 { ui.field_hover } else { bg })
            .child(
                div()
                    .bg(bg.opacity(opacity))
                    .pt(px(pad.top + 10.))
                    .pr(px(pad.right + 14.))
                    .pb(px(pad.bottom + 10.))
                    .pl(px(pad.left + 14.))
                    .font_family(font)
                    .text_size(px(size))
                    .children(lines.into_iter().map(line))
                    .child(selection)
                    .child(last),
            )
    }

    // ---- Theme -------------------------------------------------------------------

    fn theme_card(&self, ui: Ui, t: ThemeEntry, selected: bool, tags: Vec<&'static str>, cx: &Context<'_, Self>) -> impl IntoElement {
        let col = t.colors;
        let id = t.id.clone();
        let mono = kit::mono(cx);
        div()
            .id(SharedString::from(format!("theme-{}", t.id)))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .overflow_hidden()
            .border_2()
            .border_color(if selected { ui.accent } else { ui.divider })
            .cursor_pointer()
            .hover(|s| s.border_color(if selected { ui.accent } else { ui.border }))
            .child(
                div()
                    .relative()
                    .h(px(52.))
                    .bg(c(col.background))
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .font_family(mono)
                    .text_size(px(11.))
                    .child(
                        div()
                            .flex()
                            .child(div().text_color(c(col.ansi[4])).child("~ "))
                            .child(div().text_color(c(col.ansi[2])).child("❯ "))
                            .child(div().text_color(c(col.foreground)).child("ls")),
                    )
                    .child(div().flex().gap(px(3.)).children(col.ansi[1..7].iter().map(|x| div().size(px(10.)).rounded_full().bg(c(*x)))))
                    .when(selected, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(6.))
                                .right(px(6.))
                                .size(px(18.))
                                .rounded_full()
                                .bg(ui.accent)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(Icon::Check.svg().size(px(12.)).text_color(ui.on_accent)),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .h(px(30.))
                    .px(px(10.))
                    .bg(ui.card)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(text::SMALL))
                            .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                            .text_color(ui.text)
                            .child(SharedString::from(t.name.clone())),
                    )
                    .children(tags.into_iter().map(|tag| badge(tag, Tone::Neutral, ui).h(px(18.)).px(px(6.)))),
            )
            .on_click(cx.listener(move |v, _, _, cx| v.choose_theme(id.clone(), cx)))
    }

    fn theme_section(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let app = cx.global::<AppSettings>();
        let t = app.file.value.theme.clone();
        let target = if t.follow_system {
            if self.theme_target == ThemeTarget::Fixed { ThemeTarget::Dark } else { self.theme_target }
        } else {
            ThemeTarget::Fixed
        };
        let selected_id = match target {
            ThemeTarget::Fixed => t.name.clone(),
            ThemeTarget::Light => t.light.clone(),
            ThemeTarget::Dark => t.dark.clone(),
        };
        let lib = app.themes.clone();
        let name = |id: &str| lib.get(id).map(|e| e.name).unwrap_or_else(|| format!("{id} (missing)"));
        let cards: Vec<_> = lib
            .all()
            .into_iter()
            .map(|e| {
                let mut tags = Vec::new();
                if t.follow_system && e.id == t.light {
                    tags.push("light");
                }
                if t.follow_system && e.id == t.dark {
                    tags.push("dark");
                }
                if e.user {
                    tags.push("mine");
                }
                let sel = e.id == selected_id;
                self.theme_card(ui, e, sel, tags, cx).into_any_element()
            })
            .collect();
        let follow = row(ui, "Follow the system")
            .desc(if t.follow_system {
                format!(
                    "Light: {} · dark: {} · the desktop is {} now",
                    name(&t.light),
                    name(&t.dark),
                    if app.system_dark { "dark" } else { "light" }
                )
            } else {
                "Switch between a light and a dark theme with the desktop".to_string()
            })
            .control(toggle(
                "theme-follow",
                t.follow_system,
                ui,
                self.act(cx, |v, cx| {
                    let follow = !cx.global::<AppSettings>().file.value.theme.follow_system;
                    AppSettings::update(cx, |s| s.theme.follow_system = follow);
                    v.theme_target = if !follow {
                        ThemeTarget::Fixed
                    } else if cx.global::<AppSettings>().system_dark {
                        ThemeTarget::Dark
                    } else {
                        ThemeTarget::Light
                    };
                    v.select_slot(v.edit_slot, cx);
                }),
            ));
        let gallery = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .when(t.follow_system, |d| {
                d.child(div().flex().items_center().justify_between().child(kit::hint(ui, "Pick the theme for")).child(segmented(
                    "theme-target",
                    &[(ThemeTarget::Light, "Light mode"), (ThemeTarget::Dark, "Dark mode")],
                    &target,
                    ui,
                    {
                        let this = cx.entity().downgrade();
                        move |tg, _, cx| {
                            let _ = this.update(cx, |v, cx| {
                                v.theme_target = tg;
                                cx.notify();
                            });
                        }
                    },
                )))
            })
            .child(div().grid().grid_cols(5).gap(px(10.)).children(cards));
        section(ui, "Theme")
            .action(
                div()
                    .flex()
                    .gap(px(4.))
                    .child(button_with(
                        "import-theme",
                        Some(Icon::Download),
                        "Import…",
                        ButtonKind::Ghost,
                        ui,
                        self.act(cx, |v, cx| v.import_theme(cx)),
                    ))
                    .child(icon_button(
                        "themes-folder",
                        Icon::FolderOpen,
                        "Open the themes folder",
                        ui,
                        self.act(cx, |v, cx| v.open_themes_folder(cx)),
                    )),
            )
            .child(follow)
            .child(gallery)
            .child(self.colors_row(ui, cx))
            .when_some_child(self.colors_open.then_some(()), |_| self.color_editor(ui, cx).into_any_element())
            .footer("Import reads base16, iTerm2, Windows Terminal and goatty color schemes.")
    }

    fn colors_row(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let app = cx.global::<AppSettings>();
        let changed = ColorSlot::all().into_iter().filter(|&s| app.file.value.theme.overrides.get(s).is_some()).count();
        let active = app.themes.get(app.file.value.theme.active_id(app.system_dark)).map(|a| a.name).unwrap_or_default();
        let open = self.colors_open;
        div()
            .id("colors-toggle")
            .flex()
            .items_center()
            .gap(px(16.))
            .min_h(px(58.))
            .px(px(16.))
            .cursor_pointer()
            .hover(|s| s.bg(ui.surface_hover))
            .when(!open, |d| d.rounded_b(px(radius::CARD)))
            .child(div().flex().flex_col().gap(px(2.)).flex_1().child(div().text_size(px(text::BODY)).child("Customize colors")).child(
                div().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(match changed {
                    0 => format!("Change single colors of {active}"),
                    1 => format!("1 color changed from {active}"),
                    n => format!("{n} colors changed from {active}"),
                })),
            ))
            .when(changed > 0, |d| d.child(badge(format!("{changed} changed"), Tone::Warning, ui)))
            .child(
                div().flex().gap(px(2.)).children(
                    ColorSlot::all()
                        .into_iter()
                        .take(5)
                        .map(|s| div().size(px(14.)).rounded_full().border_1().border_color(ui.border).bg(c(app.colors.get(s)))),
                ),
            )
            .child((if open { Icon::ChevronDown } else { Icon::ChevronRight }).svg().size(px(16.)).text_color(ui.faint))
            .on_click(cx.listener(|v, _, _, cx| {
                v.colors_open = !v.colors_open;
                cx.notify();
            }))
    }

    /// Swatches, the picker and the theme actions.
    fn color_editor(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let app = cx.global::<AppSettings>();
        let t = &app.file.value.theme;
        let overrides = t.overrides.clone();
        let edit = self.edit_slot;
        let active = app.themes.get(t.active_id(app.system_dark));
        let colors: ThemeColors = app.colors;
        let swatch = |slot: ColorSlot, big: bool| {
            let color = colors.get(slot);
            let changed = overrides.get(slot).is_some();
            let on = slot == edit;
            div()
                .id(SharedString::from(format!("swatch-{}", slot.label())))
                .flex()
                .flex_col()
                .gap(px(4.))
                .items_center()
                .cursor_pointer()
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(if big { 34. } else { 26. }))
                        .rounded(px(8.))
                        .border_2()
                        .border_color(if on { ui.accent } else { ui.divider })
                        .bg(c(color))
                        .when(changed, |d| {
                            d.child(
                                div()
                                    .absolute()
                                    .top(px(-3.))
                                    .right(px(-3.))
                                    .size(px(9.))
                                    .rounded_full()
                                    .border_2()
                                    .border_color(ui.card)
                                    .bg(ui.warning),
                            )
                        }),
                )
                .when(big, |d| {
                    d.child(
                        div()
                            .text_size(px(text::CAPTION))
                            .text_color(if on { ui.accent_text } else { ui.muted })
                            .child(SharedString::from(slot.label())),
                    )
                })
                .tooltip(crate::ui::controls::tooltip(slot.label()))
                .on_click(cx.listener(move |v, _, _, cx| v.select_slot(slot, cx)))
        };
        let slots = ColorSlot::all();
        let caption = |t: &'static str| div().w(px(52.)).flex_shrink_0().text_size(px(text::CAPTION)).text_color(ui.muted).child(t);
        let swatches = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(div().grid().grid_cols(5).gap(px(8.)).children(slots[..5].iter().map(|&s| swatch(s, true))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(caption("Normal"))
                    .child(div().flex_1().grid().grid_cols(8).gap(px(6.)).children(slots[5..13].iter().map(|&s| swatch(s, false)))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(caption("Bright"))
                    .child(div().flex_1().grid().grid_cols(8).gap(px(6.)).children(slots[13..].iter().map(|&s| swatch(s, false)))),
            );

        let mut actions = div().flex().flex_wrap().items_center().gap(px(8.));
        if overrides.get(edit).is_some() {
            actions = actions.child(button_with(
                "revert-color",
                Some(Icon::RotateCcw),
                format!("Revert {}", edit.label().to_lowercase()),
                ButtonKind::Ghost,
                ui,
                self.act(cx, |v, cx| v.revert_color(cx)),
            ));
        }
        if !overrides.is_empty() {
            actions =
                actions.child(button("reset-colors", "Reset all colors", ButtonKind::Ghost, ui, self.act(cx, |v, cx| v.reset_colors(cx))));
            if let Some(a) = active.clone().filter(|a| a.user) {
                let id = a.id.clone();
                actions = actions.child(button(
                    "update-theme",
                    SharedString::from(format!("Save into {}", a.name)),
                    ButtonKind::Primary,
                    ui,
                    self.act(cx, move |v, cx| v.update_theme(id.clone(), cx)),
                ));
            }
        }
        actions = actions.child(div().flex_1());
        if let Some(a) = active.filter(|a| a.user) {
            let id = a.id.clone();
            actions = actions.child(button_with(
                "delete-theme",
                Some(Icon::Trash),
                SharedString::from(format!("Delete {}", a.name)),
                ButtonKind::Danger,
                ui,
                self.act(cx, move |v, cx| v.delete_theme(id.clone(), cx)),
            ));
        }
        let save = div().flex().items_center().gap(px(8.)).child(div().flex_1().child(self.theme_name.clone())).child(button_with(
            "save-theme",
            Some(Icon::Plus),
            "Save as theme",
            ButtonKind::Normal,
            ui,
            self.act(cx, |v, cx| v.save_theme_as(cx)),
        ));
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .px(px(16.))
            .pt(px(4.))
            .pb(px(16.))
            .child(swatches)
            .child(self.picker.clone())
            .child(actions)
            .child(save)
    }

    // ---- Text --------------------------------------------------------------------

    fn font_picker(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        let app = cx.global::<AppSettings>();
        let current = app.file.value.font.family.clone();
        let resolved = app.client.font.family.clone().unwrap_or_default();
        let id: SharedString = "font".into();
        let open = self.menu.as_ref() == Some(&id);
        let label = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .max_w(px(220.))
                    .truncate()
                    .font_family(SharedString::from(resolved.clone()))
                    .child(SharedString::from(resolved.clone())),
            )
            .when(current.is_none(), |d| d.child(badge("Auto", Tone::Neutral, ui).h(px(18.)).px(px(6.))));
        let filter = self.font_filter.clone();
        let trigger = select_trigger(ui, id.clone(), label, open).on_mouse_down(MouseButton::Left, move |_, window, cx| {
            window.focus(&filter.focus_handle(cx), cx);
        });
        let mut list =
            div().flex().flex_col().w(px(320.)).p(px(5.)).rounded(px(14.)).bg(ui.menu).border_1().border_color(ui.border).shadow_lg();
        if open {
            let query = self.font_filter.read(cx).text().to_lowercase();
            let show_all = self.show_all_fonts;
            let fonts: Vec<String> = app
                .fonts
                .iter()
                .filter(|f| show_all || looks_monospace(f))
                .filter(|f| query.is_empty() || f.to_lowercase().contains(&query))
                .take(300)
                .cloned()
                .collect();
            let pick = |value: Option<String>| {
                cx.listener(move |v, _, _, cx| {
                    let value = value.clone();
                    AppSettings::update(cx, |s| s.font.family = value);
                    v.menu = None;
                    cx.notify();
                })
            };
            list = list
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .p(px(6.))
                        .pb(px(8.))
                        .child(search_field(ui, "font-search", &self.font_filter, window, cx))
                        .child(
                            div()
                                .flex()
                                .gap(px(6.))
                                .child(chip(
                                    ui,
                                    "fonts-mono",
                                    "Monospace",
                                    None,
                                    !show_all,
                                    self.act(cx, |v, cx| {
                                        v.show_all_fonts = false;
                                        cx.notify();
                                    }),
                                ))
                                .child(chip(
                                    ui,
                                    "fonts-all",
                                    "All fonts",
                                    None,
                                    show_all,
                                    self.act(cx, |v, cx| {
                                        v.show_all_fonts = true;
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .child(
                    menu(ui, "font-list")
                        .p_0()
                        .border_0()
                        .shadow_none()
                        .bg(ui.menu)
                        .min_w_0()
                        .max_h(px(320.))
                        .child(
                            menu_item(
                                ui,
                                "font-auto",
                                "Automatic".into(),
                                Some("The first installed of goatty's list".into()),
                                current.is_none(),
                            )
                            .on_click(pick(None)),
                        )
                        .children(fonts.into_iter().enumerate().map(|(i, f)| {
                            let selected = current.as_deref() == Some(f.as_str());
                            menu_item(ui, ("font", i), f.clone().into(), None, selected).on_click(pick(Some(f)))
                        })),
                );
        }
        self.dropdown(id, trigger, list, cx)
    }

    fn text_section(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> impl IntoElement {
        let s = &cx.global::<AppSettings>().file.value;
        let (size, lh) = (s.font.size, s.font.line_height);
        section(ui, "Text")
            .child(row(ui, "Font").desc("Monospace fonts keep columns lined up").control(self.font_picker(ui, window, cx)))
            .child(row(ui, "Size").control(stepper(
                "font-size",
                format!("{size:.0} px"),
                ui,
                client(|s| s.font.size = (s.font.size - 1.0).max(FontSettings::MIN_SIZE)),
                client(|s| s.font.size = (s.font.size + 1.0).min(FontSettings::MAX_SIZE)),
            )))
            .child(row(ui, "Line height").desc("Space between lines, as a multiple of the font size").control(stepper(
                "line-height",
                format!("{lh:.2}×"),
                ui,
                client(|s| s.font.line_height = ((s.font.line_height - 0.05) * 100.0).round() / 100.0),
                client(|s| s.font.line_height = ((s.font.line_height + 0.05) * 100.0).round() / 100.0),
            )))
            .child(
                row(ui, "Ligatures")
                    .desc("Draw => or != as one symbol in fonts that have them. A little slower on full-screen output")
                    .control(toggle(
                        "ligatures",
                        s.terminal.renderer == Renderer::Shaped,
                        ui,
                        client(|s| {
                            s.terminal.renderer = if s.terminal.renderer == Renderer::Shaped { Renderer::Glyphs } else { Renderer::Shaped }
                        }),
                    )),
            )
            .child(row(ui, "Bold text in bright colors").desc("How xterm draws bold; off keeps the original color").control(toggle(
                "bold-bright",
                s.theme.bold_is_bright,
                ui,
                client(|s| s.theme.bold_is_bright = !s.theme.bold_is_bright),
            )))
    }

    // ---- Cursor ------------------------------------------------------------------

    fn cursor_section(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let app = cx.global::<AppSettings>();
        let s = &app.file.value;
        let t = &app.colors;
        let mono = kit::mono(cx);
        let card = |shape: CursorShape, label: &'static str| {
            let on = s.terminal.cursor_shape == shape;
            let (fg, bg, cur) = (c(t.foreground), c(t.background), c(t.cursor));
            let cell = |ch: &'static str| div().w(px(11.)).flex().justify_center().child(ch);
            let cursor_cell = match shape {
                CursorShape::Block => div().w(px(11.)).h(px(20.)).bg(cur).text_color(bg).flex().items_center().justify_center().child("t"),
                CursorShape::Bar => div()
                    .relative()
                    .w(px(11.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("t")
                    .child(div().absolute().left_0().top(px(1.)).w(px(2.)).h(px(18.)).bg(cur)),
                CursorShape::Underline => div()
                    .relative()
                    .w(px(11.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child("t")
                    .child(div().absolute().left_0().bottom(px(0.)).w_full().h(px(2.)).bg(cur)),
            };
            kit::choice_card(
                ui,
                label,
                label,
                on,
                div()
                    .h(px(52.))
                    .bg(bg)
                    .text_color(fg)
                    .font_family(mono.clone())
                    .text_size(px(15.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(cell("c"))
                    .child(cell("a"))
                    .child(cursor_cell),
            )
            .on_click(move |_, _, cx| AppSettings::update(cx, |s| s.terminal.cursor_shape = shape))
        };
        section(ui, "Cursor")
            .child(
                row(ui, "Shape").desc("Programs like vim can still change it while they run").below(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(card(CursorShape::Block, "Block"))
                        .child(card(CursorShape::Bar, "Bar"))
                        .child(card(CursorShape::Underline, "Underline")),
                ),
            )
            .child(row(ui, "Blinking").control(toggle(
                "blink",
                s.terminal.cursor_blink,
                ui,
                client(|s| s.terminal.cursor_blink = !s.terminal.cursor_blink),
            )))
    }

    // ---- Window ------------------------------------------------------------------

    fn padding_row(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let pad = cx.global::<AppSettings>().file.value.terminal.padding;
        let max = TerminalSettings::MAX_PADDING;
        let linked = self.padding_linked;
        let side_stepper = |i: usize, label: &'static str| {
            div().flex().flex_col().gap(px(6.)).child(div().text_size(px(text::CAPTION)).text_color(ui.muted).child(label)).child(stepper(
                &format!("pad-{i}"),
                format!("{:.0} px", pad.sides()[i]),
                ui,
                client(move |s| {
                    let v = s.terminal.padding.side_mut(i);
                    *v = (*v - 1.0).max(0.0);
                }),
                client(move |s| {
                    let v = s.terminal.padding.side_mut(i);
                    *v = (*v + 1.0).min(max);
                }),
            ))
        };
        let link = icon_button_tinted(
            "pad-link",
            if linked { Icon::Link } else { Icon::Unlink },
            if linked { "Set each side on its own" } else { "Use the same padding on all sides" },
            linked.then_some(ui.accent_text),
            ui,
            self.act(cx, move |v, cx| {
                v.padding_linked = !v.padding_linked;
                if v.padding_linked {
                    AppSettings::update(cx, |s| s.terminal.padding = goatty_config::Padding::all(s.terminal.padding.top));
                }
                cx.notify();
            }),
        );
        let control = div().flex().items_center().gap(px(8.)).child(link).when(linked, |d| {
            d.child(stepper(
                "padding",
                format!("{:.0} px", pad.top),
                ui,
                client(|s| s.terminal.padding = goatty_config::Padding::all((s.terminal.padding.top - 1.0).max(0.0))),
                client(move |s| s.terminal.padding = goatty_config::Padding::all((s.terminal.padding.top + 1.0).min(max))),
            ))
        });
        let r = row(ui, "Padding").desc("Space between the text and the window's edges").control(control);
        if linked {
            r
        } else {
            r.below(
                div()
                    .grid()
                    .grid_cols(4)
                    .gap(px(10.))
                    .child(side_stepper(0, "Top"))
                    .child(side_stepper(1, "Right"))
                    .child(side_stepper(2, "Bottom"))
                    .child(side_stepper(3, "Left")),
            )
        }
    }

    fn window_section(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let s = &cx.global::<AppSettings>().file.value;
        let opacity = s.terminal.opacity;
        let min = TerminalSettings::MIN_OPACITY;
        let opacity_control = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(slider("opacity", (opacity - min) / (1.0 - min), ui, move |t, done, _, cx| {
                let v = snap_opacity(min + t * (1.0 - min));
                if done {
                    AppSettings::update(cx, |s| s.terminal.opacity = v);
                } else if cx.global::<AppSettings>().file.value.terminal.opacity != v {
                    AppSettings::preview(cx, |s| s.terminal.opacity = v);
                }
            }))
            .child(
                div()
                    .w(px(40.))
                    .text_size(px(text::LABEL))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ui.text)
                    .child(SharedString::from(format!("{:.0} %", opacity * 100.0))),
            );
        section(ui, "Window")
            .child(self.padding_row(ui, cx))
            .child(row(ui, "Background opacity").desc("Text and colored cells stay opaque. Needs a compositor").control(opacity_control))
            .child(
                row(ui, "Blur behind the window")
                    .desc(if s.terminal.is_transparent() {
                        "Where the desktop supports it (KDE Plasma, niri, Hyprland, macOS, Windows)"
                    } else {
                        "Lower the background opacity to see through the window first"
                    })
                    .disabled(!s.terminal.is_transparent())
                    .control(toggle("blur", s.terminal.blur, ui, client(|s| s.terminal.blur = !s.terminal.blur))),
            )
    }
}

/// For `select`s on other pages that list fixed choices.
pub(super) fn opts<T: Clone>(items: &[(T, &'static str, &'static str)]) -> Vec<Opt<T>> {
    items.iter().map(|(v, l, d)| if d.is_empty() { opt(v.clone(), *l) } else { opt(v.clone(), *l).desc(*d) }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monospace_guess() {
        assert!(looks_monospace("JetBrains Mono"));
        assert!(looks_monospace("Fira Code"));
        assert!(!looks_monospace("Noto Sans"));
    }

    #[test]
    fn opacity_snaps_to_five_percent() {
        assert_eq!(snap_opacity(1.0), 1.0);
        assert_eq!(snap_opacity(0.87), 0.85);
        assert_eq!(snap_opacity(0.0), TerminalSettings::MIN_OPACITY);
        assert_eq!(snap_opacity(0.951), 0.95);
    }
}
