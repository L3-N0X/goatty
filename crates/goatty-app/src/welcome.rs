//! The welcome window: goatty's first-run setup (#40). Three short steps,
//! only what matters on day one; everything else has good defaults and
//! lives in the settings window:
//!
//! 1. **Look**: a theme (or follow the desktop's light/dark) and the text size.
//! 2. **Shell**: goatty's built-in tools on the window's host, preselected
//!    when missing, installed with progress (no sudo, pinned checksums),
//!    and the prompt style (icons need a Nerd Font).
//! 3. **Ready**: the few shortcuts worth knowing, links onward.
//!
//! It opens by itself once, on the very first start (no client settings
//! file and no "shown" marker in goatty's state directory), and again from
//! the command palette ("Welcome and setup"). While it's open, a tab
//! connecting doesn't show the install toast (`crate::shell_setup`): this
//! window asks instead.

use std::collections::BTreeSet;

use goatty_config::{HostSettings, PromptStyle, ThemeEntry, store};
use goatty_shell::setup::{self, Status, ToolLocation};
use gpui::{
    App, AppContext, Bounds, Context, FocusHandle, Focusable, FontWeight, Global, IntoElement, KeyBinding, ParentElement, Render,
    SharedString, Styled, Subscription, Window, WindowBounds, WindowHandle, WindowOptions, actions, div, prelude::*, px, rgb, size,
};

use crate::settings::AppSettings;
// The settings window's building blocks, so this window looks the same.
use crate::settings_window::kit::{choice_card as choice, row as settings_row, section as settings_section};
use crate::settings_window::{self, HostLink, HostLinkGlobal, Page};
use crate::shell_setup::{self, Progress};
use crate::ui::controls::{ButtonKind, Tone, badge, banner, button, button_with, keycaps, stepper, toggle};
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};

const CONTEXT: &str = "Welcome";

actions!(welcome, [CloseWelcome]);

pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("escape", CloseWelcome, Some(CONTEXT)), KeyBinding::new("ctrl-w", CloseWelcome, Some(CONTEXT))]
}

struct OpenWindow(Option<WindowHandle<Welcome>>);

impl Global for OpenWindow {}

/// Set between deciding this is the first run and opening the window, so
/// the terminal window's tab (which connects first) doesn't offer the
/// setup in a toast as well.
struct FirstRun;

impl Global for FirstRun {}

/// The welcome window is open (or about to open on the first run).
pub fn is_open(cx: &App) -> bool {
    cx.has_global::<FirstRun>() || cx.try_global::<OpenWindow>().and_then(|w| w.0).is_some_and(|h| h.read(cx).is_ok())
}

/// Before the first terminal window opens: is this the very first start?
/// Then [`open_if_first_run`] opens the welcome window. Someone who already
/// has a settings file isn't new; either way it's decided only once (a
/// marker in goatty's state directory).
pub fn check_first_run(cx: &mut App) {
    let Some(marker) = goatty_config::welcome_marker() else { return };
    if marker.exists() {
        return;
    }
    let new = goatty_config::client_file().is_none_or(|f| !f.exists());
    if let Err(e) = marker.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&marker, "")) {
        // Without the marker it would open on every start: don't open it.
        log::warn!("couldn't write {}: {e}; not showing the welcome window", marker.display());
        return;
    }
    if new {
        cx.set_global(FirstRun);
    }
}

/// After the terminal window opened: show the welcome window on the first run.
pub fn open_if_first_run(cx: &mut App) {
    if cx.has_global::<FirstRun>() {
        open(cx);
        cx.remove_global::<FirstRun>();
    }
}

/// Open the welcome window at its first step, or bring it to the front.
pub fn open(cx: &mut App) {
    if let Some(h) = cx.try_global::<OpenWindow>().and_then(|w| w.0)
        && h.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(780.), px(660.)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(640.), px(540.))),
            app_id: Some("dev.goatty.goatty".into()),
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title("Welcome to goatty");
            cx.new(|cx| Welcome::new(window, cx))
        },
    );
    match opened {
        Ok(h) => cx.set_global(OpenWindow(Some(h))),
        Err(e) => log::error!("couldn't open the welcome window: {e:#}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Step {
    Look,
    Shell,
    Ready,
}

impl Step {
    const ALL: [Step; 3] = [Step::Look, Step::Shell, Step::Ready];

    fn label(self) -> &'static str {
        match self {
            Step::Look => "Look",
            Step::Shell => "Shell",
            Step::Ready => "Ready",
        }
    }
}

