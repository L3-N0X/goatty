//! The Prompt page: a live preview, the style (Starship's look, lean,
//! classic, rainbow) with its symbols, segment ends and separators, the
//! layout (lines, frame, connection, transient prompt), the directory, the
//! prompt character and colors; and the module list as a sub-page, each
//! module with a dialog for its side, symbol and color.
//!
//! Everything edits `prompt` in the host's draft (`goatty_config::prompt`),
//! applied with Preview/Apply like the other shell pages.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use goatty_config::HostSettings;
use goatty_config::prompt::{
    self, COLOR_NAMES, Category, Connection, DirectoryPrompt, Edge, Frame, MAX_TEXT, MODULES, Module, ModuleSet, PromptColor,
    PromptSettings, PromptStyle, Separator, Side, Substitution, Symbols,
};
use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, FontWeight, IntoElement, MouseButton, ParentElement, Role, SharedString, Styled,
    Subscription, Window, div, prelude::*, px, rgb,
};

use super::kit::{self, card, chip, divider, field, modal, opt, row, search_field, section};
use super::{SettingsWindow, builtin_bool_default};
use crate::settings::AppSettings;
use crate::ui::controls::{ButtonKind, Tone, badge, button, icon_button, segmented, toggle};
use crate::ui::focus::{self, Ring, Scroller};
use crate::ui::icons::Icon;
use crate::ui::prompt_preview::{Moment, Preview, resolve};
use crate::ui::style::{Ui, radius, text};
use crate::ui::text_input::{TextInput, TextInputEvent};

/// Sets a color in the draft: `None` = back to the default.
type SetColor = Rc<dyn Fn(&mut HostSettings, Option<String>)>;

/// Which modules the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    All,
    On,
    Changed,
}

struct Inputs {
    home: Entity<TextInput>,
    cut: Entity<TextInput>,
    success: Entity<TextInput>,
    error: Entity<TextInput>,
    vicmd: Entity<TextInput>,
    /// The module dialog's symbol.
    symbol: Entity<TextInput>,
    from: Entity<TextInput>,
    to: Entity<TextInput>,
    /// A color menu's custom color.
    hex: Entity<TextInput>,
}

/// The page's own state.
pub struct PromptPage {
    /// The module list is open (a sub-page).
    pub modules_open: bool,
    /// The module whose dialog is open.
    pub dialog: Option<&'static str>,
    pub(super) modal_focus: FocusHandle,
    opener: Option<FocusHandle>,
    /// Where focus goes on the next frame, after the dialog closed.
    pub(super) restore: Option<FocusHandle>,
    /// Categories folded or unfolded by the user (they start open).
    folded: HashSet<&'static str>,
    filter: Entity<TextInput>,
    scope: Scope,
    inputs: Inputs,
    /// What the open color menu changes (set while it's drawn), for its
    /// custom color field.
    color_target: RefCell<Option<SetColor>>,
    _subs: Vec<Subscription>,
}

/// Keep typed symbols within what the settings take.
fn short(t: &str) -> String {
    t.chars().filter(|c| !c.is_control()).take(MAX_TEXT).collect()
}

impl PromptPage {
    pub fn new(cx: &mut Context<'_, SettingsWindow>) -> Self {
        let input = |cx: &mut Context<'_, SettingsWindow>, placeholder: &str, label: &str| {
            let (placeholder, label) = (placeholder.to_owned(), label.to_owned());
            cx.new(|cx| TextInput::new(placeholder, "", cx).label(label))
        };
        let inputs = Inputs {
            home: input(cx, "~", "Home folder as"),
            cut: input(cx, "…/", "Cut folders as"),
            success: input(cx, "❯", "Prompt character after success"),
            error: input(cx, "❯", "Prompt character after failure"),
            vicmd: input(cx, "❮", "Prompt character in vi command mode"),
            symbol: input(cx, "No symbol", "Symbol"),
            from: input(cx, "~/coding/goatty", "Path to replace"),
            to: input(cx, "goatty", "Replacement"),
            hex: input(cx, "#ff8800 or 0–255", "Custom color"),
        };
        let filter = cx.new(|cx| TextInput::new("Search modules", "", cx).bare());
        // Typing changes the draft right away, so the preview follows.
        let on_change = |cx: &mut Context<'_, SettingsWindow>, e: &Entity<TextInput>, f: fn(&mut PromptSettings, String)| {
            cx.subscribe(e, move |v, _, ev: &TextInputEvent, cx| {
                if let TextInputEvent::Changed(t) = ev {
                    let t = short(t);
                    v.edit_host(cx, |h| f(&mut h.prompt, t));
                }
            })
        };
        let mut subs = vec![
            cx.subscribe(&filter, |_, _, _: &TextInputEvent, cx| cx.notify()),
            on_change(cx, &inputs.home, |p, t| p.directory.home_symbol = t),
            on_change(cx, &inputs.cut, |p, t| p.directory.truncation_symbol = t),
            on_change(cx, &inputs.success, |p, t| p.character.success = t),
            on_change(cx, &inputs.error, |p, t| p.character.error = t),
            on_change(cx, &inputs.vicmd, |p, t| p.character.vicmd = t),
            cx.subscribe(&inputs.symbol, |v, _, ev: &TextInputEvent, cx| match ev {
                TextInputEvent::Changed(t) => {
                    let Some(m) = v.prompt.dialog.and_then(prompt::module) else { return };
                    let t = short(t);
                    v.edit_host(cx, |h| {
                        let default = m.symbol(h.prompt.symbols);
                        h.prompt.edit_module(m.id, |c| c.symbol = (t != default).then_some(t));
                    });
                }
                TextInputEvent::Submitted(_) => v.close_module_dialog(cx),
            }),
            cx.subscribe(&inputs.hex, |v, _, ev: &TextInputEvent, cx| {
                if let TextInputEvent::Submitted(t) = ev {
                    v.custom_color(t, cx);
                }
            }),
        ];
        for e in [&inputs.from, &inputs.to] {
            subs.push(cx.subscribe(e, |v, _, ev: &TextInputEvent, cx| {
                if let TextInputEvent::Submitted(_) = ev {
                    v.add_substitution(cx);
                }
            }));
        }
        Self {
            modules_open: false,
            dialog: None,
            modal_focus: cx.focus_handle(),
            opener: None,
            restore: None,
            folded: HashSet::new(),
            filter,
            scope: Scope::All,
            inputs,
            color_target: RefCell::new(None),
            _subs: subs,
        }
    }
}

