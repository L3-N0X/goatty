//! The command palette (Ctrl+Shift+P): search goatty's commands by name and
//! run one, like VS Code's or Obsidian's.
//!
//! [`CommandPalette`] is an overlay the [`Workspace`](crate::workspace::Workspace)
//! shows above its panes. It snapshots the [`CommandRegistry`] when it opens,
//! ranks the commands with [`fuzzy`] on every keystroke (recently used ones
//! first while the query is empty, and slightly ahead when scores tie), and
//! emits [`PaletteEvent::Run`] with the chosen command. The workspace then
//! closes it, focuses the terminal again, and runs the command there.

pub mod fuzzy;
pub mod registry;

use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use goatty_config::CommandHistory;
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, KeyBinding, ScrollStrategy, SharedString,
    StyledText, Subscription, UniformListScrollHandle, Window, actions, div, prelude::*, px, uniform_list,
};

pub use registry::{Command, CommandRegistry, register_builtins};

use crate::settings::AppSettings;
use crate::ui::controls::pretty_keys;
use crate::ui::style::Ui;
use crate::ui::text_input::{TextInput, TextInputEvent};

pub const CONTEXT: &str = "CommandPalette";

const ROW_HEIGHT: f32 = 32.;
const MAX_VISIBLE_ROWS: usize = 12;
const WIDTH: f32 = 620.;
/// Shortcuts shown per command (some have one per keyboard layout).
const MAX_SHORTCUTS: usize = 2;
/// A query word that only matches a keyword ("preferences" → Open settings)
/// counts this much less than one matching the label.
const KEYWORD_PENALTY: i32 = 12;
/// Score bonus for the most recently used command, decreasing by one per
/// step back in the history: enough to break near-ties, not to beat a
/// clearly better match.
const RECENT_BONUS: i32 = 8;
/// A query word that matches the title (not just the category) is more
/// specific: "sett" prefers "Settings: Open settings" over "Settings: Shell".
const TITLE_BONUS: i32 = 10;

actions!(command_palette, [SelectNext, SelectPrevious, SelectFirst, SelectLast, PageDown, PageUp, Dismiss]);

/// Key bindings inside the palette. `toggle_keys` are the keystrokes bound
/// to opening it, which close it again.
pub fn bindings(toggle_keys: &[String], cx: &App) -> Vec<KeyBinding> {
    let c = Some(CONTEXT);
    let mut v = vec![
        KeyBinding::new("down", SelectNext, c),
        KeyBinding::new("up", SelectPrevious, c),
        KeyBinding::new("ctrl-n", SelectNext, c),
        KeyBinding::new("ctrl-p", SelectPrevious, c),
        KeyBinding::new("tab", SelectNext, c),
        KeyBinding::new("shift-tab", SelectPrevious, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("ctrl-home", SelectFirst, c),
        KeyBinding::new("ctrl-end", SelectLast, c),
        KeyBinding::new("escape", Dismiss, c),
    ];
    for k in toggle_keys {
        match KeyBinding::load(k, Box::new(Dismiss), gpui::KeyBindingContextPredicate::parse(CONTEXT).ok().map(Rc::new), false, None, cx) {
            Ok(b) => v.push(b),
            Err(e) => log::debug!("palette: can't bind {k} to close: {e}"),
        }
    }
    v
}

/// What the palette tells the workspace.
pub enum PaletteEvent {
    /// Close without running anything.
    Dismissed,
    /// Close and run this command.
    Run(Rc<Command>),
}

/// One ranked result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Index into the palette's command list.
    pub index: usize,
    /// Matched characters of the label, for highlighting.
    pub positions: Vec<usize>,
    pub recent: bool,
}

/// What ranking needs to know about a command.
pub struct Entry<'a> {
    /// "Category: Title".
    pub label: &'a str,
    /// Byte offset of the title in `label` (0 without a category).
    pub title_start: usize,
    pub keywords: &'a [SharedString],
    pub recent_rank: Option<usize>,
}