pub struct Welcome {
    focus: FocusHandle,
    step: Step,
    /// The host whose shell step 2 sets up (the terminal window's).
    link: Option<HostLink>,
    status: Option<Status>,
    loading: bool,
    error: Option<String>,
    /// Tools to install (ids); starts as what's missing.
    chosen: BTreeSet<String>,
    progress: Option<Progress>,
    /// The host's settings, for the prompt style.
    host: Option<HostSettings>,
    /// Applying a prompt style change.
    saving: bool,
    prompt_error: Option<String>,
    _subs: Vec<Subscription>,
}

impl Focusable for Welcome {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Welcome {
    fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let subs = vec![
            cx.observe_global::<AppSettings>(|_, cx| cx.notify()),
            // The terminal window connects after this opens: pick its host up.
            cx.observe_global::<HostLinkGlobal>(|this, cx| {
                if this.link.is_none() {
                    this.link = cx.try_global::<HostLinkGlobal>().and_then(|g| g.0.clone());
                    if this.link.is_some() && this.step == Step::Shell {
                        this.load(cx);
                    }
                    cx.notify();
                }
            }),
        ];
        Self {
            focus,
            step: Step::Look,
            link: cx.try_global::<HostLinkGlobal>().and_then(|g| g.0.clone()),
            status: None,
            loading: false,
            error: None,
            chosen: BTreeSet::new(),
            progress: None,
            host: None,
            saving: false,
            prompt_error: None,
            _subs: subs,
        }
    }

    fn close(&mut self, _: &CloseWelcome, window: &mut Window, _cx: &mut Context<'_, Self>) {
        window.remove_window();
    }

    fn go(&mut self, step: Step, cx: &mut Context<'_, Self>) {
        self.step = step;
        if step == Step::Shell && self.status.is_none() && !self.loading {
            self.load(cx);
        }
        cx.notify();
    }

