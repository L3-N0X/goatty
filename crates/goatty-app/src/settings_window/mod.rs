//! The settings window: every setting in the GUI (Stage 4).
//!
//! Client settings (appearance, shortcuts, workspace) apply live and are
//! saved right away. Host settings (shell, prompt, history) belong to the
//! connected goattyd's host: edits are staged as a draft, can be previewed
//! as a diff of the generated shell configuration, and are applied as a new
//! generation (validated first, rollback-able). See `docs/profiles.md`.

mod host;
mod pages;

use goatty_config::{HostSettings, Loaded, store};
use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, Global, KeyBinding, Subscription, Window, WindowBounds, WindowHandle,
    WindowOptions, actions, px, size,
};

pub use host::{GenRow, HostLink, HostLinkGlobal};

use crate::settings::AppSettings;
use crate::ui::text_input::{TextInput, TextInputEvent};

pub const CONTEXT: &str = "SettingsWindow";

actions!(settings_window, [CloseSettings]);

pub fn bindings() -> Vec<KeyBinding> {
    vec![KeyBinding::new("ctrl-w", CloseSettings, Some(CONTEXT)), KeyBinding::new("ctrl-,", CloseSettings, Some(CONTEXT))]
}

/// The open settings window, if any (there's at most one).
struct OpenWindow(Option<WindowHandle<SettingsWindow>>);

impl Global for OpenWindow {}

/// Open the settings window, or bring it to the front.
pub fn open(cx: &mut App) {
    open_at(cx, None);
}

/// Open the settings window at `page` (or where it was).
pub fn open_at(cx: &mut App, page: Option<Page>) {
    if let Some(handle) = cx.try_global::<OpenWindow>().and_then(|w| w.0)
        && handle
            .update(cx, |view, window, cx| {
                window.activate_window();
                if let Some(p) = page {
                    view.page = p;
                    cx.notify();
                }
            })
            .is_ok()
    {
        return;
    }
    let bounds = Bounds::centered(None, size(px(860.), px(640.)), cx);
    let handle: anyhow::Result<WindowHandle<SettingsWindow>> = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            app_id: Some("dev.goatty.goatty".into()),
            ..Default::default()
        },
        |window, cx| {
            window.set_window_title("goatty settings");
            cx.new(|cx| {
                let mut w = SettingsWindow::new(window, cx);
                if let Some(p) = page {
                    w.page = p;
                }
                w
            })
        },
    );
    match handle {
        Ok(h) => cx.set_global(OpenWindow(Some(h))),
        Err(e) => log::error!("couldn't open the settings window: {e:#}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Appearance,
    Shell,
    Prompt,
    History,
    Shortcuts,
    Workspace,
    Profile,
}

impl Page {
    pub const ALL: [Page; 7] =
        [Page::Appearance, Page::Shell, Page::Prompt, Page::History, Page::Shortcuts, Page::Workspace, Page::Profile];

    pub fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Shell => "Shell",
            Page::Prompt => "Prompt",
            Page::History => "History",
            Page::Shortcuts => "Shortcuts",
            Page::Workspace => "Workspace",
            Page::Profile => "Profile",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.title().eq_ignore_ascii_case(name))
    }

    /// Edits the connected host's settings (staged, then applied).
    pub fn is_host(self) -> bool {
        matches!(self, Page::Shell | Page::Prompt | Page::History)
    }
}

/// The host settings as loaded from the host.
#[derive(Debug, Clone)]
pub struct HostView {
    pub loaded: Loaded<HostSettings>,
    /// Problems the host reported with its settings file.
    pub issues: Vec<String>,
    pub env_overrides: Vec<String>,
    pub generation: Option<u32>,
}

/// What the last preview/apply/rollback said.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub ok: bool,
    pub lines: Vec<String>,
    /// (file name, unified diff)
    pub diff: Vec<(String, String)>,
}

pub struct SettingsWindow {
    focus: FocusHandle,
    page: Page,
    link: Option<HostLink>,
    host: Option<HostView>,
    /// Edited host settings not yet applied (`None`: no changes).
    draft: Option<HostSettings>,
    host_error: Option<String>,
    busy: bool,
    outcome: Option<Outcome>,
    show_diff: bool,
    generations: Vec<GenRow>,
    font_filter: Entity<TextInput>,
    shell_path: Entity<TextInput>,
    show_all_fonts: bool,
    /// Shortcut being recorded: the next keystroke binds to this action.
    recording: Option<&'static str>,
    _subs: Vec<Subscription>,
}

