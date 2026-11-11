//! The client settings as a GPUI global: loaded from `settings.json`,
//! reloaded when the file changes, edited by the settings window, observed
//! by every view that renders with them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use goatty_config::{ClientSettings, Document, FileStamp, Issue, Loaded, ThemeColors, ThemeLibrary, store};
use goatty_term::Palette;
use gpui::{App, ClipboardItem, Global, WindowAppearance};

use crate::toast::{self, Toast};

/// Preferred UI fonts, first installed wins.
const UI_FONTS: &[&str] = &["Inter", "Cantarell", "Noto Sans", "Ubuntu", "Open Sans", "DejaVu Sans", "Liberation Sans"];

/// How often the settings file is checked for outside edits.
const POLL: Duration = Duration::from_millis(500);

#[derive(Clone)]
pub struct AppSettings {
    /// What views use: the file plus `GOATTY_*` overrides, with the font
    /// family resolved against the installed fonts.
    pub client: Arc<ClientSettings>,
    /// The file as loaded (what the settings window edits).
    pub file: Loaded<ClientSettings>,
    pub env_overrides: Vec<&'static str>,
    pub path: Option<PathBuf>,
    stamp: FileStamp,
    /// Installed font families, sorted.
    pub fonts: Arc<Vec<String>>,
    /// Proportional font for goatty's own windows (GPUI's default UI font
    /// name doesn't resolve on every Linux system).
    pub ui_font: gpui::SharedString,
    /// Problems with key bindings (bad keystrokes).
    pub key_errors: Vec<String>,
    /// The last save failed (read-only config dir, ...).
    pub save_error: Option<String>,
    /// The toast showing `save_error`, dismissed once a save works again.
    save_toast: Option<toast::ToastId>,
    /// The problems the user was last told about (or that were there at
    /// start), so a change is announced against this and not against a
    /// live preview that already moved `issues()`.
    announced: Vec<Issue>,
    /// Problems an edit cleared that didn't reach the disk (a failed save):
    /// the all-clear for them waits until a save works.
    owed_all_clear: Vec<Issue>,
    /// Built-in plus user themes (`themes/*.json`).
    pub themes: Arc<ThemeLibrary>,
    themes_stamp: Vec<(PathBuf, FileStamp)>,
    /// The desktop prefers dark (portal `color-scheme`); picks
    /// `theme.dark`/`theme.light` when `theme.follow_system` is on.
    pub system_dark: bool,
    /// The active theme's colors with overrides: what everything renders with.
    pub colors: ThemeColors,
    /// Themes the settings name that don't exist.
    pub theme_issues: Vec<Issue>,
    /// Settings that parse but don't work on this machine (a font that
    /// isn't installed, a file opener that isn't on PATH).
    pub env_issues: Vec<Issue>,
}

pub fn is_dark(a: WindowAppearance) -> bool {
    matches!(a, WindowAppearance::Dark | WindowAppearance::VibrantDark)
}

impl Global for AppSettings {}

