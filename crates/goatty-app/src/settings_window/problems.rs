//! Problems with the settings, where people look for them (#53, #85):
//! on the row of the setting (a warning under its description, or "Set by
//! GOATTY_…" with the control disabled), and all of them on the Profile
//! page, with a button that copies them as text for a bug report.

use std::collections::HashMap;

use goatty_config::{ClientSettings, HostSettings, Issue, store};
use gpui::{AnyElement, App, ClipboardItem, Context, IntoElement, SharedString};

use super::kit::{RowNote, row, section};
use super::search::{self, Hit, INDEX};
use super::{Page, SettingsWindow};
use crate::settings::AppSettings;
use crate::toast::{self, Toast};
use crate::ui::controls::{ButtonKind, button_with};
use crate::ui::icons::Icon;
use crate::ui::style::Ui;

/// `GOATTY_*` variables in effect, with the settings each one replaces.
type Overrides = Vec<(String, &'static [&'static str])>;

impl SettingsWindow {
    /// The host's problems with its settings. One about a value changed in
    /// the draft is left out: the edit is the fix, applied or not.
    pub(super) fn host_issues(&self) -> Vec<Issue> {
        let Some(h) = &self.host else {
            // No snapshot (yet, or it failed): the shell status carries the
            // host file's problems, so Diagnostics and Profile agree.
            return self.diag.status.iter().flat_map(|s| &s.settings_issues).map(|i| Issue::parse(i)).collect();
        };
        h.issues
            .iter()
            .map(|i| Issue::parse(i))
            .filter(|i| match &self.draft {
                Some(draft) if !i.path.is_empty() => store::get_key(draft, &i.path) == store::get_key(&h.loaded.value, &i.path),
                _ => true,
            })
            .collect()
    }

    /// Tell the user about host problems that weren't in the snapshot
    /// before (#85), like [`AppSettings`] does for this computer's: the
    /// same keyed warning toast, with buttons to see the setting and to
    /// copy the problems.
    pub(super) fn announce_host_issues(&self, before: &[String], cx: &mut App) {
        let Some(host) = &self.host else { return };
        let issues: Vec<Issue> = host.issues.iter().map(|i| Issue::parse(i)).collect();
        let new: Vec<&Issue> = host.issues.iter().zip(&issues).filter(|(raw, _)| !before.contains(raw)).map(|(_, i)| i).collect();
        let Some(first) = new.first() else { return };
        let title = match issues.len() {
            1 => "A host setting couldn't be used".to_string(),
            n => format!("{n} host settings couldn't be used"),
        };
        let mut body = first.to_string();
        if new.len() > 1 {
            body.push_str(&format!(" (and {} more)", new.len() - 1));
        }
        let path = first.path.clone();
        let report = store::report([(format!("host.json on {}", self.host_label()).as_str(), issues.as_slice())]);
        toast::show(
            cx,
            Toast::warning(title)
                .body(body)
                .key(crate::settings::HOST_TOAST_KEY)
                .action("Show", move |_, cx| super::open_issue(cx, true, &path))
                .action("Copy", move |_, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(report.clone()));
                    toast::show(cx, Toast::success("Copied the problems").key(crate::settings::HOST_TOAST_KEY));
                }),
        );
    }

    fn host_overrides(&self) -> Overrides {
        self.host.as_ref().map_or_else(Vec::new, |h| h.env_overrides.iter().map(|v| (v.clone(), HostSettings::env_keys(v))).collect())
    }

    fn page_problems(&self, page: Page, cx: &App) -> (Vec<Issue>, Overrides) {
        if page.is_host() {
            (self.host_issues(), self.host_overrides())
        } else {
            let app = cx.global::<AppSettings>();
            (app.issues(), app.env_overrides.iter().map(|v| (v.to_string(), ClientSettings::env_keys(v))).collect())
        }
    }

    /// What each row of the current page should say, by label.
    pub(super) fn row_notes(&self, cx: &App) -> HashMap<SharedString, RowNote> {
        let (issues, overrides) = self.page_problems(self.page, cx);
        notes(self.page, &issues, &overrides)
    }

    /// The host's problems and overrides that no row of `page` shows: for
    /// the banner over a host page.
    pub(super) fn unshown_host_problems(&self, page: Page) -> (Vec<String>, Vec<String>) {
        let keys: Vec<&str> = INDEX.iter().filter(|e| e.page == page).flat_map(|e| e.keys.iter().copied()).collect();
        let issues = self.host_issues().into_iter().filter(|i| !keys.iter().any(|k| i.concerns(k))).map(|i| i.to_string()).collect();
        let vars =
            self.host_overrides().into_iter().filter(|(_, replaced)| !replaced.iter().any(|k| keys.contains(k))).map(|(v, _)| v).collect();
        (issues, vars)
    }

    /// Every problem, this computer's and the host's, as text.
    pub(super) fn problems_report(&self, cx: &App) -> String {
        let mut text = cx.global::<AppSettings>().report();
        let host = self.host_issues();
        if !host.is_empty() {
            let title = format!("host.json on {}", self.host_label());
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&store::report([(title.as_str(), host.as_slice())]));
        }
        let vars: Vec<String> = self.host_overrides().into_iter().map(|(v, _)| v).collect();
        if !vars.is_empty() {
            text.push_str(&format!("overridden on {} by: {}\n", self.host_label(), vars.join(", ")));
        }
        text
    }

    /// Profile: each problem with the setting it's about and a way there.
    /// `None` when there are none.
    pub(super) fn problems_section(&self, ui: Ui, cx: &Context<'_, Self>) -> Option<AnyElement> {
        let client = cx.global::<AppSettings>().issues();
        let host = self.host_issues();
        if client.is_empty() && host.is_empty() {
            return None;
        }
        let label = self.host_label();
        let local = self.link.as_ref().is_some_and(|l| l.is_local());
        let mut rows = Vec::new();
        for (i, (issue, is_host)) in client.iter().map(|i| (i, false)).chain(host.iter().map(|i| (i, true))).enumerate() {
            let entry = search::entry_for(is_host, &issue.path);
            let title = match entry {
                Some(e) => format!("{} › {}", e.page.title(), e.label),
                None if issue.path.is_empty() && is_host => format!("host.json on {label}"),
                None if issue.path.is_empty() => "settings.json".into(),
                None => issue.path.clone(),
            };
            let place = match (is_host, local) {
                (false, _) => "This computer",
                (true, true) => "Shell settings",
                (true, false) => "Shell settings on the host",
            };
            let shown = if entry.is_some() || issue.path.is_empty() { issue.message.clone() } else { issue.to_string() };
            let mut r = row(ui, title).icon(Icon::TriangleAlert).desc(format!("{place} · {shown}"));
            if let Some(e) = entry {
                r = r.control(button_with(
                    ("problem-show", i),
                    None,
                    "Show",
                    ButtonKind::Ghost,
                    ui,
                    self.act(cx, move |v, cx| v.open_hit(&Hit::Setting(e), cx)),
                ));
            }
            rows.push(r);
        }
        let n = rows.len();
        Some(
            section(ui, if n == 1 { "1 problem".to_string() } else { format!("{n} problems") })
                .desc("goatty couldn't use these settings as written")
                .action(self.copy_button(ui, "problems-copy", "Copy all", self.problems_report(cx), cx))
                .children(rows)
                .into_any_element(),
        )
    }
}