    /// Ask the host what its shell has (and its settings, for the prompt).
    fn load(&mut self, cx: &mut Context<'_, Self>) {
        let Some(link) = self.link.clone() else { return };
        self.loading = true;
        self.error = None;
        // This window asks: no toast for the same host later in this run.
        shell_setup::mark_asked(&link, cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let status = link.shell_status()?;
                    if status.should_offer() && !status.offered {
                        // The hint in new shells stays quiet: this window asks.
                        if let Err(e) = link.shell(setup::Request::Offered) {
                            log::warn!("couldn't tell the host the setup was offered: {e:#}");
                        }
                    }
                    let host = link.get().ok().map(|s| store::parse::<HostSettings>(&s.json).value);
                    anyhow::Ok((status, host))
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.loading = false;
                match result {
                    Ok((status, host)) => {
                        v.chosen = status.offer().map(|t| t.id.clone()).collect();
                        v.status = Some(status);
                        v.host = host;
                    }
                    Err(e) => v.error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn install(&mut self, cx: &mut Context<'_, Self>) {
        let (Some(link), Some(status)) = (self.link.clone(), self.status.as_ref()) else { return };
        let progress = Progress::new(status.tools.iter().filter(|t| self.chosen.contains(&t.id)));
        self.progress = Some(progress.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let weak = this.clone();
            let report = move |p: &Progress, cx: &mut App| {
                let _ = weak.update(cx, |v, cx| {
                    v.progress = Some(p.clone());
                    cx.notify();
                });
            };
            shell_setup::install(link, progress, cx, report).await;
        })
        .detach();
    }

    /// Set the host's prompt style (applied as a new generation; running
    /// shells reload).
    fn set_prompt(&mut self, style: PromptStyle, cx: &mut Context<'_, Self>) {
        let (Some(link), Some(host)) = (self.link.clone(), self.host.as_mut()) else { return };
        if host.prompt.style == style || self.saving {
            return;
        }
        host.prompt.style = style;
        self.saving = true;
        self.prompt_error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    // Edit what the host has now, keeping keys this build doesn't know.
                    let loaded = store::parse::<HostSettings>(&link.get()?.json);
                    let mut value = loaded.value;
                    value.prompt.style = style;
                    let applied = link.set(store::to_json(&value, &loaded.unknown), true)?;
                    if applied.applied && !applied.diff.is_empty() {
                        link.reload_shells()?;
                    }
                    anyhow::Ok(())
                })
                .await;
            let _ = this.update(cx, |v, cx| {
                v.saving = false;
                if let Err(e) = result {
                    v.prompt_error = Some(format!("{e:#}"));
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn installing(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| !p.finished)
    }

    fn installed(&self) -> bool {
        self.progress.as_ref().is_some_and(|p| p.finished)
    }

    /// Callback acting on this view.
    fn act(
        &self,
        cx: &Context<'_, Self>,
        f: impl Fn(&mut Self, &mut Context<'_, Self>) + 'static,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        let this = cx.entity().downgrade();
        move |_, cx| {
            let _ = this.update(cx, |v, cx| f(v, cx));
        }
    }

    // ---- Rendering ------------------------------------------------------------

    fn steps(&self, ui: Ui) -> impl IntoElement {
        div().flex().items_center().gap(px(8.)).children(Step::ALL.into_iter().enumerate().map(|(i, s)| {
            let (current, past) = (s == self.step, s < self.step);
            let dot =
                div()
                    .size(px(22.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(text::CAPTION))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(current, |d| d.bg(ui.accent).text_color(ui.on_accent))
                    .when(past, |d| d.bg(ui.accent_soft))
                    .when(!current && !past, |d| d.bg(ui.field).text_color(ui.muted))
                    .map(|d| {
                        if past {
                            d.child(Icon::Check.svg().size(px(12.)).text_color(ui.accent_text))
                        } else {
                            d.child((i + 1).to_string())
                        }
                    });
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when(i > 0, |d| {
                    d.child(div().w(px(28.)).h(px(2.)).rounded_full().bg(if current || past { ui.accent_soft } else { ui.divider }))
                })
                .child(dot)
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .font_weight(if current { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(if current { ui.text } else { ui.muted })
                        .child(s.label()),
                )
        }))
    }

    fn title(ui: Ui, title: impl Into<SharedString>, subtitle: impl Into<SharedString>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(title.into()))
            .child(div().text_size(px(text::BODY)).text_color(ui.muted).child(subtitle.into()))
    }

    fn look(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let app = cx.global::<AppSettings>();
        let theme = app.file.value.theme.clone();
        let font_size = app.file.value.font.size;
        let themes = app.themes.all();
        let cards = themes.into_iter().map(|t| {
            let selected = if theme.follow_system { t.id == theme.light || t.id == theme.dark } else { t.id == theme.name };
            self.theme_card(ui, t, selected, cx)
        });
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(Self::title(
                ui,
                "Welcome to goatty",
                "A fast terminal with a shell that's ready to go. Two quick choices, then you're set.",
            ))
            .child(
                settings_section(ui, "Theme")
                    .child(
                        settings_row(ui, "Follow the system")
                            .desc("A light theme by day, a dark one by night, as your desktop switches")
                            .control(toggle("follow-system", theme.follow_system, ui, |_, cx| {
                                AppSettings::update(cx, |s| s.theme.follow_system = !s.theme.follow_system)
                            })),
                    )
                    .child(div().flex().flex_wrap().gap(px(10.)).p(px(14.)).children(cards)),
            )
            .child(settings_section(ui, "Text").child(settings_row(ui, "Size").desc("Ctrl+= and Ctrl+- change it in any window").control(
                stepper(
                    "font-size",
                    format!("{font_size:.0} px"),
                    ui,
                    |_, cx| AppSettings::update(cx, |s| s.font.size = (s.font.size - 1.0).max(goatty_config::FontSettings::MIN_SIZE)),
                    |_, cx| AppSettings::update(cx, |s| s.font.size = (s.font.size + 1.0).min(goatty_config::FontSettings::MAX_SIZE)),
                ),
            )))
    }

    fn theme_card(&self, ui: Ui, t: ThemeEntry, selected: bool, cx: &Context<'_, Self>) -> impl IntoElement {
        let col = t.colors;
        let c = |x: goatty_config::Color| -> gpui::Hsla { rgb(x.0).into() };
        let (id, dark) = (t.id.clone(), col.is_dark());
        div()
            .id(SharedString::from(format!("theme-{}", t.id)))
            .w(px(128.))
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
                    .h(px(46.))
                    .bg(c(col.background))
                    .px(px(10.))
                    .py(px(8.))
                    .flex()
                    .flex_col()
                    .justify_between()
                    .font_family(crate_mono(cx))
                    .text_size(px(11.))
                    .child(
                        div()
                            .flex()
                            .child(div().text_color(c(col.ansi[2])).child("❯ "))
                            .child(div().text_color(c(col.foreground)).child("ls")),
                    )
                    .child(div().flex().gap(px(3.)).children(col.ansi[1..7].iter().map(|x| div().size(px(9.)).rounded_full().bg(c(*x))))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .h(px(28.))
                    .px(px(9.))
                    .bg(ui.card)
                    .text_size(px(text::CAPTION))
                    .text_color(if selected { ui.accent_text } else { ui.text })
                    .when(selected, |d| d.child(Icon::Check.svg().size(px(12.)).text_color(ui.accent_text)))
                    .child(div().truncate().child(SharedString::from(t.name.clone()))),
            )
            .on_click(move |_, _, cx| {
                let id = id.clone();
                AppSettings::update(cx, move |s| match (s.theme.follow_system, dark) {
                    (false, _) => s.theme.name = id.clone(),
                    (true, true) => s.theme.dark = id.clone(),
                    (true, false) => s.theme.light = id.clone(),
                });
            })
    }

    fn shell(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let host = match &self.link {
            Some(l) if !l.is_local() => format!(" on {}", l.label()),
            _ => String::new(),
        };
        let mut page = div().flex().flex_col().gap(px(24.)).child(Self::title(
            ui,
            format!("Your shell{host}"),
            "goatty runs zsh with suggestions as you type, syntax colors, a clear prompt and searchable history. Your own dotfiles stay as they are.",
        ));
        if self.link.is_none() {
            return page.child(loading_card(ui, "Waiting for the terminal window to connect…"));
        }
        let Some(status) = &self.status else {
            return match &self.error {
                Some(e) => page.child(banner(ui, Tone::Danger, "Couldn't check the shell", vec![e.clone()])).child(div().flex().child(
                    button_with("retry", Some(Icon::Refresh), "Try again", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.load(cx))),
                )),
                None => page.child(loading_card(ui, "Checking what's installed…")),
            };
        };
        if let Some(why) = &status.unmanaged {
            return page.child(banner(
                ui,
                Tone::Neutral,
                format!("{} starts as is", status.program),
                vec![
                    why.clone(),
                    "goatty's shell setup is for zsh. Install zsh to get it; everything else in goatty works either way.".into(),
                ],
            ));
        }
        let busy = self.installing() || self.installed();
        // The default tools first; optional ones (fzf) after them.
        let mut tools: Vec<_> = status.tools.iter().collect();
        tools.sort_by_key(|t| !t.default_install);
        let rows = tools.into_iter().map(|t| {
            let mut r = settings_row(ui, t.name.clone());
            r = match (&t.location, &t.unavailable) {
                (ToolLocation::System { .. }, _) => {
                    r.desc(format!("{} · already on your system", t.summary)).control(badge("Installed", Tone::Success, ui))
                }
                (ToolLocation::Managed { .. }, _) => {
                    r.desc(format!("{} · installed by goatty", t.summary)).control(badge("Installed", Tone::Success, ui))
                }
                (ToolLocation::Missing, Some(why)) => r.desc(why.clone()).control(badge("Not available", Tone::Neutral, ui)),
                (ToolLocation::Missing, None) => {
                    let on = self.chosen.contains(&t.id);
                    let id = t.id.clone();
                    let extra = if t.default_install { "" } else { " · optional" };
                    r.desc(format!("{} · {}{extra}", t.summary, setup::size(t.download))).disabled(busy).control(toggle(
                        SharedString::from(format!("tool-{}", t.id)),
                        on,
                        ui,
                        self.act(cx, move |v, cx| {
                            if !v.chosen.remove(&id) {
                                v.chosen.insert(id.clone());
                            }
                            cx.notify();
                        }),
                    ))
                }
            };
            r
        });
        page = page.child(
            settings_section(ui, "Tools")
                .desc("Each one is optional: goatty works without them")
                .children(rows)
                .footer("Downloads go to ~/.local/share/goatty and are checked against the checksums goatty pins. No sudo."),
        );
        if let Some(p) = &self.progress {
            page = page.child(settings_window::progress_card(ui, p));
        }
        if let Some(h) = &self.host {
            let style = h.prompt.style;
            let card = |s: PromptStyle, label: &'static str, sample: Vec<(&'static str, usize)>| {
                let selected = style == s;
                let colors = cx.global::<AppSettings>().colors;
                let line = div()
                    .h(px(64.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .bg(gpui::Hsla::from(rgb(colors.background.0)))
                    .font_family(crate_mono(cx))
                    .text_size(px(13.))
                    .children(sample.into_iter().map(|(t, i)| {
                        let c = if i == 99 { colors.foreground } else { colors.ansi[i] };
                        div().text_color(gpui::Hsla::from(rgb(c.0))).child(t)
                    }));
                choice(ui, SharedString::from(format!("prompt-{label}")), label, selected, line)
                    .on_click(cx.listener(move |v: &mut Self, _: &gpui::ClickEvent, _, cx| v.set_prompt(s, cx)))
            };
            let mut sec = settings_section(ui, "Prompt").desc("Pick the one that looks right: the icons need a Nerd Font");
            if let Some(e) = &self.prompt_error {
                sec = sec.footer(format!("Couldn't change it: {e}"));
            }
            page = page.child(
                sec.child(
                    div()
                        .flex()
                        .gap(px(12.))
                        .p(px(14.))
                        .child(card(
                            PromptStyle::Rich,
                            "With icons",
                            vec![("~/projects", 6), (" on ", 99), ("\u{e0a0} main", 5), (" ❯", 2)],
                        ))
                        .child(card(PromptStyle::Minimal, "Plain", vec![("~/projects", 6), (" main", 5), (" ❯", 2)])),
                ),
            );
        }
        page
    }

    fn ready(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let overrides = cx.global::<AppSettings>().client.keybindings.clone();
        let key = |action: &str| goatty_config::keys::keys_for(action, &overrides).into_iter().next();
        let below = match key("split_down") {
            Some(k) => format!("Side by side; {} splits below", crate::ui::controls::pretty_keys(&k)),
            None => "Side by side, or below".into(),
        };
        let shortcut = |label: &'static str, desc: &str, action: &str| {
            settings_row(ui, label).desc(desc.to_owned()).control(match key(action) {
                Some(k) => keycaps(&k, ui).into_any_element(),
                None => badge("Not bound", Tone::Neutral, ui).into_any_element(),
            })
        };
        let installed: Vec<&str> =
            self.progress.iter().flat_map(|p| p.tools.iter().filter(|t| t.2 == shell_setup::Step::Done).map(|t| t.1.as_str())).collect();
        let subtitle = if installed.is_empty() {
            "That's all. These are the shortcuts worth knowing; the rest is in the command palette.".to_string()
        } else {
            format!(
                "{} {} ready in your shells. These are the shortcuts worth knowing.",
                shell_setup::names(&installed),
                if installed.len() == 1 { "is" } else { "are" }
            )
        };
        div()
            .flex()
            .flex_col()
            .gap(px(24.))
            .child(Self::title(ui, "You're set", subtitle))
            .child(
                settings_section(ui, "Shortcuts")
                    .child(shortcut("Command palette", "Every command, setting and host, by name", "command_palette"))
                    .child(shortcut("New tab", "In the current tab's directory", "new_tab"))
                    .child(shortcut("Split the pane", &below, "split_right"))
                    .child(shortcut("Connect to a host", "Your SSH hosts, with goatty's shell there too", "connect_to_host"))
                    .child(shortcut("Settings", "Everything else, with a search", "open_settings")),
            )
            .child(
                settings_section(ui, "More")
                    .child(settings_row(ui, "Settings").icon(Icon::Sliders).desc("Fonts, tabs, prompt, history, plugins").on_click(
                        "more-settings",
                        |_, window, cx| {
                            settings_window::open(cx);
                            window.remove_window();
                        },
                    ))
                    .child(
                        settings_row(ui, "Diagnostics").icon(Icon::Activity).desc("What the shell loads, and how fast it starts").on_click(
                            "more-diagnostics",
                            |_, window, cx| {
                                settings_window::open_at(cx, Some(Page::Diagnostics));
                                window.remove_window();
                            },
                        ),
                    ),
            )
    }

    fn footer(&self, ui: Ui, cx: &Context<'_, Self>) -> impl IntoElement {
        let back = match self.step {
            Step::Look => None,
            Step::Shell => Some(Step::Look),
            Step::Ready => Some(Step::Shell),
        };
        let primary = match self.step {
            Step::Look => button("next", "Continue", ButtonKind::Primary, ui, self.act(cx, |v, cx| v.go(Step::Shell, cx))),
            Step::Shell if self.installing() => {
                let line = self.progress.as_ref().map(Progress::status_line).unwrap_or_default();
                button_with("next", Some(Icon::Loader), line, ButtonKind::Primary, ui, |_, _| {}).opacity(0.7)
            }
            Step::Shell if !self.chosen.is_empty() && !self.installed() && self.status.as_ref().is_some_and(|s| s.managed) => {
                let s = self.status.as_ref();
                let bytes: u64 = s.map(|s| s.tools.iter().filter(|t| self.chosen.contains(&t.id)).map(|t| t.download).sum()).unwrap_or(0);
                let n = self.chosen.len();
                let label = format!("Install {n} {} · {}", if n == 1 { "tool" } else { "tools" }, setup::size(bytes));
                button_with("next", Some(Icon::Download), label, ButtonKind::Primary, ui, self.act(cx, |v, cx| v.install(cx)))
            }
            Step::Shell => button("next", "Continue", ButtonKind::Primary, ui, self.act(cx, |v, cx| v.go(Step::Ready, cx))),
            Step::Ready => button("next", "Start using goatty", ButtonKind::Primary, ui, |window, _| window.remove_window()),
        };
        // "Not now" on the shell step: go on without installing.
        let skip_install = self.step == Step::Shell && !self.chosen.is_empty() && !self.installing() && !self.installed();
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(28.))
            .py(px(16.))
            .border_t_1()
            .border_color(ui.divider)
            .bg(ui.sidebar)
            .when(self.step != Step::Ready, |d| {
                d.child(button("skip", "Skip setup", ButtonKind::Ghost, ui, |window, _| window.remove_window()))
            })
            .child(div().flex_1())
            .when_some(back, |d, step| d.child(button("back", "Back", ButtonKind::Normal, ui, self.act(cx, move |v, cx| v.go(step, cx)))))
            .when(skip_install, |d| {
                d.child(button("not-now", "Not now", ButtonKind::Normal, ui, self.act(cx, |v, cx| v.go(Step::Ready, cx))))
            })
            .child(primary)
    }
}

impl Render for Welcome {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let cx: &Context<'_, Self> = cx;
        let body = match self.step {
            Step::Look => self.look(ui, cx).into_any_element(),
            Step::Shell => self.shell(ui, cx).into_any_element(),
            Step::Ready => self.ready(ui, cx).into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(ui.bg)
            .text_color(ui.text)
            .font_family(cx.global::<AppSettings>().ui_font.clone())
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::close))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .px(px(28.))
                    .pt(px(20.))
                    .pb(px(12.))
                    .child(
                        div()
                            .size(px(30.))
                            .rounded(px(9.))
                            .bg(ui.accent)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(Icon::Terminal.svg().size(px(17.)).text_color(ui.on_accent)),
                    )
                    .child(div().flex_1())
                    .child(self.steps(ui)),
            )
            .child(
                div().id("welcome-page").flex_1().overflow_y_scroll().child(
                    div().flex().justify_center().px(px(28.)).child(div().w_full().max_w(px(700.)).pt(px(12.)).pb(px(32.)).child(body)),
                ),
            )
            .child(self.footer(ui, cx))
    }
}

fn crate_mono(cx: &App) -> SharedString {
    cx.global::<AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into()).into()
}

fn loading_card(ui: Ui, t: &'static str) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .p(px(18.))
        .rounded(px(radius::CARD))
        .bg(ui.card)
        .border_1()
        .border_color(ui.divider)
        .child(Icon::Loader.svg().size(px(18.)).text_color(ui.accent_text))
        .child(div().text_size(px(text::BODY)).text_color(ui.muted).child(t))
}
