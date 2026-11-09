//! "Search settings" in the sidebar: an index of every setting (its row
//! label, section and extra words), plus the pages and the shortcut
//! actions. Opening a result shows its page, scrolls to the row and
//! highlights it ([`super::kit::set_focus_row`]).
//!
//! Keep `label` identical to the row's label on the page, or the jump
//! lands on the page without a highlight.

use std::time::Instant;

use goatty_config::keys::ACTIONS;
use gpui::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, div, px};

use super::kit::{self, card, row, section};
use super::{Page, SettingsWindow};
use crate::ui::icons::Icon;
use crate::ui::style::Ui;

pub struct Entry {
    pub page: Page,
    /// The row's label.
    pub label: &'static str,
    pub section: &'static str,
    /// Other words people search for.
    pub keywords: &'static str,
}

const fn e(page: Page, section: &'static str, label: &'static str, keywords: &'static str) -> Entry {
    Entry { page, label, section, keywords }
}

pub const INDEX: &[Entry] = &[
    e(Page::Appearance, "Theme", "Follow the system", "theme light dark mode color scheme"),
    e(Page::Appearance, "Theme", "Customize colors", "palette color editor ansi background foreground cursor"),
    e(Page::Appearance, "Text", "Font", "family typeface monospace nerd"),
    e(Page::Appearance, "Text", "Size", "font size zoom bigger smaller"),
    e(Page::Appearance, "Text", "Line height", "spacing leading"),
    e(Page::Appearance, "Text", "Ligatures", "renderer shaping"),
    e(Page::Appearance, "Text", "Bold text in bright colors", "bold bright xterm"),
    e(Page::Appearance, "Cursor", "Shape", "cursor block bar beam underline caret"),
    e(Page::Appearance, "Cursor", "Blinking", "cursor blink"),
    e(Page::Appearance, "Window", "Padding", "margin spacing border inset"),
    e(Page::Appearance, "Window", "Background opacity", "transparency transparent alpha see through"),
    e(Page::Appearance, "Window", "Blur behind the window", "blur frosted glass"),
    e(Page::Terminal, "Links", "Open links", "url hyperlink click osc 8"),
    e(Page::Terminal, "Links", "Open files with", "editor file opener path vscode"),
    e(Page::Terminal, "Selection", "Copy selected text", "copy on select primary clipboard"),
    e(Page::Terminal, "Pasting", "Middle click pastes", "middle mouse button paste"),
    e(Page::Terminal, "Pasting", "Ask before pasting", "paste confirm protection bracketed multiline"),
    e(Page::Workspace, "Startup", "When goatty starts", "restore reattach session launch open"),
    e(Page::Workspace, "Tabs", "Style", "tab style modern compact"),
    e(Page::Workspace, "Tabs", "Show the tab bar", "hide tabs"),
    e(Page::Workspace, "Tabs", "Program icons", "nerd font icon tab"),
    e(Page::Workspace, "Panes", "Fade inactive panes", "dim split focus"),
    e(Page::Workspace, "Panes", "Ask before closing a running program", "confirm close quit"),
    e(Page::Hosts, "Connecting", "Open hosts in", "ssh window tab remote"),
    e(Page::Hosts, "From ~/.ssh/config", "List these hosts too", "ssh config discover"),
    e(Page::Shell, "", "goatty's managed zsh", "zsh managed dotfiles"),
    e(Page::Shell, "Program", "Shell", "program bash fish login shell"),
    e(Page::Shell, "Your dotfiles", "Use ~/.zshenv and ~/.zprofile", "dotfiles environment path"),
    e(Page::Shell, "Your dotfiles", "Also use ~/.zshrc", "zshrc dotfiles"),
    e(Page::Shell, "Integration", "Shell integration", "osc 133 osc 7 prompt marks directory"),
    e(Page::Prompt, "Prompt", "Starship", "prompt git"),
    e(Page::Prompt, "Prompt", "Style", "rich minimal nerd font symbols"),
    e(Page::Prompt, "Prompt", "Use my own starship.toml", "starship config"),
    e(Page::Prompt, "Layout", "Input on its own line", "two line newline"),
    e(Page::Prompt, "Layout", "Empty line before each prompt", "newline blank space"),
    e(Page::Prompt, "Layout", "Show how long commands took", "duration time took"),
    e(Page::History, "Saving", "Commands to keep", "size length histsize"),
    e(Page::History, "Saving", "Skip duplicates", "dedupe repeat"),
    e(Page::History, "Saving", "Skip commands starting with a space", "secret ignore space private"),
    e(Page::History, "Saving", "Share between running shells", "sync share tabs"),
    e(Page::History, "Search", "Atuin", "ctrl+r history search"),
    e(Page::History, "Search", "Atuin on the Up arrow", "up arrow"),
    e(Page::Profile, "Files", "Appearance, shortcuts, workspace and hosts", "settings.json config file path"),
    e(Page::Profile, "Share", "Export a profile", "backup save file"),
    e(Page::Profile, "Share", "Import a profile", "restore load file"),
    e(Page::Profile, "Shell configuration history", "Generation", "rollback generations undo"),
    e(Page::Profile, "Reset", "Reset appearance, shortcuts and workspace", "defaults factory reset"),
    e(Page::About, "Details", "goatty", "version about update bug report"),
];