/// The notes for `page`'s rows: problems with the settings a row edits,
/// and the variable overriding them.
fn notes(page: Page, issues: &[Issue], overrides: &Overrides) -> HashMap<SharedString, RowNote> {
    let mut out = HashMap::new();
    for e in INDEX.iter().filter(|e| e.page == page && !e.keys.is_empty()) {
        let problems: Vec<SharedString> = issues
            .iter()
            .filter(|i| e.keys.iter().any(|k| i.concerns(k)))
            // A row about one setting doesn't need its path repeated.
            .map(|i| if e.keys.len() == 1 && i.path == e.keys[0] { i.message.clone() } else { i.to_string() })
            .map(SharedString::from)
            .collect();
        let overridden_by =
            overrides.iter().find(|(_, replaced)| replaced.iter().any(|k| e.keys.contains(k))).map(|(v, _)| v.clone().into());
        if !problems.is_empty() || overridden_by.is_some() {
            out.insert(SharedString::from(e.label), RowNote { problems, overridden_by });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_land_on_the_row_that_edits_the_setting() {
        let issues = vec![
            Issue::new("font.size", "400 is outside 6–48; using 48"),
            Issue::new("terminal.padding.top", "99 is outside 0–48; using 48"),
            Issue::new("keybindings.ctrl-k", "unknown action"),
            Issue::new("tabs.style", "unknown variant"),
        ];
        let overrides: Overrides = vec![("GOATTY_THEME".into(), ClientSettings::env_keys("GOATTY_THEME"))];
        let n = notes(Page::Appearance, &issues, &overrides);
        assert_eq!(n["Size"].problems, [SharedString::from("400 is outside 6–48; using 48")]);
        assert_eq!(n["Padding"].problems, [SharedString::from("terminal.padding.top: 99 is outside 0–48; using 48")]);
        assert_eq!(n["Follow the system"].overridden_by.as_ref().map(|s| s.as_ref()), Some("GOATTY_THEME"));
        assert!(!n.contains_key("Style"), "tabs.style is on the Workspace page");
        assert_eq!(n.len(), 3);
        assert!(notes(Page::Workspace, &issues, &overrides).contains_key("Style"));

        let host: Overrides = vec![("GOATTY_SHELL".into(), HostSettings::env_keys("GOATTY_SHELL"))];
        let n = notes(Page::Shell, &[Issue::new("shell.program", "\"fish\" isn't installed")], &host);
        assert_eq!(n["Shell"].overridden_by.as_ref().map(|s| s.as_ref()), Some("GOATTY_SHELL"));
        assert_eq!(n["Shell"].problems.len(), 1);
    }
}