/// Rank `entries` for `query`. An empty query lists recently used commands
/// (most recent first), then the rest alphabetically.
pub fn rank(entries: &[Entry<'_>], query: &str) -> Vec<Hit> {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        let mut idx: Vec<usize> = (0..entries.len()).collect();
        idx.sort_by(|&a, &b| {
            let (ea, eb) = (&entries[a], &entries[b]);
            (ea.recent_rank.is_none(), ea.recent_rank, ea.label.to_lowercase()).cmp(&(
                eb.recent_rank.is_none(),
                eb.recent_rank,
                eb.label.to_lowercase(),
            ))
        });
        return idx.into_iter().map(|index| Hit { index, positions: Vec::new(), recent: entries[index].recent_rank.is_some() }).collect();
    }
    let mut scored: Vec<(i32, Hit)> = Vec::new();
    'entries: for (index, e) in entries.iter().enumerate() {
        let mut score = 0;
        let mut positions = Vec::new();
        let title = &e.label[e.title_start..];
        let title_chars = e.label[..e.title_start].chars().count();
        for w in &words {
            let label = fuzzy::fuzzy_match(w, e.label);
            let in_title = if e.title_start > 0 { fuzzy::fuzzy_match(w, title) } else { None };
            // Prefer the title's alignment (and highlight it), with a bonus.
            let label = match (label, in_title) {
                (Some(l), Some(t)) => Some(fuzzy::Match {
                    score: l.score.max(t.score) + TITLE_BONUS,
                    positions: t.positions.iter().map(|p| p + title_chars).collect(),
                }),
                (l, _) => l,
            };
            let keyword = e.keywords.iter().filter_map(|k| fuzzy::fuzzy_match(w, k)).map(|m| m.score - KEYWORD_PENALTY).max();
            match (label, keyword) {
                (Some(l), Some(k)) if k > l.score => score += k,
                (Some(l), _) => {
                    score += l.score;
                    positions.extend(l.positions);
                }
                (None, Some(k)) => score += k,
                (None, None) => continue 'entries,
            }
        }
        if let Some(r) = e.recent_rank {
            score += RECENT_BONUS.saturating_sub(r as i32).max(0);
        }
        positions.sort_unstable();
        positions.dedup();
        scored.push((score, Hit { index, positions, recent: e.recent_rank.is_some() }));
    }
    scored.sort_by(|(sa, a), (sb, b)| sb.cmp(sa).then_with(|| entries[a.index].label.len().cmp(&entries[b.index].label.len())));
    scored.into_iter().map(|(_, h)| h).collect()
}

/// How part of a label is drawn (the rest is plain text).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// The "Category: " prefix, muted so the title stands out.
    Category,
    /// Letters the query matched.
    Match,
}

/// Non-overlapping, sorted styled ranges of a label: the matched `matches`,
/// and the category prefix (`..title_start`) around them.
pub fn label_spans(title_start: usize, matches: &[Range<usize>]) -> Vec<(Range<usize>, Span)> {
    let mut out = Vec::new();
    let mut at = 0;
    for m in matches {
        if at < m.start.min(title_start) {
            out.push((at..m.start.min(title_start), Span::Category));
        }
        out.push((m.clone(), Span::Match));
        at = m.end;
    }
    if at < title_start {
        out.push((at..title_start, Span::Category));
    }
    out
}

pub struct CommandPalette {
    input: Entity<TextInput>,
    commands: Vec<Rc<Command>>,
    labels: Vec<SharedString>,
    /// Pretty shortcuts per command ("Ctrl+,").
    shortcuts: Vec<Vec<SharedString>>,
    hits: Vec<Hit>,
    selected: usize,
    query_empty: bool,
    scroll: UniformListScrollHandle,
    history: CommandHistory,
    history_path: Option<PathBuf>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl Focusable for CommandPalette {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl CommandPalette {
    /// A palette over the registered commands, with the user's history.
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        Self::with_history(goatty_config::command_history::history_file(), window, cx)
    }

    pub fn with_history(history_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let commands: Vec<Rc<Command>> = cx.try_global::<CommandRegistry>().map(|r| r.commands().to_vec()).unwrap_or_default();
        let overrides = &cx.global::<AppSettings>().client.keybindings;
        let labels = commands.iter().map(|c| SharedString::from(c.label())).collect();
        let shortcuts = commands
            .iter()
            .map(|c| {
                c.key_action()
                    .map(|a| {
                        let keys = goatty_config::keys::keys_for(a, overrides);
                        keys.iter().take(MAX_SHORTCUTS).map(|k| SharedString::from(pretty_keys(k))).collect()
                    })
                    .unwrap_or_default()
            })
            .collect();
        // A small file, read once per opening (like the color picker's history).
        let history = history_path.as_deref().map(CommandHistory::load).unwrap_or_default();
        let input = cx.new(|cx| TextInput::new("Type a command…", "", cx));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &TextInputEvent, _window, cx| match ev {
            TextInputEvent::Changed(q) => this.set_query(q, cx),
            TextInputEvent::Submitted(_) => this.confirm(cx),
        });
        input.focus_handle(cx).focus(window, cx);
        let mut this = Self {
            input,
            commands,
            labels,
            shortcuts,
            hits: Vec::new(),
            selected: 0,
            query_empty: true,
            scroll: UniformListScrollHandle::new(),
            history,
            history_path,
            _subscriptions: vec![sub],
        };
        this.set_query("", cx);
        this
    }