/// A search hit.
pub enum Hit {
    Setting(&'static Entry),
    Page(Page),
    /// A shortcut action: (label, group).
    Action(&'static str, &'static str),
}

/// Everything that matches every word of `query` (lowercase).
pub fn search(query: &str) -> Vec<Hit> {
    let words: Vec<&str> = query.split_whitespace().collect();
    let all = |hay: &str| {
        let hay = hay.to_lowercase();
        words.iter().all(|w| hay.contains(w))
    };
    let mut hits: Vec<Hit> = Page::ALL.into_iter().filter(|p| all(&format!("{} {}", p.title(), p.summary()))).map(Hit::Page).collect();
    hits.extend(INDEX.iter().filter(|e| all(&format!("{} {} {} {}", e.label, e.section, e.page.title(), e.keywords))).map(Hit::Setting));
    hits.extend(ACTIONS.iter().filter(|a| all(&format!("{} {} shortcut key", a.label, a.group))).map(|a| Hit::Action(a.label, a.group)));
    hits
}

impl SettingsWindow {
    fn open_hit(&mut self, hit: &Hit, cx: &mut Context<'_, Self>) {
        self.search.update(cx, |i, cx| i.set_text("", cx));
        match hit {
            Hit::Setting(e) => {
                self.go(e.page, cx);
                if e.section == "Theme" && e.label == "Customize colors" {
                    self.colors_open = true;
                }
                self.highlight = Some((e.label.into(), Instant::now()));
            }
            Hit::Page(p) => self.go(*p, cx),
            Hit::Action(label, _) => {
                self.go(Page::Shortcuts, cx);
                self.shortcuts_search(label, cx);
            }
        }
        cx.notify();
    }

    /// Enter in the search field: open the first result.
    pub(super) fn open_first_result(&mut self, cx: &mut Context<'_, Self>) {
        let query = self.search.read(cx).text().trim().to_lowercase();
        if let Some(hit) = search(&query).into_iter().next() {
            self.open_hit(&hit, cx);
        }
    }

    pub(super) fn search_results(&self, ui: Ui, query: &str, cx: &Context<'_, Self>) -> AnyElement {
        let hits = search(query);
        if hits.is_empty() {
            return card(ui)
                .child(kit::empty(
                    ui,
                    Icon::Search,
                    format!("Nothing found for \u{201c}{query}\u{201d}"),
                    "Try another word: settings are also found by what they do.",
                ))
                .into_any_element();
        }
        let mut page = div().flex().flex_col().gap(px(28.));
        let pages: Vec<&Hit> = hits.iter().filter(|h| matches!(h, Hit::Page(_))).collect();
        if !pages.is_empty() {
            page = page.child(section(ui, "Pages").children(pages.into_iter().enumerate().map(|(i, h)| {
                let Hit::Page(p) = h else { unreachable!() };
                let p = *p;
                row(ui, p.title())
                    .icon(p.icon())
                    .desc(p.summary())
                    .on_click(("search-page", i), cx.listener(move |v, _, _, cx| v.open_hit(&Hit::Page(p), cx)))
            })));
        }
        for p in Page::ALL {
            let in_page: Vec<&'static Entry> = hits
                .iter()
                .filter_map(|h| match h {
                    Hit::Setting(e) if e.page == p => Some(*e),
                    _ => None,
                })
                .collect();
            if in_page.is_empty() {
                continue;
            }
            page = page.child(section(ui, p.title()).children(in_page.into_iter().map(|e| {
                let desc = if e.section.is_empty() { p.title().to_string() } else { format!("{} › {}", p.title(), e.section) };
                row(ui, e.label).icon(p.icon()).desc(desc).on_click(
                    SharedString::from(format!("search-{}-{}", p.title(), e.label)),
                    cx.listener(move |v, _, _, cx| v.open_hit(&Hit::Setting(e), cx)),
                )
            })));
        }
        let actions: Vec<(&'static str, &'static str)> = hits
            .iter()
            .filter_map(|h| match h {
                Hit::Action(l, g) => Some((*l, *g)),
                _ => None,
            })
            .collect();
        if !actions.is_empty() {
            page = page.child(section(ui, "Shortcuts").children(actions.into_iter().take(12).map(|(label, group)| {
                row(ui, label).icon(Icon::Keyboard).desc(format!("Shortcuts › {group}")).on_click(
                    SharedString::from(format!("search-action-{label}")),
                    cx.listener(move |v, _, _, cx| v.open_hit(&Hit::Action(label, group), cx)),
                )
            })));
        }
        page.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_settings_by_label_and_keywords() {
        let labels = |q: &str| {
            search(q)
                .into_iter()
                .filter_map(|h| match h {
                    Hit::Setting(e) => Some(e.label),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(labels("opacity").contains(&"Background opacity"));
        assert!(labels("transparent").contains(&"Background opacity"));
        assert!(labels("cursor blink").contains(&"Blinking"));
        assert!(labels("starship").contains(&"Use my own starship.toml"));
        assert!(labels("zzzz").is_empty());
    }

    #[test]
    fn finds_pages_and_shortcuts() {
        let hits = search("split");
        assert!(hits.iter().any(|h| matches!(h, Hit::Action(..))));
        assert!(search("plugins").iter().any(|h| matches!(h, Hit::Page(Page::Plugins))));
    }
}
