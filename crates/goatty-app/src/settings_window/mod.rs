//! The settings window: every setting in the GUI (Stage 4).
//!
//! Client settings (appearance, shortcuts, workspace) apply live and are
//! saved right away. Host settings (shell, prompt, history) belong to the
//! connected goattyd's host: edits are staged as a draft, can be previewed
//! as a diff of the generated shell configuration, and are applied as a new
//! generation (validated first, rollback-able). See `docs/profiles.md`.

mod host;
mod pages;
mod plugins;

use std::time::Duration;

use goatty_config::{Color, ColorSlot, HostSettings, Loaded, store};
use gpui::{
    App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, Global, KeyBinding, Subscription, Window, WindowBounds, WindowHandle,
    WindowOptions, actions, px, size,
};

pub use host::{GenRow, HostLink, HostLinkGlobal};

use crate::settings::AppSettings;
use crate::ui::color_picker::{ColorPicker, ColorPickerEvent};
use crate::ui::text_input::{TextInput, TextInputEvent};

pub const CONTEXT: &str = "SettingsWindow";

/// A color that was typed or dragged is saved this long after the last change
/// (letting go of the mouse or pressing Enter saves right away).
const COLOR_SAVE_DELAY: Duration = Duration::from_millis(600);

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
                if page == Some(Page::Plugins) {
                    view.load_plugins(false, cx);
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
    Plugins,
    Workspace,
    Profile,
}

impl Page {
    pub const ALL: [Page; 8] =
        [Page::Appearance, Page::Shell, Page::Prompt, Page::History, Page::Plugins, Page::Shortcuts, Page::Workspace, Page::Profile];

    pub fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Shell => "Shell",
            Page::Prompt => "Prompt",
            Page::History => "History",
            Page::Plugins => "Plugins",
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
        matches!(self, Page::Shell | Page::Prompt | Page::History | Page::Plugins)
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

/// Which theme setting a click in the gallery changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeTarget {
    /// `theme.name` (not following the system).
    Fixed,
    Light,
    Dark,
}