impl Focusable for SettingsWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let font_filter = cx.new(|cx| TextInput::new("Filter fonts…", "", cx));
        let shell_path = cx.new(|cx| TextInput::new("/path/to/shell, then Enter", "", cx));
        let subs = vec![
            cx.observe_global::<AppSettings>(|_, cx| cx.notify()),
            cx.subscribe(&font_filter, |_, _, _: &TextInputEvent, cx| cx.notify()),
            cx.subscribe(&shell_path, |this, _, ev: &TextInputEvent, cx| {
                if let TextInputEvent::Submitted(text) = ev {
                    let text = text.trim().to_string();
                    this.edit_host(cx, |h| h.shell.program = (!text.is_empty()).then_some(text));
                }
            }),
        ];
        let mut this = Self {
            focus,
            page: Page::Appearance,
            link: cx.try_global::<HostLinkGlobal>().and_then(|g| g.0.clone()),
            host: None,
            draft: None,
            host_error: None,
            busy: false,
            outcome: None,
            show_diff: false,
            generations: Vec::new(),
            font_filter,
            shell_path,
            show_all_fonts: false,
            recording: None,
            _subs: subs,
        };
        this.load_host(cx);
        this
    }

    /// The host settings being shown: the draft, else what the host has.
    pub fn host_value(&self) -> Option<&HostSettings> {
        self.draft.as_ref().or(self.host.as_ref().map(|h| &h.loaded.value))
    }

    fn close(&mut self, _: &CloseSettings, window: &mut Window, _cx: &mut Context<'_, Self>) {
        window.remove_window();
    }

    /// (Re)load the host settings and generations off the UI thread.
    pub fn load_host(&mut self, cx: &mut Context<'_, Self>) {
        let Some(link) = self.link.clone() else {
            self.host_error = Some("No host to configure.".into());
            return;
        };
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let result =
                cx.background_executor().spawn(async move { link.get().map(|s| (s, link.generations().unwrap_or_default())) }).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok((snap, gens)) => {
                        // The host's issues are about its file; re-parsing what it
                        // sent can only add "unknown key" ones (a newer daemon).
                        let loaded = store::parse::<HostSettings>(&snap.json);
                        let mut issues = snap.issues;
                        for i in loaded.issues.iter().map(ToString::to_string) {
                            if !issues.contains(&i) {
                                issues.push(i);
                            }
                        }
                        let program = loaded.value.shell.program.clone().unwrap_or_default();
                        view.shell_path.update(cx, |input, cx| input.set_text(program, cx));
                        view.host = Some(HostView { loaded, issues, env_overrides: snap.env_overrides, generation: snap.generation });
                        view.generations = gens;
                        view.host_error = None;
                    }
                    Err(e) => view.host_error = Some(format!("{e:#}")),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Change the draft host settings.
    pub fn edit_host(&mut self, cx: &mut Context<'_, Self>, f: impl FnOnce(&mut HostSettings)) {
        let Some(base) = self.host_value().cloned() else { return };
        let mut draft = base;
        f(&mut draft);
        self.draft = match &self.host {
            Some(h) if h.loaded.value == draft => None,
            _ => Some(draft),
        };
        self.outcome = None;
        self.show_diff = false;
        cx.notify();
    }

    fn draft_json(&self) -> Option<String> {
        let unknown = self.host.as_ref().map(|h| h.loaded.unknown.clone()).unwrap_or_default();
        self.draft.as_ref().map(|d| store::to_json(d, &unknown))
    }

    /// Ask the host what applying the draft would change.
    pub fn preview(&mut self, cx: &mut Context<'_, Self>) {
        self.send_draft(false, cx);
    }

    /// Validate, activate and save the draft on the host.
    pub fn apply(&mut self, cx: &mut Context<'_, Self>) {
        self.send_draft(true, cx);
    }

    fn send_draft(&mut self, apply: bool, cx: &mut Context<'_, Self>) {
        let (Some(link), Some(json)) = (self.link.clone(), self.draft_json()) else { return };
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { link.set(json, apply) }).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                match result {
                    Ok(a) => {
                        let mut lines = Vec::new();
                        if a.applied {
                            lines.push(match a.generation {
                                Some(g) => format!("Applied as generation {g}. New sessions use it; running shells: goatty-reload."),
                                None => "Applied.".into(),
                            });
                        } else if a.diff.is_empty() {
                            lines.push("No change to the generated shell configuration.".into());
                        } else {
                            lines.push(format!("{} generated file(s) would change.", a.diff.len()));
                        }
                        lines.extend(a.issues.iter().map(|i| format!("⚠ {i}")));
                        view.show_diff = !apply;
                        view.outcome = Some(Outcome { ok: true, lines, diff: a.diff });
                        if a.applied {
                            view.draft = None;
                            view.load_host(cx);
                        }
                    }
                    Err(e) => view.outcome = Some(Outcome { ok: false, lines: vec![format!("{e:#}")], diff: Vec::new() }),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn discard(&mut self, cx: &mut Context<'_, Self>) {
        self.draft = None;
        self.outcome = None;
        self.show_diff = false;
        if let Some(h) = &self.host {
            let program = h.loaded.value.shell.program.clone().unwrap_or_default();
            self.shell_path.update(cx, |input, cx| input.set_text(program, cx));
        }
        cx.notify();
    }

    pub fn rollback(&mut self, generation: u32, cx: &mut Context<'_, Self>) {
        let Some(link) = self.link.clone() else { return };
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { link.rollback(Some(generation)) }).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                view.outcome = Some(match result {
                    Ok(_) => Outcome {
                        ok: true,
                        lines: vec![format!("Generation {generation} is active again, with its settings.")],
                        diff: Vec::new(),
                    },
                    Err(e) => Outcome { ok: false, lines: vec![format!("{e:#}")], diff: Vec::new() },
                });
                view.draft = None;
                view.load_host(cx);
            });
        })
        .detach();
    }

    /// Export client + host settings to a file the user picks.
    pub fn export_profile(&mut self, cx: &mut Context<'_, Self>) {
        let client = cx.global::<AppSettings>().file.value.clone();
        let Some(host) = self.host.as_ref().map(|h| h.loaded.value.clone()) else { return };
        let dir = dirs_home();
        let rx = cx.prompt_for_new_path(&dir, Some("goatty-profile.json"));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(path))) = rx.await else { return };
            let text = goatty_config::profile::export(&client, &host);
            let result = std::fs::write(&path, text);
            let _ = this.update(cx, |view, cx| {
                view.outcome = Some(match result {
                    Ok(()) => Outcome { ok: true, lines: vec![format!("Exported to {}", path.display())], diff: Vec::new() },
                    Err(e) => Outcome { ok: false, lines: vec![format!("Couldn't write {}: {e}", path.display())], diff: Vec::new() },
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Import a profile: client settings apply now; host settings become a
    /// draft to preview and apply.
    pub fn import_profile(&mut self, cx: &mut Context<'_, Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import goatty profile".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let parsed = std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| goatty_config::profile::import(&t));
            let _ = this.update(cx, |view, cx| {
                match parsed {
                    Ok((client, host)) => {
                        let mut lines = vec![format!("Imported {}.", path.display())];
                        if let Some(c) = client {
                            lines.extend(c.issues.iter().map(|i| format!("⚠ client: {i}")));
                            AppSettings::set(cx, c.value);
                            lines.push("Appearance, shortcuts and workspace settings are applied.".into());
                        }
                        if let Some(h) = host {
                            lines.extend(h.issues.iter().map(|i| format!("⚠ host: {i}")));
                            view.edit_host(cx, |d| *d = h.value);
                            lines.push("Shell, prompt and history settings are staged: preview and apply them below.".into());
                        }
                        view.outcome = Some(Outcome { ok: true, lines, diff: Vec::new() });
                    }
                    Err(e) => view.outcome = Some(Outcome { ok: false, lines: vec![format!("Not imported: {e}")], diff: Vec::new() }),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("HOME").map(Into::into).unwrap_or_else(|| "/".into())
}

/// Connection-or-local, for the window: set once by `app::run`.
pub fn set_host_link(cx: &mut App, link: Option<HostLink>) {
    cx.set_global(HostLinkGlobal(link));
}