impl AppSettings {
    /// Load the settings, bind keys, and start watching the file.
    pub fn init(cx: &mut App) {
        let path = goatty_config::client_file();
        let file: Loaded<ClientSettings> = path.as_deref().map(store::load).unwrap_or_default();
        for issue in &file.issues {
            log::warn!("settings: {issue}");
        }
        let mut fonts = cx.text_system().all_font_names();
        fonts.sort_by_key(|f| f.to_lowercase());
        fonts.dedup();
        let stamp = path.as_deref().map(FileStamp::of).unwrap_or_default();
        let ui_font =
            UI_FONTS.iter().find(|f| fonts.iter().any(|i| i == *f)).map(|f| f.to_string()).unwrap_or_else(|| "sans-serif".into()).into();
        let themes = ThemeLibrary::load_default();
        for issue in &themes.issues {
            log::warn!("themes: {issue}");
        }
        let themes_stamp = themes.dir.as_deref().map(ThemeLibrary::stamp).unwrap_or_default();
        let mut s = Self {
            themes: Arc::new(themes),
            themes_stamp,
            system_dark: is_dark(cx.window_appearance()),
            colors: goatty_config::ThemeSettings::default().colors(),
            theme_issues: Vec::new(),
            env_issues: Vec::new(),
            client: Arc::new(ClientSettings::default()),
            file,
            env_overrides: ClientSettings::env_overrides(|k| std::env::var(k).ok()),
            path,
            stamp,
            fonts: Arc::new(fonts),
            ui_font,
            key_errors: Vec::new(),
            save_error: None,
            save_toast: None,
            announced: Vec::new(),
            owed_all_clear: Vec::new(),
        };
        s.recompute();
        log::info!(
            "font: {} {}px, theme {}{}",
            s.client.font.family.as_deref().unwrap_or("?"),
            s.client.font.size,
            s.client.theme.active_id(s.system_dark),
            if s.client.theme.follow_system { " (following the system)" } else { "" }
        );
        s.key_errors = crate::actions::apply_keybindings(&s.client.keybindings, cx);
        let issues = s.announceable();
        s.announced = issues.clone();
        cx.set_global(s);
        // A file broken before goatty started: say so once a window is up.
        Self::announce(cx, &[], issues);

        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                cx.update(Self::reload_if_changed);
            }
        })
        .detach();
    }

    fn recompute(&mut self) {
        let mut v = self.file.value.clone();
        v.apply_env(|k| std::env::var(k).ok());
        self.env_issues = v.check_environment(&self.fonts, has_program);
        v.font.family = Some(v.font.resolve_family(&self.fonts));
        let (colors, _) = self.themes.resolve(&v.theme, self.system_dark);
        self.colors = colors;
        self.theme_issues = self.themes.check(&v.theme);
        self.client = Arc::new(v);
    }

    /// The desktop switched between light and dark.
    pub fn set_system_dark(cx: &mut App, dark: bool) {
        if cx.global::<Self>().system_dark == dark {
            return;
        }
        let mut s = cx.global::<Self>().clone();
        s.system_dark = dark;
        s.recompute();
        log::info!("desktop appearance: {}", if dark { "dark" } else { "light" });
        cx.set_global(s);
    }

    /// Re-read `themes/` (after the editor saved or deleted a theme).
    pub fn reload_themes(cx: &mut App) {
        let mut s = cx.global::<Self>().clone();
        let themes = ThemeLibrary::load_default();
        s.themes_stamp = themes.dir.as_deref().map(ThemeLibrary::stamp).unwrap_or_default();
        s.themes = Arc::new(themes);
        s.recompute();
        cx.set_global(s);
    }

    /// Replace the stored settings and apply them everywhere.
    fn replace(cx: &mut App, file: Loaded<ClientSettings>, save_error: Option<String>) {
        let mut s = cx.global::<Self>().clone();
        let old_keys = s.client.keybindings.clone();
        s.file = file;
        if save_error.is_none()
            && let Some(id) = s.save_toast.take()
        {
            toast::dismiss(cx, id);
        }
        s.save_error = save_error;
        s.stamp = s.path.as_deref().map(FileStamp::of).unwrap_or_default();
        s.recompute();
        if s.client.keybindings != old_keys {
            s.key_errors = crate::actions::apply_keybindings(&s.client.keybindings, cx);
        }
        cx.set_global(s);
    }

    /// Edit the stored settings (the settings window). Validates, saves
    /// (with a backup), and applies live.
    pub fn update(cx: &mut App, f: impl FnOnce(&mut ClientSettings)) {
        Self::update_with(cx, f, true);
    }

    /// Like [`update`](Self::update) but only applies the change, without
    /// writing the file: a live preview while a color is being dragged. The
    /// caller saves with `update` once the user lets go.
    pub fn preview(cx: &mut App, f: impl FnOnce(&mut ClientSettings)) {
        Self::update_with(cx, f, false);
    }

    fn update_with(cx: &mut App, f: impl FnOnce(&mut ClientSettings), save: bool) {
        let mut file = cx.global::<Self>().file.clone();
        f(&mut file.value);
        file.issues = file.value.validate();
        let save_error = if !save {
            cx.global::<Self>().save_error.clone()
        } else {
            let e = match &cx.global::<Self>().path {
                Some(p) => store::save(p, &file.value, &file.unknown).err().map(|e| format!("couldn't save {}: {e}", p.display())),
                None => Some("$HOME is not set: settings apply until goatty quits".into()),
            };
            if let Some(e) = &e {
                log::warn!("{e}");
            }
            e
        };
        // Every failed save says so, even with the same text as the last one:
        // the user may have dismissed that toast. A preview doesn't save, so
        // it stays quiet.
        let save_toast = save_error.as_ref().filter(|_| save).map(|e| {
            let copy = e.clone();
            toast::show(
                cx,
                Toast::error("Couldn't save the settings").body(e.clone()).key(SAVE_TOAST_KEY).action("Copy", move |_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                    toast::show(cx, Toast::success("Copied the error"));
                }),
            )
        });
        Self::replace(cx, file, save_error);
        if save_toast.is_some() {
            cx.global_mut::<Self>().save_toast = save_toast;
        }
        if save {
            Self::announce_changes(cx);
        }
    }

    /// Replace everything (profile import, reset). Same as [`update`](Self::update).
    pub fn set(cx: &mut App, value: ClientSettings) {
        Self::update(cx, |s| *s = value);
    }

    /// Pick up edits made outside goatty (an editor on settings.json, a
    /// theme dropped into themes/).
    fn reload_if_changed(cx: &mut App) {
        let s = cx.global::<Self>();
        if let Some(dir) = s.themes.dir.as_deref()
            && ThemeLibrary::stamp(dir) != s.themes_stamp
        {
            log::info!("themes changed on disk; reloading");
            Self::reload_themes(cx);
            for issue in &cx.global::<Self>().themes.issues {
                log::warn!("themes: {issue}");
            }
        }
        let s = cx.global::<Self>();
        if let Some(path) = s.path.clone()
            && FileStamp::of(&path) != s.stamp
        {
            let file: Loaded<ClientSettings> = store::load(&path);
            log::info!("settings changed on disk; reloading");
            for issue in &file.issues {
                log::warn!("settings: {issue}");
            }
            Self::replace(cx, file, None);
        }
        Self::announce_changes(cx);
    }

    /// Announce what changed in the problems since the last announcement.
    fn announce_changes(cx: &mut App) {
        let s = cx.global::<Self>();
        let after = s.announceable();
        let save_failing = s.save_error.is_some();
        let owes = !s.owed_all_clear.is_empty();
        if after == s.announced && !(owes && !save_failing) {
            return;
        }
        let g = cx.global_mut::<Self>();
        let before = std::mem::replace(&mut g.announced, after.clone());
        if after.is_empty() && save_failing {
            // The edit that cleared the last problem didn't reach the disk:
            // hold the all-clear back until a save works.
            if !owes {
                g.owed_all_clear = before;
            }
            return;
        }
        let owed = std::mem::take(&mut g.owed_all_clear);
        let before = if after.is_empty() && !owed.is_empty() { owed } else { before };
        Self::announce(cx, &before, after);
    }

    /// [`issues`](Self::issues) without a failed save, which has its own toast.
    fn announceable(&self) -> Vec<Issue> {
        let save = self.save_error.as_ref().map(|e| Issue::new("", e.clone()));
        self.issues().into_iter().filter(|i| Some(i) != save.as_ref()).collect()
    }

    /// Tell the user about problems that weren't there `before` (#85): a
    /// warning toast that replaces itself on every reload, with buttons to
    /// see the setting and to copy the problems. When an edit fixed the
    /// last problem, the warning turns into an all-clear.
    fn announce(cx: &mut App, before: &[Issue], after: Vec<Issue>) {
        let new: Vec<&Issue> = after.iter().filter(|i| !before.contains(i)).collect();
        let Some(first) = new.first() else {
            if after.is_empty() && !before.is_empty() {
                toast::show(cx, Toast::success("The settings on this computer are fine again").key(TOAST_KEY));
            }
            return;
        };
        let title = match after.len() {
            1 => "A setting couldn't be used".to_string(),
            n => format!("{n} settings couldn't be used"),
        };
        let mut body = first.to_string();
        if new.len() > 1 {
            body.push_str(&format!(" (and {} more)", new.len() - 1));
        }
        let path = first.path.clone();
        let t = Toast::warning(title)
            .body(body)
            .key(TOAST_KEY)
            .action("Open settings", move |_, cx| crate::settings_window::open_issue(cx, false, &path))
            .action("Copy", |_, cx| {
                let report = cx.global::<Self>().report();
                cx.write_to_clipboard(ClipboardItem::new_string(report));
                toast::show(cx, Toast::success("Copied the problems").key(TOAST_KEY));
            });
        toast::show(cx, t);
    }

    /// Every problem with this computer's settings, deduplicated: the
    /// file, its themes, key bindings, the last save.
    pub fn issues(&self) -> Vec<Issue> {
        let mut v: Vec<Issue> = Vec::new();
        let all = self.file.issues.iter().chain(&self.env_issues).chain(&self.theme_issues).chain(&self.themes.issues).cloned();
        let keys = self.key_errors.iter().map(|e| Issue::parse(e));
        let save = self.save_error.iter().map(|e| Issue::new("", e.clone()));
        for i in all.chain(keys).chain(save) {
            if !v.contains(&i) {
                v.push(i);
            }
        }
        v
    }

    /// The problems as text for a bug report or a chat.
    pub fn report(&self) -> String {
        let file = self.path.as_ref().map_or_else(|| "settings.json".to_string(), |p| p.display().to_string());
        let issues = self.issues();
        let mut text = store::report([(file.as_str(), issues.as_slice())]);
        if !self.env_overrides.is_empty() {
            text.push_str(&format!("overridden by: {}\n", self.env_overrides.join(", ")));
        }
        text
    }
}

/// Problem toasts replace each other.
pub(crate) const TOAST_KEY: &str = "settings.issues";

/// Problems with the host's `host.json` get their own key, so a recovery
/// message about this computer's settings can't replace them.
pub(crate) const HOST_TOAST_KEY: &str = "settings.host.issues";

/// Save errors get their own key, so a theme reload's warning can't replace them.
const SAVE_TOAST_KEY: &str = "settings.save";

/// Whether `program` (a path or a name on PATH) exists.
pub(crate) fn has_program(program: &str) -> bool {
    if program.contains('/') {
        let p = std::path::Path::new(program);
        return p.is_file();
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(program).is_file()))
}

/// The terminal palette for a theme.
pub fn palette(c: &ThemeColors, bold_is_bright: bool) -> Palette {
    Palette::from_rgb_list(&c.to_vec(), bold_is_bright).expect("21 colors")
}