    fn set_query(&mut self, query: &str, cx: &mut Context<'_, Self>) {
        let entries: Vec<Entry<'_>> = self
            .commands
            .iter()
            .zip(&self.labels)
            .map(|(c, label)| Entry { label, title_start: c.title_start(), keywords: &c.keywords, recent_rank: self.history.rank(&c.id) })
            .collect();
        self.hits = rank(&entries, query);
        self.query_empty = query.trim().is_empty();
        self.select(0, cx);
    }

    fn select(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        self.selected = ix.min(self.hits.len().saturating_sub(1));
        if !self.hits.is_empty() {
            self.scroll.scroll_to_item(self.selected, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<'_, Self>) {
        // Wraps around, like VS Code.
        let n = self.hits.len().max(1);
        self.select((self.selected + 1) % n, cx);
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<'_, Self>) {
        let n = self.hits.len().max(1);
        self.select((self.selected + n - 1) % n, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.select(0, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.select(self.hits.len().saturating_sub(1), cx);
    }

    fn page_down(&mut self, _: &PageDown, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.select(self.selected + MAX_VISIBLE_ROWS - 1, cx);
    }

    fn page_up(&mut self, _: &PageUp, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.select(self.selected.saturating_sub(MAX_VISIBLE_ROWS - 1), cx);
    }

    fn dismiss(&mut self, _: &Dismiss, _: &mut Window, cx: &mut Context<'_, Self>) {
        cx.emit(PaletteEvent::Dismissed);
    }

    fn confirm(&mut self, cx: &mut Context<'_, Self>) {
        self.run_hit(self.selected, cx);
    }

    fn run_hit(&mut self, ix: usize, cx: &mut Context<'_, Self>) {
        let Some(hit) = self.hits.get(ix) else { return };
        let command = self.commands[hit.index].clone();
        if self.history.push(&command.id)
            && let Some(path) = self.history_path.clone()
        {
            let history = self.history.clone();
            cx.background_spawn(async move {
                if let Err(e) = history.save(&path) {
                    log::warn!("couldn't save {}: {e}", path.display());
                }
            })
            .detach();
        }
        cx.emit(PaletteEvent::Run(command));
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, ui: Ui, cx: &mut Context<'_, Self>) -> Vec<gpui::AnyElement> {
        range
            .filter_map(|ix| {
                let hit = self.hits.get(ix)?;
                let label = self.labels[hit.index].clone();
                let selected = ix == self.selected;
                let matched = HighlightStyle { color: Some(ui.accent), font_weight: Some(FontWeight::BOLD), ..Default::default() };
                let category = HighlightStyle { color: Some(ui.muted), ..Default::default() };
                let matches = fuzzy::byte_ranges(&label, &hit.positions);
                let spans = label_spans(self.commands[hit.index].title_start(), &matches);
                let text = StyledText::new(label.clone()).with_highlights(spans.into_iter().map(|(r, span)| {
                    (
                        r,
                        match span {
                            Span::Category => category,
                            Span::Match => matched,
                        },
                    )
                }));
                let keys = self.shortcuts[hit.index].clone();
                let recent = hit.recent && self.query_empty;
                Some(
                    div()
                        .id(ix)
                        .w_full()
                        .h(px(ROW_HEIGHT))
                        .px_3()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .rounded_md()
                        .cursor_pointer()
                        .when(selected, |d| d.bg(ui.surface_hover))
                        .when(!selected, |d| d.hover(|s| s.bg(ui.surface)))
                        .on_click(cx.listener(move |this, _, _, cx| this.run_hit(ix, cx)))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_ellipsis().child(text))
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .gap_1()
                                .items_center()
                                .text_xs()
                                .text_color(ui.muted)
                                .when(recent, |d| d.child("recently used"))
                                .children(
                                    keys.into_iter()
                                        .map(|k| div().px_1p5().rounded_sm().border_1().border_color(ui.border).bg(ui.surface).child(k)),
                                ),
                        )
                        .into_any_element(),
                )
            })
            .collect()
    }
}

impl Render for CommandPalette {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        let ui_font = cx.global::<AppSettings>().ui_font.clone();
        let rows = self.hits.len().min(MAX_VISIBLE_ROWS);
        let list = if self.hits.is_empty() {
            div().px_4().py_3().text_color(ui.muted).child("No matching commands").into_any_element()
        } else {
            let list =
                uniform_list("commands", self.hits.len(), cx.processor(move |this, range, _window, cx| this.render_rows(range, ui, cx)))
                    .track_scroll(&self.scroll)
                    .w_full()
                    .h(px(ROW_HEIGHT * rows as f32));
            div().px_1().child(list).into_any_element()
        };
        div()
            .key_context(CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::select_last))
            .on_action(cx.listener(Self::page_down))
            .on_action(cx.listener(Self::page_up))
            .on_action(cx.listener(Self::dismiss))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PaletteEvent::Dismissed)))
            .occlude()
            .w(px(WIDTH))
            .max_w_full()
            .flex()
            .flex_col()
            .gap_1()
            .pb_1()
            .font_family(ui_font)
            .text_sm()
            .text_color(ui.text)
            .bg(ui.bg)
            .border_1()
            .border_color(ui.border)
            .rounded_lg()
            .shadow_lg()
            .overflow_hidden()
            .child(div().p_2().child(self.input.clone()))
            .child(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries<'a>(labels: &'a [(&'a str, &'a [SharedString], Option<usize>)]) -> Vec<Entry<'a>> {
        labels
            .iter()
            .map(|(label, keywords, recent_rank)| Entry {
                label,
                title_start: label.find(": ").map_or(0, |i| i + 2),
                keywords,
                recent_rank: *recent_rank,
            })
            .collect()
    }

    fn labels(e: &[Entry<'_>], hits: &[Hit]) -> Vec<String> {
        hits.iter().map(|h| e[h.index].label.to_string()).collect()
    }

    #[test]
    fn empty_query_lists_recent_then_alphabetical() {
        let none: &[SharedString] = &[];
        let data = [("b", none, None), ("c", none, Some(1)), ("a", none, None), ("d", none, Some(0))];
        let e = entries(&data);
        let hits = rank(&e, "");
        assert_eq!(labels(&e, &hits), ["d", "c", "a", "b"]);
        assert!(hits[0].recent && !hits[2].recent);
    }

    #[test]
    fn keywords_find_commands_without_highlighting() {
        let kw = [SharedString::from("preferences")];
        let none: &[SharedString] = &[];
        let data = [("Settings: Open settings", &kw[..], None), ("Terminal: Paste", none, None)];
        let e = entries(&data);
        let hits = rank(&e, "prefs");
        assert_eq!(labels(&e, &hits), ["Settings: Open settings"]);
        assert!(hits[0].positions.is_empty());
        // A label match is highlighted.
        let hits = rank(&e, "paste");
        assert_eq!(hits[0].positions, [10, 11, 12, 13, 14]);
    }

    #[test]
    fn label_matches_beat_keyword_matches() {
        let kw = [SharedString::from("theme")];
        let none: &[SharedString] = &[];
        let data = [("Settings: Appearance", &kw[..], None), ("Themes: Open the themes folder", none, None)];
        let e = entries(&data);
        assert_eq!(labels(&e, &rank(&e, "theme"))[0], "Themes: Open the themes folder");
    }

    #[test]
    fn recency_breaks_ties() {
        let none: &[SharedString] = &[];
        let data = [("View: Bigger text", none, None), ("View: Bigger text!", none, Some(0))];
        let e = entries(&data);
        assert_eq!(labels(&e, &rank(&e, "bigger"))[0], "View: Bigger text!");
    }

    #[test]
    fn category_is_muted_around_matches() {
        use Span::*;
        // "Settings: Open settings", title at 10.
        assert_eq!(label_spans(10, &[]), [(0..10, Category)]);
        assert_eq!(label_spans(10, std::slice::from_ref(&(15..19))), [(0..10, Category), (15..19, Match)]);
        assert_eq!(label_spans(10, &[0..4, 15..17]), [(0..4, Match), (4..10, Category), (15..17, Match)]);
        assert_eq!(label_spans(10, &[2..3, 6..12]), [(0..2, Category), (2..3, Match), (3..6, Category), (6..12, Match)]);
        assert_eq!(label_spans(0, std::slice::from_ref(&(1..2))), [(1..2, Match)], "no category");
    }

    #[test]
    fn builtins_rank_sensibly() {
        let cmds = registry::builtins();
        let label_strings: Vec<String> = cmds.iter().map(Command::label).collect();
        let e: Vec<Entry<'_>> = cmds
            .iter()
            .zip(&label_strings)
            .map(|(c, l)| Entry { label: l, title_start: c.title_start(), keywords: &c.keywords, recent_rank: None })
            .collect();
        let top = |q: &str| e[rank(&e, q).first().unwrap_or_else(|| panic!("nothing for {q:?}")).index].label.to_string();
        assert_eq!(top("open settings"), "Settings: Open settings");
        assert_eq!(top("sett"), "Settings: Open settings");
        assert_eq!(top("settings shell"), "Settings: Shell");
        assert_eq!(top("shortcuts"), "Settings: Keyboard shortcuts");
        assert_eq!(top("keybind"), "Settings: Keyboard shortcuts");
        assert_eq!(top("reload"), "Shell: Reload running shells");
        assert_eq!(top("rel sh"), "Shell: Reload running shells");
        assert_eq!(top("zoom in"), "View: Bigger text");
        assert_eq!(top("paste"), "Terminal: Paste");
        assert_eq!(top("theme"), "Themes: Open the themes folder");
        assert_eq!(top("quit"), "goatty: Quit");
        assert!(rank(&e, "qqqq").is_empty());
    }
}