/// "bright-blue" → "Bright blue", "236" → "Color 236".
fn color_label(c: &str) -> String {
    match PromptColor::parse(c) {
        Some(PromptColor::Ansi(i)) => {
            let name = COLOR_NAMES[usize::from(i % 8)];
            let name = if name == "purple" { "magenta" } else { name };
            if i >= 8 {
                format!("Bright {name}")
            } else {
                let mut s = name.to_owned();
                s[..1].make_ascii_uppercase();
                s
            }
        }
        Some(PromptColor::Indexed(i)) => format!("Color {i}"),
        Some(PromptColor::Rgb(..)) => c.to_ascii_lowercase(),
        None => c.to_owned(),
    }
}

/// Grays from the 256-color ramp, for backgrounds.
const GRAYS: [u8; 10] = [233, 234, 235, 236, 237, 238, 240, 242, 244, 248];

impl SettingsWindow {
    // ---- State ------------------------------------------------------------

    /// Put the host's values into the page's text fields (after loading or
    /// discarding).
    pub(super) fn sync_prompt_inputs(&mut self, cx: &mut Context<'_, Self>) {
        let Some(p) = self.host_value().map(|h| h.prompt.clone()) else { return };
        let i = &self.prompt.inputs;
        for (input, text) in [
            (&i.home, p.directory.home_symbol.clone()),
            (&i.cut, p.directory.truncation_symbol.clone()),
            (&i.success, p.character.success.clone()),
            (&i.error, p.character.error.clone()),
            (&i.vicmd, p.character.vicmd.clone()),
        ] {
            input.update(cx, |input, cx| input.set_text(text, cx));
        }
        let plain = PromptSettings { character: Default::default(), ..p.clone() };
        let defaults = plain.character_symbols().map(str::to_owned);
        for (input, d) in [(&i.success, &defaults[0]), (&i.error, &defaults[1]), (&i.vicmd, &defaults[2])] {
            input.update(cx, |input, cx| input.set_placeholder(d.clone(), cx));
        }
    }

    pub(super) fn prompt_modal_open(&self) -> bool {
        self.prompt.dialog.is_some()
    }

    fn open_module_dialog(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(p) = self.host_value().map(|h| h.prompt.clone()) else { return };
        let Some(m) = prompt::module(id) else { return };
        self.prompt.inputs.symbol.update(cx, |input, cx| input.set_text(p.module_symbol(m).to_owned(), cx));
        self.prompt.opener = window.focused(cx);
        window.focus(&self.prompt.modal_focus, cx);
        self.prompt.dialog = Some(id);
        cx.notify();
    }

    pub(super) fn close_module_dialog(&mut self, cx: &mut Context<'_, Self>) {
        self.prompt.dialog = None;
        self.menu = None;
        self.prompt.restore = Some(self.prompt.opener.take().unwrap_or_else(|| self.focus.clone()));
        cx.notify();
    }

    /// Leave the module list.
    pub(super) fn close_prompt_modules(&mut self, cx: &mut Context<'_, Self>) {
        self.prompt.modules_open = false;
        self.prompt.dialog = None;
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
        cx.notify();
    }

    fn add_substitution(&mut self, cx: &mut Context<'_, Self>) {
        let from = short(self.prompt.inputs.from.read(cx).text().trim());
        let to = short(self.prompt.inputs.to.read(cx).text());
        if from.is_empty() {
            return;
        }
        self.edit_host(cx, |h| {
            let subs = &mut h.prompt.directory.substitutions;
            subs.retain(|s| s.from != from);
            subs.push(Substitution { from, to });
        });
        for i in [&self.prompt.inputs.from, &self.prompt.inputs.to] {
            i.update(cx, |i, cx| i.set_text("", cx));
        }
    }

    /// The custom color field of an open color menu.
    fn custom_color(&mut self, typed: &str, cx: &mut Context<'_, Self>) {
        let Some(c) = PromptColor::parse(typed) else { return };
        let Some(set) = self.prompt.color_target.borrow().clone() else { return };
        self.edit_host(cx, |h| set(h, Some(c.name())));
        self.menu = None;
        cx.notify();
    }

