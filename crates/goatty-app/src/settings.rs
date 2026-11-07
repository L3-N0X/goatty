//! The client settings as a GPUI global: loaded from `settings.json`,
//! reloaded when the file changes, edited by the settings window, observed
//! by every view that renders with them.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use goatty_config::{ClientSettings, Document, FileStamp, Loaded, ThemeColors, store};
use goatty_term::Palette;
use gpui::{App, Global};

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
        let mut s = Self {
            client: Arc::new(ClientSettings::default()),
            file,
            env_overrides: ClientSettings::env_overrides(|k| std::env::var(k).ok()),
            path,
            stamp,
            fonts: Arc::new(fonts),
            ui_font,
            key_errors: Vec::new(),
            save_error: None,
        };
        s.recompute();
        log::info!("font: {} {}px, theme {}", s.client.font.family.as_deref().unwrap_or("?"), s.client.font.size, s.client.theme.name);
        s.key_errors = crate::actions::apply_keybindings(&s.client.keybindings, cx);
        cx.set_global(s);

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
        v.font.family = Some(v.font.resolve_family(&self.fonts));
        self.client = Arc::new(v);
    }

    /// Replace the stored settings and apply them everywhere.
    fn replace(cx: &mut App, file: Loaded<ClientSettings>, save_error: Option<String>) {
        let mut s = cx.global::<Self>().clone();
        let old_keys = s.client.keybindings.clone();
        s.file = file;
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
        let mut file = cx.global::<Self>().file.clone();
        f(&mut file.value);
        file.issues = file.value.validate();
        let save_error = match &cx.global::<Self>().path {
            Some(p) => store::save(p, &file.value, &file.unknown).err().map(|e| format!("couldn't save {}: {e}", p.display())),
            None => Some("$HOME is not set: settings apply until goatty quits".into()),
        };
        if let Some(e) = &save_error {
            log::warn!("{e}");
        }
        Self::replace(cx, file, save_error);
    }

    /// Replace everything (profile import, reset). Same as [`update`](Self::update).
    pub fn set(cx: &mut App, value: ClientSettings) {
        Self::update(cx, |s| *s = value);
    }

    /// Pick up edits made outside goatty (an editor on settings.json).
    fn reload_if_changed(cx: &mut App) {
        let s = cx.global::<Self>();
        let Some(path) = s.path.clone() else { return };
        if FileStamp::of(&path) == s.stamp {
            return;
        }
        let file: Loaded<ClientSettings> = store::load(&path);
        log::info!("settings changed on disk; reloading");
        for issue in &file.issues {
            log::warn!("settings: {issue}");
        }
        Self::replace(cx, file, None);
    }

    /// All problems to show: file issues, key binding errors, save errors.
    pub fn problems(&self) -> Vec<String> {
        let mut v: Vec<String> = self.file.issues.iter().map(ToString::to_string).collect();
        v.extend(self.key_errors.iter().cloned());
        v.extend(self.save_error.iter().cloned());
        v
    }
}

/// The terminal palette for a theme.
pub fn palette(c: &ThemeColors, bold_is_bright: bool) -> Palette {
    Palette::from_rgb_list(&c.to_vec(), bold_is_bright).expect("21 colors")
}