/// What the last preview/apply/rollback said.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub ok: bool,
    pub lines: Vec<String>,
    /// (file name, unified diff)
    pub diff: Vec<(String, String)>,
    /// The shell configuration changed: offer to reload running shells.
    pub reload: bool,
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
    /// Theme gallery target when following the system.
    pub theme_target: ThemeTarget,
    /// The color being edited in the theme editor.
    pub edit_slot: ColorSlot,
    picker: Entity<ColorPicker>,
    /// A color change applied live but not yet written to settings.json.
    pending_color: Option<(ColorSlot, Color)>,
    color_save_gen: u64,
    theme_name: Entity<TextInput>,
    /// Shortcut being recorded: the next keystroke binds to this action.
    recording: Option<&'static str>,
    /// What the last recorded shortcut replaced or takes from the shell:
    /// (action it was recorded for, message).
    shortcut_note: Option<(&'static str, String)>,
    plugins: plugins::PluginsPage,
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
        let picker = cx.new(|cx| ColorPicker::new(cx.global::<AppSettings>().colors.get(ColorSlot::Background), cx));
        let theme_name = cx.new(|cx| TextInput::new("Name for a new theme", "", cx));
        let subs = vec![
            // The colors can change underneath the editor (theme switch,
            // desktop light/dark, outside edits): keep the picker current.
            cx.observe_global::<AppSettings>(|this, cx| this.sync_picker(cx)),
            cx.subscribe(&font_filter, |_, _, _: &TextInputEvent, cx| cx.notify()),
            cx.subscribe(&picker, |this, _, ev: &ColorPickerEvent, cx| match *ev {
                ColorPickerEvent::Changed(c) => this.color_changed(c, cx),
                ColorPickerEvent::Committed(_) => this.flush_color(cx),
            }),
            // Closing the window right after a change must not lose it.
            cx.on_release(|this, cx| this.flush_color(cx)),
            cx.subscribe(&theme_name, |this, _, ev: &TextInputEvent, cx| {
                if let TextInputEvent::Submitted(_) = ev {
                    this.save_theme_as(cx);
                }
            }),
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
            theme_target: if cx.global::<AppSettings>().file.value.theme.follow_system {
                if cx.global::<AppSettings>().system_dark { ThemeTarget::Dark } else { ThemeTarget::Light }
            } else {
                ThemeTarget::Fixed
            },
            edit_slot: ColorSlot::Background,
            picker,
            pending_color: None,
            color_save_gen: 0,
            theme_name,
            recording: None,
            shortcut_note: None,
            plugins: plugins::PluginsPage::default(),
            _subs: subs,
        };
        this.load_host(cx);
        this.load_plugins(false, cx);
        this.select_slot(ColorSlot::Background, cx);
        this
    }

    // ---- Themes -----------------------------------------------------------------

    /// Point the current gallery target at theme `id`.
    pub fn choose_theme(&mut self, id: String, cx: &mut Context<'_, Self>) {
        self.flush_color(cx);
        let target = self.theme_target;
        AppSettings::update(cx, |s| match target {
            ThemeTarget::Fixed => s.theme.name = id,
            ThemeTarget::Light => s.theme.light = id,
            ThemeTarget::Dark => s.theme.dark = id,
        });
        self.select_slot(self.edit_slot, cx);
    }

    /// Edit `slot`: open its current color in the picker.
    pub fn select_slot(&mut self, slot: ColorSlot, cx: &mut Context<'_, Self>) {
        self.flush_color(cx);
        self.edit_slot = slot;
        let color = cx.global::<AppSettings>().colors.get(slot);
        self.picker.update(cx, |p, cx| p.open(color, cx));
        cx.notify();
    }

    /// The colors changed under the picker: follow them.
    fn sync_picker(&mut self, cx: &mut Context<'_, Self>) {
        let color = cx.global::<AppSettings>().colors.get(self.edit_slot);
        self.picker.update(cx, |p, cx| p.sync(color, cx));
    }

    /// The picker moved to `color`: show it live, and save it once the changes stop.
    fn color_changed(&mut self, color: Color, cx: &mut Context<'_, Self>) {
        let slot = self.edit_slot;
        self.pending_color = Some((slot, color));
        AppSettings::preview(cx, |s| s.theme.overrides.set(slot, Some(color)));
        self.color_save_gen += 1;
        let generation = self.color_save_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COLOR_SAVE_DELAY).await;
            let _ = this.update(cx, |this, cx| {
                if this.color_save_gen == generation {
                    this.flush_color(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Write a live color change to settings.json.
    fn flush_color(&mut self, cx: &mut App) {
        if let Some((slot, color)) = self.pending_color.take() {
            AppSettings::update(cx, |s| s.theme.overrides.set(slot, Some(color)));
        }
    }

    pub fn revert_color(&mut self, cx: &mut Context<'_, Self>) {
        self.flush_color(cx);
        let slot = self.edit_slot;
        AppSettings::update(cx, |s| s.theme.overrides.set(slot, None));
        self.select_slot(slot, cx);
    }

    pub fn reset_colors(&mut self, cx: &mut Context<'_, Self>) {
        self.flush_color(cx);
        AppSettings::update(cx, |s| s.theme.overrides = Default::default());
        self.select_slot(self.edit_slot, cx);
    }

    fn message(&mut self, ok: bool, line: String, cx: &mut Context<'_, Self>) {
        self.outcome = Some(Outcome { ok, lines: vec![line], diff: Vec::new(), reload: false });
        cx.notify();
    }

    /// Save the colors on screen as a new user theme and switch to it.
    pub fn save_theme_as(&mut self, cx: &mut Context<'_, Self>) {
        self.flush_color(cx);
        let name = self.theme_name.read(cx).text().trim().to_string();
        if name.is_empty() {
            return self.message(false, "Give the new theme a name first.".into(), cx);
        }
        let app = cx.global::<AppSettings>();
        let (lib, colors) = (app.themes.clone(), app.colors);
        let id = lib.unique_id(&name);
        match lib.save(&id, &name, &colors) {
            Ok(path) => {
                AppSettings::reload_themes(cx);
                AppSettings::update(cx, |s| s.theme.overrides = Default::default());
                self.choose_theme(id, cx);
                self.theme_name.update(cx, |i, cx| i.set_text("", cx));
                self.message(true, format!("Saved {name:?} to {}.", path.display()), cx);
            }
            Err(e) => self.message(false, format!("Couldn't save the theme: {e}"), cx),
        }
    }

    /// Write the colors on screen into the active user theme.
    pub fn update_theme(&mut self, id: String, cx: &mut Context<'_, Self>) {
        self.flush_color(cx);
        let app = cx.global::<AppSettings>();
        let (lib, colors) = (app.themes.clone(), app.colors);
        let Some(entry) = lib.get(&id).filter(|t| t.user) else { return };
        match lib.save(&id, &entry.name, &colors) {
            Ok(_) => {
                AppSettings::reload_themes(cx);
                AppSettings::update(cx, |s| s.theme.overrides = Default::default());
                self.message(true, format!("Updated {:?}.", entry.name), cx);
            }
            Err(e) => self.message(false, format!("Couldn't save the theme: {e}"), cx),
        }
    }

    /// Delete a user theme; settings pointing at it fall back to the defaults.
    pub fn delete_theme(&mut self, id: String, cx: &mut Context<'_, Self>) {
        self.flush_color(cx);
        let lib = cx.global::<AppSettings>().themes.clone();
        match lib.delete(&id) {
            Ok(_) => {
                AppSettings::reload_themes(cx);
                let d = goatty_config::ThemeSettings::default();
                AppSettings::update(cx, |s| {
                    // A user theme may have replaced a built-in of the same id:
                    // then the built-in is back and the setting can stay.
                    if goatty_config::find_theme(&id).is_none() {
                        if s.theme.name == id {
                            s.theme.name = d.name.clone();
                        }
                        if s.theme.light == id {
                            s.theme.light = d.light.clone();
                        }
                        if s.theme.dark == id {
                            s.theme.dark = d.dark.clone();
                        }
                    }
                });
                self.select_slot(self.edit_slot, cx);
                self.message(true, format!("Deleted theme {id:?}."), cx);
            }
            Err(e) => self.message(false, format!("Couldn't delete the theme: {e}"), cx),
        }
    }

    /// Import color schemes (base16, iTerm2, Windows Terminal, goatty) as
    /// user themes and switch to the first.
    pub fn import_theme(&mut self, cx: &mut Context<'_, Self>) {
        let rx = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Import color schemes".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = rx.await else { return };
            let _ = this.update(cx, |view, cx| {
                let mut lines = Vec::new();
                let mut first = None;
                let mut ok = true;
                for path in paths {
                    match crate::commands::import_theme_file(&path) {
                        Ok(saved) => {
                            for (id, name, format) in saved {
                                lines.push(format!("Imported {name:?} ({format})."));
                                first.get_or_insert(id);
                            }
                        }
                        Err(e) => {
                            ok = false;
                            lines.push(format!("{}: {e}", path.display()));
                        }
                    }
                }
                AppSettings::reload_themes(cx);
                if let Some(id) = first {
                    view.choose_theme(id, cx);
                }
                view.outcome = Some(Outcome { ok, lines, diff: Vec::new(), reload: false });
                cx.notify();
            });
        })
        .detach();
    }

    pub fn open_themes_folder(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(dir) = goatty_config::themes::themes_dir() {
            let _ = std::fs::create_dir_all(&dir);
            cx.open_with_system(&dir);
        }
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
                        let changed = a.applied && !a.diff.is_empty();
                        if a.applied {
                            lines.push(match a.generation {
                                Some(g) if changed => {
                                    format!("Applied as generation {g}. New sessions use it; reload running shells to use it there too.")
                                }
                                Some(g) => format!("Applied. The shell configuration didn't change (generation {g})."),
                                None => "Applied.".into(),
                            });
                        } else if a.diff.is_empty() {
                            lines.push("No change to the generated shell configuration.".into());
                        } else {
                            lines.push(format!("{} generated file(s) would change.", a.diff.len()));
                        }
                        lines.extend(a.issues.iter().map(|i| format!("⚠ {i}")));
                        view.show_diff = !apply;
                        view.outcome = Some(Outcome { ok: true, lines, diff: a.diff, reload: changed });
                        if a.applied {
                            view.draft = None;
                            view.load_host(cx);
                        }
                    }
                    Err(e) => view.outcome = Some(Outcome { ok: false, lines: vec![format!("{e:#}")], diff: Vec::new(), reload: false }),
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

    /// Ask the host's running managed shells to restart with the active
    /// generation.
    pub fn reload_shells(&mut self, cx: &mut Context<'_, Self>) {
        let Some(link) = self.link.clone() else { return };
        self.busy = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { link.reload_shells() }).await;
            let _ = this.update(cx, |view, cx| {
                view.busy = false;
                let (ok, lines) = match result {
                    Ok(0) => (true, vec!["No running goatty shells to reload.".to_string()]),
                    Ok(n) => (
                        true,
                        vec![
                            format!("Asked {n} running shell(s) to reload."),
                            "Idle prompts restart now, busy shells after their current command. Shells with background jobs only show a reminder."
                                .into(),
                        ],
                    ),
                    Err(e) => (false, vec![format!("Couldn't reload shells: {e:#}")]),
                };
                view.outcome = Some(Outcome { ok, lines, diff: Vec::new(), reload: false });
                cx.notify();
            });
        })
        .detach();
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
                        lines: vec![format!(
                            "Generation {generation} is active again, with its settings. New sessions use it; reload running shells to use it there too."
                        )],
                        diff: Vec::new(),
                        reload: true,
                    },
                    Err(e) => Outcome { ok: false, lines: vec![format!("{e:#}")], diff: Vec::new(), reload: false },
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
                    Ok(()) => Outcome { ok: true, lines: vec![format!("Exported to {}", path.display())], diff: Vec::new(), reload: false },
                    Err(e) => Outcome {
                        ok: false,
                        lines: vec![format!("Couldn't write {}: {e}", path.display())],
                        diff: Vec::new(),
                        reload: false,
                    },
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
                        view.outcome = Some(Outcome { ok: true, lines, diff: Vec::new(), reload: false });
                    }
                    Err(e) => {
                        view.outcome =
                            Some(Outcome { ok: false, lines: vec![format!("Not imported: {e}")], diff: Vec::new(), reload: false })
                    }
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