    /// Callback editing the prompt settings with a chosen value.
    fn set_prompt<T: 'static>(
        &self,
        cx: &Context<'_, Self>,
        f: impl Fn(&mut PromptSettings, T) + 'static,
    ) -> impl Fn(T, &mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |v, _, cx| {
            let _ = this.update(cx, |w, cx| w.edit_host(cx, |h| f(&mut h.prompt, v)));
        }
    }

    // ---- Pieces ---------------------------------------------------------------

    /// A swatch of `color` (the theme's version of it).
    fn swatch(&self, color: &str, size: f32, ui: Ui, cx: &Context<'_, Self>) -> gpui::Div {
        let t = &cx.global::<AppSettings>().colors;
        div()
            .flex_shrink_0()
            .size(px(size))
            .rounded(px(size / 4.))
            .border_1()
            .border_color(ui.border)
            .when_some(resolve(color, t), |d, c| d.bg(c))
    }

    /// A dropdown of colors: the theme's 16, grays, a custom one, or the
    /// default. `set` gets the choice (`None`: the default).
    fn color_select(
        &self,
        id: &str,
        name: &str,
        current: Option<&str>,
        default: &str,
        cx: &Context<'_, Self>,
        set: impl Fn(&mut HostSettings, Option<String>) + 'static,
    ) -> AnyElement {
        let ui = Ui::get(cx);
        let id: SharedString = id.to_owned().into();
        let open = self.menu.as_ref() == Some(&id);
        let set: SetColor = Rc::new(set);
        let shown = current.unwrap_or(default);
        let value = match current {
            Some(c) => color_label(c),
            None => format!("Default · {}", color_label(default)),
        };
        let mut list = kit::menu(ui, SharedString::from(format!("{id}-menu"))).p(px(10.)).gap(px(10.));
        if open {
            *self.prompt.color_target.borrow_mut() = Some(set.clone());
            let pick = |key: String, value: Option<String>, label: String, swatch: Option<String>, selected: bool| {
                let (set, this) = (set.clone(), cx.entity().downgrade());
                let el_id: gpui::ElementId = SharedString::from(format!("{id}-{key}")).into();
                let el = div()
                    .id(el_id.clone())
                    .role(Role::Button)
                    .aria_label(SharedString::from(label))
                    .aria_selected(selected)
                    .size(px(26.))
                    .rounded(px(7.))
                    .border_2()
                    .border_color(if selected { ui.accent } else { ui.divider })
                    .cursor_pointer()
                    .when_some(swatch.and_then(|s| resolve(&s, &cx.global::<AppSettings>().colors)), |d, c| d.bg(c))
                    .on_click(move |_, window, cx| {
                        let value = value.clone();
                        let set = set.clone();
                        let _ = this.update(cx, |v, cx| {
                            v.edit_host(cx, |h| set(h, value));
                            v.close_menu(window, cx);
                        });
                    });
                focus::tab(el, &el_id, ui, Ring::Outer, Scroller::Menu)
            };
            let selected_name = current.and_then(PromptColor::parse);
            let grid = |items: Vec<gpui::Stateful<gpui::Div>>| div().flex().flex_wrap().gap(px(6.)).children(items);
            let theme: Vec<_> = (0u8..16)
                .map(|i| {
                    let c = PromptColor::Ansi(i);
                    pick(format!("a{i}"), Some(c.name()), color_label(&c.name()), Some(c.name()), selected_name == Some(c))
                })
                .collect();
            let grays: Vec<_> = GRAYS
                .iter()
                .map(|&i| {
                    let c = PromptColor::Indexed(i);
                    pick(format!("g{i}"), Some(c.name()), color_label(&c.name()), Some(c.name()), selected_name == Some(c))
                })
                .collect();
            let (default_set, this) = (set.clone(), cx.entity().downgrade());
            let default_id = SharedString::from(format!("{id}-default"));
            list =
                list.w(px(276.))
                    .child(
                        kit::menu_item(ui, default_id, format!("Default · {}", color_label(default)).into(), None, current.is_none())
                            .on_click(move |_, window, cx| {
                                let set = default_set.clone();
                                let _ = this.update(cx, |v, cx| {
                                    v.edit_host(cx, |h| set(h, None));
                                    v.close_menu(window, cx);
                                });
                            }),
                    )
                    .child(field(ui, "Theme colors", grid(theme)))
                    .child(field(ui, "Grays", grid(grays)))
                    .child(field(ui, "Custom (Enter to use)", self.prompt.inputs.hex.clone()));
        }
        let label = div().flex().items_center().gap(px(8.)).child(self.swatch(shown, 16., ui, cx)).child(SharedString::from(value.clone()));
        let trigger = kit::select_trigger(ui, id.clone(), name.to_owned(), value, label, open);
        self.dropdown(id, trigger, list, cx)
    }

    fn mono_font(cx: &App) -> gpui::Font {
        let family = cx.global::<AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into());
        crate::fonts::terminal(&family)
    }

    /// A short symbol field on the right of a row.
    fn symbol_field(input: &Entity<TextInput>, cx: &App) -> impl IntoElement {
        div().w(px(150.)).font(Self::mono_font(cx)).child(input.clone())
    }

    // ---- The page ---------------------------------------------------------------

    /// What the prompt looks like with the settings on screen.
    fn prompt_preview(&self, ui: Ui, h: &HostSettings, cx: &Context<'_, Self>) -> impl IntoElement {
        let t = &cx.global::<AppSettings>().colors;
        let fg: gpui::Hsla = rgb(t.foreground.0).into();
        let p = &h.prompt;
        let starship = h.plugin_enabled("starship");
        let own = starship && h.plugin_bool("starship", "prefer_user_config", builtin_bool_default("starship", "prefer_user_config"));
        let preview = Preview { p, theme: t, starship, line_height: 22. };
        let dim = fg.opacity(0.6);
        let typed = div().child("cargo build");
        let cursor = div().w(px(8.)).h(px(17.)).bg(rgb(t.cursor.0));
        let mut lines: Vec<AnyElement> = Vec::new();
        if p.transient && starship {
            lines.push(preview.transient(false, typed).into_any_element());
        } else {
            lines.extend(preview.prompt(Moment::Fresh, typed).into_iter().map(IntoElement::into_any_element));
        }
        let out = |s: &'static str| div().h(px(22.)).flex().items_center().text_color(dim).child(s).into_any_element();
        lines.push(out("   Compiling goatty v0.4.2"));
        lines.push(out("error: could not compile `goatty`"));
        if starship && p.newline_before {
            lines.push(div().h(px(22.)).into_any_element());
        }
        lines.extend(preview.prompt(Moment::Failed, cursor).into_iter().map(IntoElement::into_any_element));
        div()
            .flex()
            .flex_col()
            .rounded(px(radius::CARD))
            .overflow_hidden()
            .border_1()
            .border_color(ui.divider)
            .child(
                div()
                    .bg(rgb(t.background.0))
                    .px(px(18.))
                    .py(px(14.))
                    .font(Self::mono_font(cx))
                    .text_size(px(13.5))
                    .text_color(fg)
                    .children(lines),
            )
            .when(own, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(14.))
                        .py(px(9.))
                        .bg(ui.card)
                        .text_size(px(text::SMALL))
                        .text_color(ui.muted)
                        .child(Icon::Info.svg().size(px(14.)).text_color(ui.muted))
                        .child("If you have a ~/.config/starship.toml, it decides how the prompt looks instead"),
                )
            })
    }

    /// The style cards, each showing a small prompt in that style.
    fn style_cards(&self, ui: Ui, p: &PromptSettings, cx: &Context<'_, Self>) -> impl IntoElement {
        let t = &cx.global::<AppSettings>().colors;
        let cards = PromptStyle::ALL.into_iter().map(|style| {
            let mut sample = PromptSettings {
                style,
                two_line: false,
                frame: Frame::None,
                show: ModuleSet::Essential,
                modules: Default::default(),
                duration_threshold_ms: 0,
                ..p.clone()
            };
            sample.directory.truncation_length = 1;
            sample.directory.repo_root = false;
            let preview = Preview { p: &sample, theme: t, starship: true, line_height: 20. };
            let lines = preview.prompt(Moment::Fresh, div());
            let body = div()
                .h(px(56.))
                .px(px(12.))
                .flex()
                .flex_col()
                .justify_center()
                .overflow_hidden()
                .bg(rgb(t.background.0))
                .font(Self::mono_font(cx))
                .text_size(px(12.))
                .children(lines);
            kit::choice_card(ui, SharedString::from(format!("prompt-style-{}", style.label())), style.label(), p.style == style, body)
                .min_w(px(120.))
                .on_click(cx.listener(move |v: &mut Self, _: &gpui::ClickEvent, _, cx| v.edit_host(cx, |h| h.prompt.style = style)))
        });
        div().flex().flex_wrap().gap(px(10.)).p(px(14.)).children(cards)
    }

    pub(super) fn prompt(&self, ui: Ui, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        if self.prompt.modules_open {
            return self.host_page(ui, |h| vec![self.prompt_modules(ui, h, window, cx)]);
        }
        self.host_page(ui, |h| {
            let p = h.prompt.clone();
            let starship = h.plugin_enabled("starship");
            let own = h.plugin_bool("starship", "prefer_user_config", builtin_bool_default("starship", "prefer_user_config"));
            let needs = |on: bool, desc: &'static str| if on { desc } else { "Needs Starship" };
            let segments = p.style.has_segments();

            let main = section(ui, "Prompt")
                .child(row(ui, "Starship").desc("Git status, durations and more. Off: goatty's small built-in prompt").control(toggle(
                    "starship",
                    starship,
                    ui,
                    self.toggle_plugin(cx, "starship"),
                )))
                .child(
                    row(ui, "Use my own starship.toml")
                        .desc(needs(starship, "~/.config/starship.toml, when it exists, instead of these settings"))
                        .disabled(!starship)
                        .control(toggle("own-starship", own, ui, self.toggle_plugin_bool(cx, "starship", "prefer_user_config"))),
                );

            let symbols = Symbols::ALL
                .into_iter()
                .map(|s| {
                    opt(s, s.label()).desc(match s {
                        Symbols::Nerd => "Nerd Font icons, drawn with goatty's bundled font",
                        Symbols::Emoji => "Starship's own symbols, mostly emoji",
                        Symbols::Text => "Short words like “git” and “rs”: any font",
                        Symbols::None => "Just the values",
                    })
                })
                .collect();
            let edges = || Edge::ALL.into_iter().map(|e| opt(e, e.label())).collect::<Vec<_>>();
            let style = section(ui, "Style")
                .desc(needs(starship, "How modules are drawn"))
                .child(self.style_cards(ui, &p, cx))
                .child(
                    row(ui, "Symbols")
                        .desc(needs(starship, "Icons, emoji, words or nothing before each value"))
                        .disabled(!starship)
                        .control(self.select(
                            "prompt-symbols",
                            "Symbols",
                            symbols,
                            &p.symbols,
                            cx,
                            self.set_prompt(cx, |p, s| p.symbols = s),
                        )),
                )
                .child(
                    row(ui, "Separators")
                        .desc(if p.style == PromptStyle::Classic { "Between segments of the bar" } else { "Only for Classic" })
                        .disabled(p.style != PromptStyle::Classic || !starship)
                        .control(self.select(
                            "prompt-separator",
                            "Separators",
                            Separator::ALL.into_iter().map(|s| opt(s, s.label())).collect(),
                            &p.separator,
                            cx,
                            self.set_prompt(cx, |p, s| p.separator = s),
                        )),
                )
                .child(
                    row(ui, "Segment heads")
                        .desc(if segments { "The end that points toward your input" } else { "Only for Classic and Rainbow" })
                        .disabled(!segments || !starship)
                        .control(self.select(
                            "prompt-heads",
                            "Segment heads",
                            edges(),
                            &p.heads,
                            cx,
                            self.set_prompt(cx, |p, e| p.heads = e),
                        )),
                )
                .child(
                    row(ui, "Segment tails")
                        .desc(if segments { "The other end" } else { "Only for Classic and Rainbow" })
                        .disabled(!segments || !starship)
                        .control(self.select(
                            "prompt-tails",
                            "Segment tails",
                            edges(),
                            &p.tails,
                            cx,
                            self.set_prompt(cx, |p, e| p.tails = e),
                        )),
                );

            let mut durations: Vec<u32> = vec![0, 1000, 2000, 3000, 5000, 10_000, 30_000, 60_000];
            if !durations.contains(&p.duration_threshold_ms) {
                durations.push(p.duration_threshold_ms);
                durations.sort_unstable();
            }
            let duration_opts = durations
                .into_iter()
                .map(|ms| match ms {
                    0 => opt(0, "Never"),
                    ms if ms % 1000 == 0 => opt(ms, format!("After {} s", ms / 1000)),
                    ms => opt(ms, format!("After {ms} ms")),
                })
                .collect();
            let two = p.two_line;
            let right = MODULES.iter().any(|m| p.module_on(m) && p.module_side(m) == Side::Right);
            let layout = section(ui, "Layout")
                .child(row(ui, "Input on its own line").desc("Type commands on a line below the directory").control(toggle(
                    "two-line",
                    p.two_line,
                    ui,
                    self.host(cx, |h| h.prompt.two_line = !h.prompt.two_line),
                )))
                .child(
                    row(ui, "Frame")
                        .desc(if !starship {
                            "Needs Starship"
                        } else if two {
                            "Lines around the prompt's two lines"
                        } else {
                            "Needs the input on its own line"
                        })
                        .disabled(!two || !starship)
                        .control(segmented(
                            "prompt-frame",
                            &Frame::ALL.map(|f| (f, f.label())),
                            &p.frame,
                            ui,
                            self.set_prompt(cx, |p, f| p.frame = f),
                        )),
                )
                .child(
                    row(ui, "Connection line")
                        .desc(if !starship {
                            "Needs Starship"
                        } else if !two {
                            "Needs the input on its own line"
                        } else if right || p.frame == Frame::Full {
                            "Fills the gap to what's on the right"
                        } else {
                            "Shows once something is on the right (Modules)"
                        })
                        .disabled(!two || !starship)
                        .control(segmented(
                            "prompt-connection",
                            &Connection::ALL.map(|c| (c, c.label())),
                            &p.connection,
                            ui,
                            self.set_prompt(cx, |p, c| p.connection = c),
                        )),
                )
                .child(
                    row(ui, "Empty line before each prompt")
                        .desc(needs(starship, "Space between one command's output and the next prompt"))
                        .disabled(!starship)
                        .control(toggle(
                            "newline",
                            p.newline_before,
                            ui,
                            self.host(cx, |h| h.prompt.newline_before = !h.prompt.newline_before),
                        )),
                )
                .child(
                    row(ui, "Shrink old prompts")
                        .desc("Once a command runs, its prompt becomes just ❯, so scrollback stays tidy")
                        .control(toggle("transient", p.transient, ui, self.host(cx, |h| h.prompt.transient = !h.prompt.transient))),
                )
                .child(
                    row(ui, "Show how long commands took")
                        .desc(needs(starship, "For commands that ran at least this long"))
                        .disabled(!starship)
                        .control(self.select(
                            "duration",
                            "Show how long commands took",
                            duration_opts,
                            &p.duration_threshold_ms,
                            cx,
                            self.set_prompt(cx, |p, ms| p.duration_threshold_ms = ms),
                        )),
                );

            let on = MODULES.iter().filter(|m| p.module_on(m)).count();
            let modules = section(ui, "Modules")
                .child(
                    row(ui, "Modules shown")
                        .desc(needs(starship, "Everything relevant where you are, or just directory, git and duration"))
                        .disabled(!starship)
                        .control(segmented(
                            "prompt-show",
                            &[(ModuleSet::All, "All"), (ModuleSet::Essential, "Essentials")],
                            &p.show,
                            ui,
                            self.set_prompt(cx, |p, s| p.show = s),
                        )),
                )
                .child(
                    row(ui, "Choose modules")
                        .icon(Icon::Puzzle)
                        .desc(format!("Switch each one, put it on the right, change its symbol or color · {on} of {} on", MODULES.len()))
                        .on_click(
                            "prompt-modules",
                            cx.listener(|v, _, _, cx| {
                                v.prompt.modules_open = true;
                                v.scroll.set_offset(gpui::point(px(0.), px(0.)));
                                cx.notify();
                            }),
                        ),
                );

            vec![
                self.prompt_preview(ui, h, cx).into_any_element(),
                main.into_any_element(),
                style.into_any_element(),
                layout.into_any_element(),
                self.directory_section(ui, &p, starship, cx).into_any_element(),
                self.character_section(ui, &p, starship, cx).into_any_element(),
                self.colors_section(ui, &p, starship, cx).into_any_element(),
                modules.into_any_element(),
            ]
        })
    }

    fn directory_section(&self, ui: Ui, p: &PromptSettings, starship: bool, cx: &Context<'_, Self>) -> impl IntoElement {
        let d = &p.directory;
        let mut lengths: Vec<u8> = (0..=8).collect();
        if !lengths.contains(&d.truncation_length) {
            lengths.push(d.truncation_length);
        }
        let length_opts = lengths
            .into_iter()
            .map(|n| match n {
                0 => opt(0, "Whole path"),
                1 => opt(1, "Last folder"),
                n => opt(n, format!("Last {n} folders")),
            })
            .collect();
        let abbreviate_opts = (0..=DirectoryPrompt::MAX_ABBREVIATE.min(3))
            .chain((d.abbreviate > 3).then_some(d.abbreviate))
            .map(|n| match n {
                0 => opt(0, "Hide them").desc("Show the cut marker instead"),
                1 => opt(1, "First letter").desc("~/p/goatty/src, like fish"),
                n => opt(n, format!("First {n} letters")),
            })
            .collect();
        let directory = prompt::module("directory").expect("directory module");
        let current = p.modules.get("directory").and_then(|c| c.color.clone());
        let off = !starship;
        let subs = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .children(d.substitutions.iter().enumerate().map(|(i, s)| {
                let from = s.from.clone();
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .h(px(36.))
                    .rounded(px(radius::CONTROL))
                    .bg(ui.field)
                    .font(Self::mono_font(cx))
                    .text_size(px(text::LABEL))
                    .child(div().truncate().child(SharedString::from(s.from.clone())))
                    .child(Icon::ChevronRight.svg().flex_shrink_0().size(px(14.)).text_color(ui.muted))
                    .child(div().flex_1().truncate().child(SharedString::from(if s.to.is_empty() {
                        "(nothing)".to_owned()
                    } else {
                        s.to.clone()
                    })))
                    .child(icon_button(
                        SharedString::from(format!("prompt-sub-remove-{i}")),
                        Icon::Trash,
                        "Remove",
                        ui,
                        self.host(cx, move |h| h.prompt.directory.substitutions.retain(|s| s.from != from)),
                    ))
            }))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(10.))
                    .child(field(ui, "Path", div().font(Self::mono_font(cx)).child(self.prompt.inputs.from.clone())).flex_1())
                    .child(field(ui, "Shown as", div().font(Self::mono_font(cx)).child(self.prompt.inputs.to.clone())).flex_1())
                    .child(button("prompt-sub-add", "Add", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.add_substitution(cx)))),
            );
        section(ui, "Directory")
            .desc(if off { "Needs Starship" } else { "How the working directory is shown" })
            .child(row(ui, "Folders shown").desc("Long paths are cut to this many folders").disabled(off).control(self.select(
                "prompt-dir-length",
                "Folders shown",
                length_opts,
                &d.truncation_length,
                cx,
                self.set_prompt(cx, |p, n| p.directory.truncation_length = n),
            )))
            .child(row(ui, "Cut folders").desc("What's left of the folders that were cut").disabled(off).control(self.select(
                "prompt-dir-abbreviate",
                "Cut folders",
                abbreviate_opts,
                &d.abbreviate,
                cx,
                self.set_prompt(cx, |p, n| p.directory.abbreviate = n),
            )))
            .child(
                row(ui, "Start at the repository")
                    .desc("Inside a git repository, show the path from its top folder")
                    .disabled(off)
                    .control(toggle(
                        "prompt-dir-repo",
                        d.truncate_to_repo,
                        ui,
                        self.host(cx, |h| h.prompt.directory.truncate_to_repo = !h.prompt.directory.truncate_to_repo),
                    )),
            )
            .child(row(ui, "Highlight the repository").desc("Its top folder in bold").disabled(off).control(toggle(
                "prompt-dir-root",
                d.repo_root,
                ui,
                self.host(cx, |h| h.prompt.directory.repo_root = !h.prompt.directory.repo_root),
            )))
            .child(row(ui, "Lock on read-only folders").desc("A lock after folders you can't write to").disabled(off).control(toggle(
                "prompt-dir-lock",
                d.read_only,
                ui,
                self.host(cx, |h| h.prompt.directory.read_only = !h.prompt.directory.read_only),
            )))
            .child(
                row(ui, "Home folder as")
                    .desc("Stands for your home folder")
                    .disabled(off)
                    .control(Self::symbol_field(&self.prompt.inputs.home, cx)),
            )
            .child(
                row(ui, "Cut folders as")
                    .desc("Marks where folders were cut")
                    .disabled(off)
                    .control(Self::symbol_field(&self.prompt.inputs.cut, cx)),
            )
            .child(row(ui, "Directory color").disabled(off).control(self.color_select(
                "prompt-dir-color",
                "Directory color",
                current.as_deref(),
                directory.color,
                cx,
                |h, c| h.prompt.edit_module("directory", |m| m.color = c),
            )))
            .child(row(ui, "Path replacements").desc("Show a path as something shorter, in order").disabled(off).below(subs))
    }

    fn character_section(&self, ui: Ui, p: &PromptSettings, starship: bool, cx: &Context<'_, Self>) -> impl IntoElement {
        let off = !starship;
        let auto = if p.symbols == Symbols::Text { "Empty: > with text symbols" } else { "Empty: ❯" };
        section(ui, "Prompt character")
            .desc(if off { "Needs Starship" } else { "What your input follows" })
            .child(row(ui, "After success").desc(auto).disabled(off).control(Self::symbol_field(&self.prompt.inputs.success, cx)))
            .child(
                row(ui, "After failure")
                    .desc("When the last command failed")
                    .disabled(off)
                    .control(Self::symbol_field(&self.prompt.inputs.error, cx)),
            )
            .child(
                row(ui, "In vi command mode")
                    .desc("With vi keys, outside insert mode")
                    .disabled(off)
                    .control(Self::symbol_field(&self.prompt.inputs.vicmd, cx)),
            )
    }

    fn colors_section(&self, ui: Ui, p: &PromptSettings, starship: bool, cx: &Context<'_, Self>) -> impl IntoElement {
        let off = !starship;
        let d = prompt::PromptColors::default();
        let dc = prompt::CharacterPrompt::default();
        let opt_of = |cur: &str, def: &str| (cur != def).then(|| cur.to_owned());
        let classic = p.style == PromptStyle::Classic;
        let rainbow = p.style == PromptStyle::Rainbow;
        let framed = p.two_line && (p.frame != Frame::None || p.connection != Connection::None);
        section(ui, "Colors")
            .desc(if off { "Needs Starship" } else { "Each module's own color is in its settings (Modules)" })
            .child(row(ui, "Prompt character").desc("After success").disabled(off).control(self.color_select(
                "prompt-color-ok",
                "Prompt character",
                opt_of(&p.character.success_color, &dc.success_color).as_deref(),
                &dc.success_color,
                cx,
                move |h, c| h.prompt.character.success_color = c.unwrap_or_else(|| prompt::CharacterPrompt::default().success_color),
            )))
            .child(row(ui, "Prompt character after failure").disabled(off).control(self.color_select(
                "prompt-color-err",
                "Prompt character after failure",
                opt_of(&p.character.error_color, &dc.error_color).as_deref(),
                &dc.error_color,
                cx,
                move |h, c| h.prompt.character.error_color = c.unwrap_or_else(|| prompt::CharacterPrompt::default().error_color),
            )))
            .child(
                row(ui, "Bar background")
                    .desc(if classic { "Behind the Classic bar" } else { "Only for Classic" })
                    .disabled(off || !classic)
                    .control(self.color_select(
                        "prompt-color-segment",
                        "Bar background",
                        opt_of(&p.colors.segment, &d.segment).as_deref(),
                        &d.segment,
                        cx,
                        |h, c| h.prompt.colors.segment = c.unwrap_or_else(|| prompt::PromptColors::default().segment),
                    )),
            )
            .child(
                row(ui, "Separator color")
                    .desc(if classic { "Between segments of the bar" } else { "Only for Classic" })
                    .disabled(off || !classic)
                    .control(self.color_select(
                        "prompt-color-separator",
                        "Separator color",
                        opt_of(&p.colors.separator, &d.separator).as_deref(),
                        &d.separator,
                        cx,
                        |h, c| h.prompt.colors.separator = c.unwrap_or_else(|| prompt::PromptColors::default().separator),
                    )),
            )
            .child(
                row(ui, "Text on segments")
                    .desc(if rainbow { "On the colored segments" } else { "Only for Rainbow" })
                    .disabled(off || !rainbow)
                    .control(self.color_select(
                        "prompt-color-text",
                        "Text on segments",
                        opt_of(&p.colors.segment_text, &d.segment_text).as_deref(),
                        &d.segment_text,
                        cx,
                        |h, c| h.prompt.colors.segment_text = c.unwrap_or_else(|| prompt::PromptColors::default().segment_text),
                    )),
            )
            .child(
                row(ui, "Frame and connection line")
                    .desc(if framed { "The lines around and across the prompt" } else { "With a frame or connection line" })
                    .disabled(off || !framed)
                    .control(self.color_select(
                        "prompt-color-frame",
                        "Frame and connection line",
                        opt_of(&p.colors.frame, &d.frame).as_deref(),
                        &d.frame,
                        cx,
                        |h, c| h.prompt.colors.frame = c.unwrap_or_else(|| prompt::PromptColors::default().frame),
                    )),
            )
    }

    // ---- Modules ----------------------------------------------------------------

    fn module_matches(&self, p: &PromptSettings, m: &Module, query: &str) -> bool {
        let scope = match self.prompt.scope {
            Scope::All => true,
            Scope::On => p.module_on(m),
            Scope::Changed => p.modules.contains_key(m.id),
        };
        scope
            && (query.is_empty()
                || m.label.to_lowercase().contains(query)
                || m.id.contains(query)
                || m.category.label().to_lowercase().contains(query))
    }

    fn prompt_modules(&self, ui: Ui, h: &HostSettings, window: &Window, cx: &Context<'_, Self>) -> AnyElement {
        let p = &h.prompt;
        let query = self.prompt.filter.read(cx).text().trim().to_lowercase();
        let count = |s: Scope| {
            MODULES
                .iter()
                .filter(|m| match s {
                    Scope::All => true,
                    Scope::On => p.module_on(m),
                    Scope::Changed => p.modules.contains_key(m.id),
                })
                .count()
        };
        let chip_for = |s: Scope, label: &'static str| {
            chip(
                ui,
                SharedString::from(format!("prompt-scope-{label}")),
                label,
                Some(count(s)),
                self.prompt.scope == s,
                self.act(cx, move |v, cx| {
                    v.prompt.scope = s;
                    cx.notify();
                }),
            )
        };
        let filters = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().min_w(px(220.)).child(search_field(ui, "prompt-modules-search", &self.prompt.filter, window, cx)))
            .child(chip_for(Scope::All, "All"))
            .child(chip_for(Scope::On, "On"))
            .child(chip_for(Scope::Changed, "Changed"));
        let filtering = !query.is_empty() || self.prompt.scope != Scope::All;
        let groups: Vec<AnyElement> = Category::ALL
            .into_iter()
            .filter_map(|c| {
                let rows: Vec<&'static Module> = MODULES.iter().filter(|m| m.category == c && self.module_matches(p, m, &query)).collect();
                (!rows.is_empty()).then(|| self.module_group(ui, p, c, &rows, filtering, cx))
            })
            .collect();
        let empty = groups.is_empty().then(|| kit::empty(ui, Icon::Search, "No modules match", "Try another word, or show all modules"));
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(filters)
            .children(groups)
            .children(empty)
            .child(kit::hint(ui, "A module shows only where it's relevant: Rust in a Cargo project, AWS with a profile set. The preview on the Prompt page shows a few of them.").px(px(4.)))
            .into_any_element()
    }

    fn module_group(
        &self,
        ui: Ui,
        p: &PromptSettings,
        c: Category,
        rows: &[&'static Module],
        filtering: bool,
        cx: &Context<'_, Self>,
    ) -> AnyElement {
        let key = c.label();
        let open = filtering || !self.prompt.folded.contains(key);
        let total = MODULES.iter().filter(|m| m.category == c).count();
        let on = MODULES.iter().filter(|m| m.category == c && p.module_on(m)).count();
        let header_id: gpui::ElementId = SharedString::from(format!("prompt-fold-{key}")).into();
        let header = div()
            .id(header_id.clone())
            .role(Role::Button)
            .aria_label(SharedString::from(key))
            .aria_expanded(open)
            .flex()
            .items_center()
            .gap(px(10.))
            .h(px(52.))
            .px(px(16.))
            .cursor_pointer()
            .hover(|s| s.bg(ui.surface_hover))
            .rounded_t(px(radius::CARD))
            .when(!open, |d| d.rounded_b(px(radius::CARD)))
            .child((if open { Icon::ChevronDown } else { Icon::ChevronRight }).svg().size(px(15.)).text_color(ui.muted))
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(key))
            .child(div().text_size(px(text::SMALL)).text_color(ui.faint).child(SharedString::from(format!("{on} of {total} on"))))
            .on_click(cx.listener(move |v, _, _, cx| {
                if !v.prompt.folded.remove(key) {
                    v.prompt.folded.insert(key);
                }
                cx.notify();
            }));
        let header = focus::tab(header, &header_id, ui, Ring::Inner, Scroller::Page);
        let mut body = card(ui).child(header);
        if open {
            body = body.child(div().h(px(1.)).bg(ui.divider));
            for (i, m) in rows.iter().enumerate() {
                if i > 0 {
                    body = body.child(divider(ui));
                }
                body = body.child(self.module_row(ui, p, m, cx));
            }
        }
        body.into_any_element()
    }

    /// A module's symbol in its color, in a tinted square.
    fn module_badge(&self, ui: Ui, p: &PromptSettings, m: &Module, cx: &App) -> impl IntoElement {
        let t = &cx.global::<AppSettings>().colors;
        let symbol = p.module_symbol(m).trim().to_owned();
        let shown = if symbol.is_empty() { m.label.chars().next().map(String::from).unwrap_or_default() } else { symbol };
        div()
            .flex_shrink_0()
            .size(px(34.))
            .rounded(px(radius::CONTROL))
            .bg(rgb(t.background.0))
            .border_1()
            .border_color(ui.divider)
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .font(Self::mono_font(cx))
            .text_size(px(15.))
            .text_color(resolve(p.module_color(m), t).unwrap_or(ui.text))
            .child(SharedString::from(shown))
    }

    fn module_row(&self, ui: Ui, p: &PromptSettings, m: &'static Module, cx: &Context<'_, Self>) -> AnyElement {
        let on = p.module_on(m);
        let cfg = p.modules.get(m.id);
        let mut notes = Vec::new();
        if p.module_side(m) == Side::Right {
            notes.push("on the right");
        }
        if cfg.is_some_and(|c| c.symbol.is_some()) {
            notes.push("own symbol");
        }
        if cfg.is_some_and(|c| c.color.is_some()) {
            notes.push("own color");
        }
        let desc = if notes.is_empty() { m.id.to_owned() } else { format!("{} · {}", m.id, notes.join(", ")) };
        let fixed = matches!(m.id, "directory");
        let row_id: gpui::ElementId = SharedString::from(format!("prompt-module-{}", m.id)).into();
        let el = div()
            .id(row_id.clone())
            .role(Role::Button)
            .aria_label(SharedString::from(format!("{} settings", m.label)))
            .flex()
            .items_center()
            .gap(px(14.))
            .min_h(px(58.))
            .px(px(16.))
            .py(px(10.))
            .cursor_pointer()
            .hover(|s| s.bg(ui.surface_hover))
            .child(self.module_badge(ui, p, m, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().text_size(px(text::BODY)).text_color(if on { ui.text } else { ui.muted }).child(m.label))
                            .when(fixed, |d| d.child(badge("Always", Tone::Neutral, ui))),
                    )
                    .child(div().text_size(px(text::SMALL)).text_color(ui.muted).truncate().child(SharedString::from(desc))),
            )
            .when(!fixed, |d| {
                d.child(
                    div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(
                        toggle(
                            SharedString::from(format!("prompt-on-{}", m.id)),
                            on,
                            ui,
                            self.host(cx, move |h| h.prompt.set_module_on(m, !on)),
                        )
                        .aria_label(SharedString::from(m.label)),
                    ),
                )
            })
            .child(Icon::ChevronRight.svg().size(px(16.)).text_color(ui.faint))
            .on_click(cx.listener(move |v, _, window, cx| v.open_module_dialog(m.id, window, cx)));
        focus::tab(el, &row_id, ui, Ring::Inner, Scroller::Page).into_any_element()
    }

    /// The open module's dialog: on/off, side, symbol, color.
    pub(super) fn module_modal(&self, ui: Ui, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let m = self.prompt.dialog.and_then(prompt::module)?;
        let h = self.host_value()?;
        let p = &h.prompt;
        let on = p.module_on(m);
        let cfg = p.modules.get(m.id).cloned().unwrap_or_default();
        let default_symbol = m.symbol(p.symbols);
        let lead = (m.lead() != m.id).then(|| prompt::module(m.lead())).flatten();
        let id = m.id;
        let symbol_row = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(div().flex_1().font(Self::mono_font(cx)).child(self.prompt.inputs.symbol.clone()))
            .when(cfg.symbol.is_some(), |d| {
                d.child(button(
                    "prompt-module-symbol-reset",
                    "Default",
                    ButtonKind::Ghost,
                    ui,
                    self.act(cx, move |v, cx| {
                        v.edit_host(cx, |h| h.prompt.edit_module(id, |c| c.symbol = None));
                        let shown = v
                            .host_value()
                            .and_then(|h| prompt::module(id).map(|m| h.prompt.module_symbol(m).to_owned()))
                            .unwrap_or_default();
                        v.prompt.inputs.symbol.update(cx, |i, cx| i.set_text(shown, cx));
                    }),
                ))
            });
        let symbol_note = if !m.has_symbol() && m.id != "os" {
            "This module has no symbol".to_owned()
        } else if default_symbol.trim().is_empty() {
            "Empty by default".to_owned()
        } else {
            format!("Default: {}", default_symbol.trim())
        };
        let color_note = match lead {
            Some(l) if p.style == PromptStyle::Rainbow => format!("In Rainbow it sits on {}'s color", l.label),
            _ if p.style == PromptStyle::Rainbow => "Its segment's background".to_owned(),
            _ => "Its text".to_owned(),
        };
        let fixed = m.id == "directory";
        let body = div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(self.module_badge(ui, p, m, cx))
                    .child(div().flex_1().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(format!(
                        "Starship's {} module · {}",
                        m.id,
                        m.category.label()
                    ))))
                    .when(!fixed, |d| {
                        d.child(
                            toggle(
                                "prompt-module-on",
                                on,
                                ui,
                                self.host(cx, move |h| h.prompt.set_module_on(prompt::module(id).expect("module"), !on)),
                            )
                            .aria_label("Show it"),
                        )
                    }),
            )
            .child(field(
                ui,
                "Side",
                segmented(
                    "prompt-module-side",
                    &[(Side::Left, "Left"), (Side::Right, "Right")],
                    &p.module_side(m),
                    ui,
                    self.set_prompt(cx, move |p, s: Side| {
                        let m = prompt::module(id).expect("module");
                        let default = if m.id == "time" { Side::Right } else { Side::Left };
                        p.edit_module(id, |c| c.side = (s != default).then_some(s));
                    }),
                ),
            ))
            .child(field(ui, "Symbol", symbol_row).child(kit::hint(ui, symbol_note)))
            .child(
                field(
                    ui,
                    "Color",
                    self.color_select("prompt-module-color", "Color", cfg.color.as_deref(), m.color, cx, move |h, c| {
                        h.prompt.edit_module(id, |m| m.color = c)
                    }),
                )
                .child(kit::hint(ui, color_note)),
            );
        let footer = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(kit::hint(ui, "Applied with the other shell changes").mr(px(8.)))
            .child(button("prompt-module-done", "Done", ButtonKind::Primary, ui, self.act(cx, |v, cx| v.close_module_dialog(cx))));
        Some(
            modal(ui, "prompt-module-modal", m.label, body, footer)
                .track_focus(&self.prompt.modal_focus)
                .on_mouse_down(MouseButton::Left, cx.listener(|v, _, _, cx| v.close_module_dialog(cx)))
                .into_any_element(),
        )
    }
}
