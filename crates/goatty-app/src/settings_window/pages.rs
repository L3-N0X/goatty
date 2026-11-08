//! Rendering of the settings window: sidebar, pages, the host-changes bar.

use goatty_config::keys::{self, ACTIONS};
use goatty_config::{ClientSettings, ColorSlot, CursorShape, HostSettings, OnLaunch, PromptStyle, Renderer, ThemeEntry, ThemeSettings};
use gpui::{
    AnyElement, App, Context, FontWeight, IntoElement, KeyDownEvent, ParentElement, Render, SharedString, Styled, Window, div, prelude::*,
    px, rgb,
};

use super::{Page, SettingsWindow, ThemeTarget};
use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, button, note, notice, row, section, segmented, stepper, toggle};
use crate::ui::style::Ui;

/// Callback editing the client settings (applied and saved immediately).
fn client(f: impl Fn(&mut ClientSettings) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
    move |_, cx| AppSettings::update(cx, &f)
}

/// Adapt a control callback to `on_click`.
fn click(f: impl Fn(&mut Window, &mut App) + 'static) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    move |_, window, cx| f(window, cx)
}

/// Monospace-looking family names, for the default font list.
fn looks_monospace(name: &str) -> bool {
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

/// `ctrl-shift-c` → `Ctrl+Shift+C`.
fn pretty_keys(k: &str) -> String {
    let parts: Vec<&str> = match k.strip_suffix("--") {
        Some(rest) => {
            let mut p: Vec<&str> = rest.split('-').collect();
            p.push("-");
            p
        }
        None => k.split('-').collect(),
    };
    parts
        .iter()
        .map(|p| match *p {
            "ctrl" => "Ctrl".to_string(),
            "shift" => "Shift".to_string(),
            "alt" => "Alt".to_string(),
            "super" | "cmd" | "platform" => "Super".to_string(),
            "pageup" => "PageUp".to_string(),
            "pagedown" => "PageDown".to_string(),
            other if other.len() == 1 => other.to_uppercase(),
            other => {
                let mut c = other.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
}

fn ago(created_unix: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    match now.saturating_sub(created_unix) {
        s @ 0..60 => format!("{s} s ago"),
        s @ 60..3600 => format!("{} min ago", s / 60),
        s @ 3600..86400 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
}

impl SettingsWindow {
    /// Callback editing the draft host settings.
    fn host(&self, cx: &Context<'_, Self>, f: impl Fn(&mut HostSettings) + 'static) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            let _ = this.update(cx, |view, cx| view.edit_host(cx, &f));
        }
    }

    /// Callback acting on the window itself.
    fn act(
        &self,
        cx: &Context<'_, Self>,
        f: impl Fn(&mut Self, &mut Context<'_, Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            let _ = this.update(cx, |view, cx| f(view, cx));
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(action) = self.recording else { return };
        let k = &ev.keystroke;
        if k.key == "escape" && !k.modifiers.modified() {
            self.recording = None;
            cx.notify();
            return;
        }
        let mut parts = Vec::new();
        if k.modifiers.control {
            parts.push("ctrl");
        }
        if k.modifiers.alt {
            parts.push("alt");
        }
        if k.modifiers.shift {
            parts.push("shift");
        }
        if k.modifiers.platform {
            parts.push("super");
        }
        parts.push(k.key.as_str());
        let keystroke = parts.join("-");
        self.recording = None;
        AppSettings::update(cx, |s| keys::set_binding(&mut s.keybindings, &keystroke, Some(action)));
        cx.stop_propagation();
        cx.notify();
    }

    fn sidebar(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        div()
            .w(px(180.))
            .flex_shrink_0()
            .h_full()
            .bg(ui.surface)
            .border_r_1()
            .border_color(ui.border)
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .child(div().px_2().py_3().text_lg().font_weight(FontWeight::BOLD).child("Settings"))
            .children(Page::ALL.iter().map(|&page| {
                let on = page == self.page;
                let changed = page.is_host() && self.draft.is_some();
                div()
                    .id(page.title())
                    .px_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .flex()
                    .justify_between()
                    .when(on, |d| d.bg(ui.accent).text_color(ui.on_accent))
                    .when(!on, |d| d.hover(|s| s.bg(ui.surface_hover)))
                    .child(page.title())
                    .when(changed, |d| d.child(div().text_color(if on { ui.on_accent } else { ui.warning }).child("●")))
                    .on_click(click(self.act(cx, move |v, cx| {
                        v.page = page;
                        cx.notify();
                    })))
            }))
            .child(div().flex_1())
            .child(note(ui, "Ctrl+, or Ctrl+W closes this window"))
    }

    // ---- Appearance ---------------------------------------------------------

    fn theme_card(&self, ui: Ui, t: ThemeEntry, selected: bool, badges: Vec<&'static str>, cx: &Context<'_, Self>) -> impl IntoElement {
        let c = t.colors;
        let id = t.id.clone();
        div()
            .id(SharedString::from(format!("theme-{}", t.id)))
            .w(px(146.))
            .rounded_lg()
            .border_2()
            .border_color(if selected { ui.accent } else { ui.border })
            .overflow_hidden()
            .cursor_pointer()
            .child(
                div()
                    .bg(rgb(c.background.0))
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .gap_1()
                            .child(div().text_xs().text_color(rgb(c.foreground.0)).truncate().child(SharedString::from(t.name.clone())))
                            .children(badges.into_iter().map(|b| {
                                div().text_xs().px_1().rounded_sm().bg(rgb(c.foreground.0)).text_color(rgb(c.background.0)).child(b)
                            })),
                    )
                    .child(
                        div().flex().gap(px(2.)).children(c.ansi[1..7].iter().map(|col| div().size(px(14.)).rounded_sm().bg(rgb(col.0)))),
                    ),
            )
            .on_click(click(self.act(cx, move |v, cx| v.choose_theme(id.clone(), cx))))
    }

    /// Theme mode, gallery, and the color editor.
    fn themes_section(&self, ui: Ui, cx: &Context<'_, Self>) -> gpui::Div {
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
        let active_id = t.active_id(app.system_dark).to_string();
        let lib = app.themes.clone();
        let active = lib.get(&active_id);

        let mut d = div().flex().flex_col().child(section(ui, "Theme")).child(row(
            ui,
            "Theme",
            Some(if t.follow_system { "Switches with your desktop's light/dark setting" } else { "Always the same theme" }),
            segmented("theme-mode", &[(false, "One theme"), (true, "Follow the system")], &t.follow_system, ui, {
                let this = cx.entity().downgrade();
                move |follow, _, cx| {
                    let _ = this.update(cx, |v, cx| {
                        AppSettings::update(cx, |s| s.theme.follow_system = follow);
                        v.theme_target = if follow {
                            if cx.global::<AppSettings>().system_dark { ThemeTarget::Dark } else { ThemeTarget::Light }
                        } else {
                            ThemeTarget::Fixed
                        };
                        v.select_slot(v.edit_slot, cx);
                    });
                }
            }),
        ));
        if t.follow_system {
            let name = |id: &str| lib.get(id).map(|e| e.name).unwrap_or_else(|| format!("{id} (missing)"));
            d = d.child(row(
                ui,
                "Pick a theme for",
                Some(&format!(
                    "Light: {} · Dark: {} · the desktop is {} now",
                    name(&t.light),
                    name(&t.dark),
                    if app.system_dark { "dark" } else { "light" }
                )),
                segmented("theme-target", &[(ThemeTarget::Light, "Light"), (ThemeTarget::Dark, "Dark")], &target, ui, {
                    let this = cx.entity().downgrade();
                    move |tg, _, cx| {
                        let _ = this.update(cx, |v, cx| {
                            v.theme_target = tg;
                            cx.notify();
                        });
                    }
                }),
            ));
        }
        let cards: Vec<_> = lib
            .all()
            .into_iter()
            .map(|e| {
                let mut badges = Vec::new();
                if t.follow_system && e.id == t.light {
                    badges.push("light");
                }
                if t.follow_system && e.id == t.dark {
                    badges.push("dark");
                }
                if e.user {
                    badges.push("mine");
                }
                let sel = e.id == selected_id;
                self.theme_card(ui, e, sel, badges, cx)
            })
            .collect();
        d = d.child(div().flex().flex_wrap().gap_2().children(cards)).child(
            div()
                .flex()
                .gap_2()
                .py_2()
                .child(button("import-theme", "Import theme…", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.import_theme(cx))))
                .child(button(
                    "themes-folder",
                    "Open themes folder",
                    ButtonKind::Normal,
                    ui,
                    self.act(cx, |v, cx| v.open_themes_folder(cx)),
                )),
        );
        d = d.child(note(ui, "Import reads base16 (YAML/JSON), iTerm2 .itermcolors, Windows Terminal schemes and goatty themes."));

        // ---- Editor -----------------------------------------------------------
        let overrides = t.overrides.clone();
        let edit = self.edit_slot;
        let swatch = |slot: ColorSlot, i: usize| {
            let color = app.colors.get(slot);
            let changed = overrides.get(slot).is_some();
            let on = slot == edit;
            div()
                .id(("swatch", i))
                .w(px(84.))
                .flex()
                .flex_col()
                .gap(px(2.))
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(if on { ui.accent } else { ui.border.opacity(0.0) })
                .cursor_pointer()
                .hover(|s| s.bg(ui.surface_hover))
                .child(div().h(px(22.)).rounded_sm().border_1().border_color(ui.border).bg(rgb(color.0)))
                .child(
                    div()
                        .flex()
                        .justify_between()
                        .text_xs()
                        .text_color(ui.muted)
                        .child(SharedString::from(slot.label()))
                        .when(changed, |d| d.child(div().text_color(ui.warning).child("●"))),
                )
                .on_click(click(self.act(cx, move |v, cx| v.select_slot(slot, cx))))
        };
        let slots = ColorSlot::all();
        d = d
            .child(section(ui, "Colors"))
            .child(note(
                ui,
                match &active {
                    Some(a) => format!("Changes apply on top of {} (● = changed). Save them as a theme to keep them apart.", a.name),
                    None => "Changes apply on top of the active theme (● = changed).".into(),
                },
            ))
            .child(div().flex().flex_wrap().gap_1().children(slots.iter().enumerate().map(|(i, &sl)| swatch(sl, i))))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .py_2()
                    .child(div().text_sm().child(SharedString::from(format!("Editing {}", edit.label().to_lowercase()))))
                    .when(overrides.get(edit).is_some(), |d| {
                        d.child(button("revert-color", "Revert to theme", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.revert_color(cx))))
                    }),
            )
            .child(self.picker.clone());
        let has_overrides = !overrides.is_empty();
        let mut actions = div().flex().flex_wrap().items_center().gap_2().py_1();
        if has_overrides {
            actions =
                actions.child(button("reset-colors", "Reset all colors", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.reset_colors(cx))));
            if let Some(a) = active.clone().filter(|a| a.user) {
                let id = a.id.clone();
                actions = actions.child(button(
                    "update-theme",
                    SharedString::from(format!("Save into {:?}", a.name)),
                    ButtonKind::Primary,
                    ui,
                    self.act(cx, move |v, cx| v.update_theme(id.clone(), cx)),
                ));
            }
        }
        actions = actions.child(div().w(px(200.)).child(self.theme_name.clone())).child(button(
            "save-theme",
            "Save as new theme",
            ButtonKind::Normal,
            ui,
            self.act(cx, |v, cx| v.save_theme_as(cx)),
        ));
        if let Some(a) = active.filter(|a| a.user) {
            let id = a.id.clone();
            actions = actions.child(button(
                "delete-theme",
                SharedString::from(format!("Delete {:?}", a.name)),
                ButtonKind::Danger,
                ui,
                self.act(cx, move |v, cx| v.delete_theme(id.clone(), cx)),
            ));
        }
        d.child(actions)
    }

    fn appearance(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let app = cx.global::<AppSettings>();
        let s = &app.file.value;
        let size = s.font.size;
        let lh = s.font.line_height;
        let pad = s.terminal.padding;
        let filter = self.font_filter.read(cx).text().to_lowercase();
        let show_all = self.show_all_fonts;
        let fonts: Vec<String> = app
            .fonts
            .iter()
            .filter(|f| show_all || looks_monospace(f))
            .filter(|f| filter.is_empty() || f.to_lowercase().contains(&filter))
            .take(300)
            .cloned()
            .collect();
        let current_family = s.font.family.clone();
        let resolved = app.client.font.family.clone().unwrap_or_default();

        let font_row = |label: String, value: Option<String>, selected: bool, i: usize| {
            div()
                .id(("font", i))
                .px_2()
                .py(px(3.))
                .rounded_sm()
                .cursor_pointer()
                .when(selected, |d| d.bg(ui.accent).text_color(ui.on_accent))
                .when(!selected, |d| d.hover(|st| st.bg(ui.surface_hover)))
                .child(SharedString::from(label))
                .on_click(click(client(move |st| st.font.family = value.clone())))
        };

        div()
            .flex()
            .flex_col()
            .child(self.themes_section(ui, cx))
            .child(row(
                ui,
                "Bold text in bright colors",
                Some("How xterm shows bold; off keeps the original color"),
                toggle("bold-bright", s.theme.bold_is_bright, ui, client(|s| s.theme.bold_is_bright = !s.theme.bold_is_bright)),
            ))
            .child(section(ui, "Text"))
            .child(row(
                ui,
                "Size",
                None,
                stepper(
                    "font-size",
                    format!("{size:.0} px"),
                    ui,
                    client(|s| s.font.size = (s.font.size - 1.0).max(goatty_config::FontSettings::MIN_SIZE)),
                    client(|s| s.font.size = (s.font.size + 1.0).min(goatty_config::FontSettings::MAX_SIZE)),
                ),
            ))
            .child(row(
                ui,
                "Line height",
                None,
                stepper(
                    "line-height",
                    format!("{lh:.2}×"),
                    ui,
                    client(|s| s.font.line_height = ((s.font.line_height - 0.05) * 100.0).round() / 100.0),
                    client(|s| s.font.line_height = ((s.font.line_height + 0.05) * 100.0).round() / 100.0),
                ),
            ))
            .child(row(
                ui,
                "Rendering",
                Some("Ligatures shape whole runs of text; a little slower when much of the screen changes"),
                segmented(
                    "renderer",
                    &[(Renderer::Glyphs, "Fast"), (Renderer::Shaped, "Ligatures")],
                    &s.terminal.renderer,
                    ui,
                    |r, _, cx| AppSettings::update(cx, |s| s.terminal.renderer = r),
                ),
            ))
            .child(row(
                ui,
                "Font",
                Some(&format!("Using {resolved}")),
                toggle(
                    "all-fonts",
                    show_all,
                    ui,
                    self.act(cx, |v, cx| {
                        v.show_all_fonts = !v.show_all_fonts;
                        cx.notify();
                    }),
                )
                .into_any_element(),
            ))
            .child(note(
                ui,
                if show_all {
                    "Showing all fonts (switch off for monospace-looking ones only)"
                } else {
                    "Showing monospace-looking fonts (switch on for all)"
                },
            ))
            .child(div().py_1().child(self.font_filter.clone()))
            .child(
                div()
                    .id("font-list")
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .rounded_md()
                    .border_1()
                    .border_color(ui.border)
                    .p_1()
                    .child(font_row("Automatic (first installed of goatty's list)".into(), None, current_family.is_none(), 0))
                    .children(fonts.into_iter().enumerate().map(|(i, f)| {
                        let selected = current_family.as_deref() == Some(f.as_str());
                        font_row(f.clone(), Some(f), selected, i + 1)
                    })),
            )
            .child(section(ui, "Window and cursor"))
            .child(row(
                ui,
                "Padding",
                Some("Space around the text"),
                stepper(
                    "padding",
                    format!("{pad:.0} px"),
                    ui,
                    client(|s| s.terminal.padding = (s.terminal.padding - 1.0).max(0.0)),
                    client(|s| s.terminal.padding = (s.terminal.padding + 1.0).min(goatty_config::TerminalSettings::MAX_PADDING)),
                ),
            ))
            .child(row(
                ui,
                "Cursor",
                Some("Programs like vim can still change it"),
                segmented(
                    "cursor",
                    &[(CursorShape::Block, "Block"), (CursorShape::Bar, "Bar"), (CursorShape::Underline, "Underline")],
                    &s.terminal.cursor_shape,
                    ui,
                    |c, _, cx| AppSettings::update(cx, |s| s.terminal.cursor_shape = c),
                ),
            ))
            .child(row(
                ui,
                "Blinking cursor",
                None,
                toggle("blink", s.terminal.cursor_blink, ui, client(|s| s.terminal.cursor_blink = !s.terminal.cursor_blink)),
            ))
            .into_any_element()
    }

    // ---- Host pages -----------------------------------------------------------

    /// Header of host pages: which host, problems, overrides. `None` when
    /// the settings aren't available (the error is shown instead).
    fn host_header(&self, ui: Ui) -> (AnyElement, bool) {
        let label = self.link.as_ref().map(|l| l.label()).unwrap_or_else(|| "no host".into());
        let mut d = div().flex().flex_col().child(note(
            ui,
            format!(
                "These settings belong to {label} and apply to new sessions there. Changes are collected; preview and apply them below."
            ),
        ));
        if let Some(e) = &self.host_error {
            return (d.child(notice(ui, ui.danger, vec![format!("Couldn't load the host's settings: {e}")])).into_any_element(), false);
        }
        let Some(h) = &self.host else { return (d.child(note(ui, "Loading…")).into_any_element(), false) };
        if !h.issues.is_empty() {
            d = d.child(notice(ui, ui.warning, h.issues.iter().map(|i| format!("⚠ {i}")).collect()));
        }
        if !h.env_overrides.is_empty() {
            d = d.child(note(ui, format!("Overridden on the host by {}", h.env_overrides.join(", "))));
        }
        if let Some(g) = h.generation {
            d = d.child(note(ui, format!("Active shell configuration: generation {g} (see Profile to roll back)")));
        }
        (d.into_any_element(), true)
    }

    fn shell(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let (header, ok) = self.host_header(ui);
        let mut page = div().flex().flex_col().child(header);
        let Some(h) = self.host_value().filter(|_| ok) else { return page.into_any_element() };
        let program = h.shell.program.clone();
        let choice = match program.as_deref() {
            None => "auto",
            Some("zsh") => "zsh",
            Some("bash") => "bash",
            Some("fish") => "fish",
            Some(_) => "custom",
        };
        let z = &h.shell.zsh;
        let managed = h.shell.managed;
        page = page
            .child(section(ui, "Shell"))
            .child(row(
                ui,
                "Program",
                Some("Automatic: zsh if installed, else your login shell"),
                segmented(
                    "program",
                    &[("auto", "Automatic"), ("zsh", "zsh"), ("bash", "bash"), ("fish", "fish"), ("custom", "Other…")],
                    &choice,
                    ui,
                    {
                        let this = cx.entity().downgrade();
                        move |c: &str, _, cx| {
                            let _ = this.update(cx, |v, cx| {
                                v.edit_host(cx, |h| {
                                    h.shell.program = match c {
                                        "auto" => None,
                                        "custom" => h
                                            .shell
                                            .program
                                            .clone()
                                            .filter(|p| !["zsh", "bash", "fish"].contains(&p.as_str()))
                                            .or(Some("/bin/sh".into())),
                                        name => Some(name.into()),
                                    }
                                })
                            });
                        }
                    },
                ),
            ))
            .when(choice == "custom", |d| d.child(div().py_1().child(self.shell_path.clone())))
            .child(row(
                ui,
                "goatty's managed zsh",
                Some("Plugins, prompt, history and shell integration without touching your dotfiles. Off: plain zsh with your own config"),
                toggle("managed", managed, ui, self.host(cx, |h| h.shell.managed = !h.shell.managed)),
            ))
            .child(section(ui, "Your dotfiles"))
            .child(row(
                ui,
                "Use ~/.zshenv and ~/.zprofile",
                Some("Your environment: PATH, EDITOR, …"),
                toggle("user-env", z.user_env, ui, self.host(cx, |h| h.shell.zsh.user_env = !h.shell.zsh.user_env)),
            ))
            .child(row(
                ui,
                "Also use ~/.zshrc",
                Some("Most .zshrc files load their own prompt and plugins, which can fight goatty's"),
                toggle("user-rc", z.user_rc, ui, self.host(cx, |h| h.shell.zsh.user_rc = !h.shell.zsh.user_rc)),
            ))
            .child(note(ui, "Your own additions for goatty's zsh go in ~/.config/goatty/zshrc."))
            .child(section(ui, "Features"))
            .child(row(
                ui,
                "Autosuggestions",
                Some("Grey suggestions from history while you type; → accepts"),
                toggle("autosuggest", z.autosuggestions, ui, self.host(cx, |h| h.shell.zsh.autosuggestions = !h.shell.zsh.autosuggestions)),
            ))
            .child(row(
                ui,
                "Syntax highlighting",
                Some("Colors commands, strings and errors as you type"),
                toggle(
                    "syntax",
                    z.syntax_highlighting,
                    ui,
                    self.host(cx, |h| h.shell.zsh.syntax_highlighting = !h.shell.zsh.syntax_highlighting),
                ),
            ))
            .child(row(
                ui,
                "fzf key bindings",
                Some("Ctrl+T picks files, Alt+C changes directory (when fzf is installed)"),
                toggle("fzf", z.fzf, ui, self.host(cx, |h| h.shell.zsh.fzf = !h.shell.zsh.fzf)),
            ))
            .child(row(
                ui,
                "Shell integration",
                Some("Marks prompts and commands (OSC 133) and reports the directory (OSC 7)"),
                toggle("integration", z.integration, ui, self.host(cx, |h| h.shell.zsh.integration = !h.shell.zsh.integration)),
            ));
        if !managed {
            page = page.child(note(ui, "The features above apply only to goatty's managed zsh."));
        }
        page.into_any_element()
    }

    fn prompt(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let (header, ok) = self.host_header(ui);
        let page = div().flex().flex_col().child(header);
        let Some(h) = self.host_value().filter(|_| ok) else { return page.into_any_element() };
        let p = h.prompt.clone();
        let secs = p.duration_threshold_ms / 1000;
        page.child(section(ui, "Prompt"))
            .child(row(
                ui,
                "Starship",
                Some("A fast, informative prompt (when installed; otherwise a small built-in one)"),
                toggle("starship", p.starship, ui, self.host(cx, |h| h.prompt.starship = !h.prompt.starship)),
            ))
            .child(row(
                ui,
                "Style",
                Some("Minimal uses plain characters only and works with any font"),
                segmented("style", &[(PromptStyle::Rich, "Rich"), (PromptStyle::Minimal, "Minimal")], &p.style, ui, {
                    let this = cx.entity().downgrade();
                    move |st, _, cx| {
                        let _ = this.update(cx, |v, cx| v.edit_host(cx, |h| h.prompt.style = st));
                    }
                }),
            ))
            .child(row(
                ui,
                "Input on its own line",
                None,
                toggle("two-line", p.two_line, ui, self.host(cx, |h| h.prompt.two_line = !h.prompt.two_line)),
            ))
            .child(row(
                ui,
                "Empty line before each prompt",
                None,
                toggle("newline", p.newline_before, ui, self.host(cx, |h| h.prompt.newline_before = !h.prompt.newline_before)),
            ))
            .child(row(
                ui,
                "Show how long commands took",
                Some("For commands running at least this long"),
                stepper(
                    "duration",
                    if secs == 0 { "never".to_string() } else { format!("{secs} s") },
                    ui,
                    self.host(cx, |h| h.prompt.duration_threshold_ms = h.prompt.duration_threshold_ms.saturating_sub(1000) / 1000 * 1000),
                    self.host(cx, |h| h.prompt.duration_threshold_ms = (h.prompt.duration_threshold_ms / 1000 + 1).min(3600) * 1000),
                ),
            ))
            .child(row(
                ui,
                "Prefer my own Starship config",
                Some("Use ~/.config/starship.toml when it exists instead of these settings"),
                toggle(
                    "own-starship",
                    p.prefer_user_config,
                    ui,
                    self.host(cx, |h| h.prompt.prefer_user_config = !h.prompt.prefer_user_config),
                ),
            ))
            .into_any_element()
    }

    fn history(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let (header, ok) = self.host_header(ui);
        let page = div().flex().flex_col().child(header);
        let Some(h) = self.host_value().filter(|_| ok) else { return page.into_any_element() };
        let hs = h.history.clone();
        let sizes: [(u32, &str); 4] = [(1_000, "1 000"), (10_000, "10 000"), (100_000, "100 000"), (1_000_000, "1 000 000")];
        page.child(section(ui, "History"))
            .child(row(
                ui,
                "Entries kept",
                Some(&format!("Currently {}", hs.size)),
                segmented("hist-size", &sizes, &hs.size, ui, {
                    let this = cx.entity().downgrade();
                    move |n, _, cx| {
                        let _ = this.update(cx, |v, cx| v.edit_host(cx, |h| h.history.size = n));
                    }
                }),
            ))
            .child(row(
                ui,
                "Skip duplicates",
                Some("Don't save a command that repeats the previous one"),
                toggle("dups", hs.ignore_duplicates, ui, self.host(cx, |h| h.history.ignore_duplicates = !h.history.ignore_duplicates)),
            ))
            .child(row(
                ui,
                "Skip commands starting with a space",
                Some("Handy for one-off commands with secrets"),
                toggle("space", hs.ignore_space, ui, self.host(cx, |h| h.history.ignore_space = !h.history.ignore_space)),
            ))
            .child(row(
                ui,
                "Share between running shells",
                Some("Commands show up in other tabs right away"),
                toggle(
                    "share",
                    hs.share_between_sessions,
                    ui,
                    self.host(cx, |h| h.history.share_between_sessions = !h.history.share_between_sessions),
                ),
            ))
            .child(section(ui, "Atuin"))
            .child(row(
                ui,
                "Searchable history on Ctrl+R",
                Some("Atuin, when installed (goatty-setup installs it)"),
                toggle("atuin", hs.atuin, ui, self.host(cx, |h| h.history.atuin = !h.history.atuin)),
            ))
            .child(row(
                ui,
                "Atuin on the Up arrow too",
                Some("Off: Up searches plain history by what you typed"),
                toggle("atuin-up", hs.atuin_up_arrow, ui, self.host(cx, |h| h.history.atuin_up_arrow = !h.history.atuin_up_arrow)),
            ))
            .into_any_element()
    }

    // ---- Shortcuts ------------------------------------------------------------

    fn shortcuts(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let app = cx.global::<AppSettings>();
        let overrides = app.file.value.keybindings.clone();
        let mut page =
            div().flex().flex_col().child(note(ui, "Shortcuts work in terminal panes. Click + and press the new keys; Esc cancels."));
        if !app.key_errors.is_empty() {
            page = page.child(notice(ui, ui.warning, app.key_errors.clone()));
        }
        for (ai, a) in ACTIONS.iter().enumerate() {
            let keys_for = keys::keys_for(a.name, &overrides);
            let customized = overrides.iter().any(|(k, v)| {
                v.as_deref() == Some(a.name) || (v.is_none() && keys::DEFAULT_BINDINGS.iter().any(|(dk, da)| dk == k && *da == a.name))
            });
            let recording = self.recording == Some(a.name);
            let name = a.name;
            let chips = div()
                .flex()
                .flex_wrap()
                .gap_1()
                .items_center()
                .children(keys_for.into_iter().enumerate().map(|(ki, k)| {
                    let key = k.clone();
                    div()
                        .id(SharedString::from(format!("chip-{ai}-{ki}")))
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .rounded_md()
                        .border_1()
                        .border_color(ui.border)
                        .bg(ui.surface)
                        .text_sm()
                        .child(SharedString::from(pretty_keys(&k)))
                        .child(
                            div()
                                .id(SharedString::from(format!("rm-{ai}-{ki}")))
                                .text_color(ui.muted)
                                .cursor_pointer()
                                .hover(|s| s.text_color(ui.danger))
                                .child("×")
                                .on_click(move |_, _, cx| AppSettings::update(cx, |s| keys::set_binding(&mut s.keybindings, &key, None))),
                        )
                }))
                .child(if recording {
                    div()
                        .px_2()
                        .rounded_md()
                        .border_1()
                        .border_color(ui.accent)
                        .text_sm()
                        .text_color(ui.accent)
                        .child("Press keys…")
                        .into_any_element()
                } else {
                    button(SharedString::from(format!("add-{ai}")), "+", ButtonKind::Normal, ui, {
                        let this = cx.entity().downgrade();
                        move |window, cx| {
                            let _ = this.update(cx, |v, cx| {
                                v.recording = Some(name);
                                window.focus(&v.focus, cx);
                                cx.notify();
                            });
                        }
                    })
                    .into_any_element()
                })
                .when(customized, |d| {
                    d.child(button(SharedString::from(format!("reset-{ai}")), "Reset", ButtonKind::Normal, ui, move |_, cx| {
                        AppSettings::update(cx, |s| keys::reset_action(&mut s.keybindings, name))
                    }))
                });
            page = page.child(row(ui, a.label, None, chips));
        }
        page.into_any_element()
    }

    // ---- Workspace ------------------------------------------------------------

    fn workspace(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let s = &cx.global::<AppSettings>().file.value;
        div()
            .flex()
            .flex_col()
            .child(section(ui, "Starting goatty"))
            .child(row(
                ui,
                "New window",
                Some("Sessions keep running when you close the window; goatty can pick up where you left off"),
                segmented(
                    "on-launch",
                    &[(OnLaunch::Reattach, "Reattach the newest session"), (OnLaunch::New, "Start a new session")],
                    &s.workspace.on_launch,
                    ui,
                    |v, _, cx| AppSettings::update(cx, |s| s.workspace.on_launch = v),
                ),
            ))
            .child(note(ui, "Command-line flags (--new, --attach) still win. Tabs and splits arrive with #30."))
            .into_any_element()
    }

    // ---- Profile ----------------------------------------------------------------

    fn profile(&self, ui: Ui, cx: &Context<'_, Self>) -> AnyElement {
        let app = cx.global::<AppSettings>();
        let path = app.path.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(no $HOME: not saved)".into());
        let label = self.link.as_ref().map(|l| l.label()).unwrap_or_else(|| "no host".into());
        let mut problems: Vec<String> = app.problems().into_iter().map(|p| format!("this computer: {p}")).collect();
        if let Some(h) = &self.host {
            problems.extend(h.issues.iter().map(|i| format!("{label}: {i}")));
        }
        let mut page = div()
            .flex()
            .flex_col()
            .child(section(ui, "Files"))
            .child(row(ui, "Appearance, shortcuts, workspace", Some(&path), div()))
            .child(row(ui, &format!("Shell, prompt, history on {label}"), Some("~/.config/goatty/host.json on that host"), div()));
        if !app.env_overrides.is_empty() {
            page = page.child(note(ui, format!("Overridden for this run by {}", app.env_overrides.join(", "))));
        }
        page = page.child(section(ui, "Problems"));
        page = if problems.is_empty() {
            page.child(note(ui, "None: every setting was read as written."))
        } else {
            page.child(notice(ui, ui.warning, problems))
        };

        page = page
            .child(section(ui, "Share"))
            .child(row(
                ui,
                "Export a profile",
                Some("Both sides in one file, without machine-specific paths"),
                button("export", "Export…", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.export_profile(cx))),
            ))
            .child(row(
                ui,
                "Import a profile",
                Some("Appearance applies right away; shell settings are staged for preview"),
                button("import", "Import…", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.import_profile(cx))),
            ))
            .child(row(
                ui,
                "Reset appearance, shortcuts and workspace",
                Some("The current file is kept in backups/"),
                button("reset-client", "Reset", ButtonKind::Danger, ui, |_, cx| {
                    AppSettings::set(cx, ClientSettings { theme: ThemeSettings::default(), ..Default::default() })
                }),
            ));

        page = page.child(section(ui, &format!("Shell configuration generations on {label}"))).child(note(
            ui,
            "Each applied change builds a validated generation. Rolling back activates an older one and restores the settings it came from.",
        ));
        for g in self.generations.iter().rev() {
            let n = g.number;
            let control = if g.active {
                div().text_sm().text_color(ui.success).child("active").into_any_element()
            } else {
                button(("rollback", n as usize), "Roll back", ButtonKind::Normal, ui, self.act(cx, move |v, cx| v.rollback(n, cx)))
                    .into_any_element()
            };
            page = page.child(row(ui, &format!("Generation {n}"), Some(&ago(g.created_unix)), control));
        }
        if self.generations.is_empty() {
            page = page.child(note(ui, "None yet: one is built when the first managed shell starts or settings are applied."));
        }
        page.into_any_element()
    }

    // ---- Bottom bar -------------------------------------------------------------

    fn diff_view(&self, ui: Ui, mono: SharedString, diff: &[(String, String)]) -> impl IntoElement {
        div()
            .id("diff")
            .max_h(px(220.))
            .overflow_y_scroll()
            .rounded_md()
            .border_1()
            .border_color(ui.border)
            .bg(ui.input_bg)
            .p_2()
            .font_family(mono)
            .text_xs()
            .children(diff.iter().flat_map(|(_, text)| text.lines().map(str::to_owned).collect::<Vec<_>>()).map(|line| {
                let color = if line.starts_with("+++") || line.starts_with("---") {
                    ui.muted
                } else if line.starts_with('+') {
                    ui.success
                } else if line.starts_with('-') {
                    ui.danger
                } else if line.starts_with("@@") {
                    ui.accent
                } else {
                    ui.text
                };
                div().text_color(color).whitespace_nowrap().child(SharedString::from(if line.is_empty() { " ".to_string() } else { line }))
            }))
    }

    fn bottom_bar(&self, ui: Ui, cx: &Context<'_, Self>) -> Option<AnyElement> {
        if self.draft.is_none() && self.outcome.is_none() {
            return None;
        }
        let mono: SharedString = cx.global::<AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into()).into();
        let mut bar = div().flex().flex_col().gap_2().px_6().py_3().border_t_1().border_color(ui.border).bg(ui.surface);
        if let Some(o) = &self.outcome {
            bar = bar.child(notice(ui, if o.ok { ui.success } else { ui.danger }, o.lines.clone()));
            if !o.diff.is_empty() {
                bar = bar.child(div().flex().gap_2().child(button(
                    "toggle-diff",
                    if self.show_diff { "Hide changes" } else { "Show changes" },
                    ButtonKind::Normal,
                    ui,
                    self.act(cx, |v, cx| {
                        v.show_diff = !v.show_diff;
                        cx.notify();
                    }),
                )));
                if self.show_diff {
                    bar = bar.child(self.diff_view(ui, mono, &o.diff));
                }
            }
        }
        if self.draft.is_some() {
            let label = self.link.as_ref().map(|l| l.label()).unwrap_or_default();
            bar = bar.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().child(SharedString::from(format!("Unapplied shell settings for {label}"))))
                    .when(self.busy, |d| d.child(div().text_color(ui.muted).child("Working…")))
                    .child(button("discard", "Discard", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.discard(cx))))
                    .child(button("preview", "Preview", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.preview(cx))))
                    .child(button("apply", "Apply", ButtonKind::Primary, ui, self.act(cx, |v, cx| v.apply(cx)))),
            );
        } else {
            bar = bar.child(div().flex().justify_end().child(button(
                "dismiss",
                "OK",
                ButtonKind::Normal,
                ui,
                self.act(cx, |v, cx| {
                    v.outcome = None;
                    cx.notify();
                }),
            )));
        }
        Some(bar.into_any_element())
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let page = match self.page {
            Page::Appearance => self.appearance(ui, cx),
            Page::Shell => self.shell(ui, cx),
            Page::Prompt => self.prompt(ui, cx),
            Page::History => self.history(ui, cx),
            Page::Shortcuts => self.shortcuts(ui, cx),
            Page::Workspace => self.workspace(ui, cx),
            Page::Profile => self.profile(ui, cx),
        };
        let title = self.page.title();
        div()
            .size_full()
            .flex()
            .bg(ui.bg)
            .text_color(ui.text)
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .key_context(super::CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close))
            .on_key_down(cx.listener(Self::on_key))
            .child(self.sidebar(ui, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .child(
                        div()
                            .id("page")
                            .flex_1()
                            .overflow_y_scroll()
                            .px_6()
                            .py_4()
                            .child(div().text_xl().font_weight(FontWeight::BOLD).pb_2().child(title))
                            .child(page),
                    )
                    .children(self.bottom_bar(ui, cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        assert_eq!(pretty_keys("ctrl-shift-c"), "Ctrl+Shift+C");
        assert_eq!(pretty_keys("ctrl-shift--"), "Ctrl+Shift+-");
        assert_eq!(pretty_keys("shift-pageup"), "Shift+PageUp");
        assert_eq!(pretty_keys("ctrl-,"), "Ctrl+,");
    }

    #[test]
    fn monospace_guess() {
        assert!(looks_monospace("JetBrains Mono"));
        assert!(looks_monospace("Fira Code"));
        assert!(!looks_monospace("Noto Sans"));
    }
}
