//! The pieces every settings page is built from, so all pages share one
//! layout (rules: `.claude/rules/settings-ui.md`):
//!
//! - [`section`]: a heading (title, optional description and action) over
//!   a card of rows with hairline dividers between them.
//! - [`row`]: label and description on the left, one control on the right;
//!   or a navigation row (chevron) with [`Row::on_click`].
//! - [`SettingsWindow::select`]: a dropdown for choices whose labels are
//!   too long, or too many, for a segmented control.
//! - [`search_field`], [`chip`] (filters), [`hint`], [`empty`], [`modal`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, Context, Div, ElementId, Entity, Focusable, FontWeight, IntoElement, MouseButton, ParentElement,
    RenderOnce, ScrollHandle, SharedString, Stateful, Styled, Window, anchored, canvas, deferred, div, point, prelude::*, px, relative,
};

use super::SettingsWindow;
use crate::settings::AppSettings;
use crate::ui::icons::Icon;
use crate::ui::style::{Ui, radius, text};
use crate::ui::text_input::TextInput;

type Callback = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

/// The terminal's font, for commands, paths and code.
pub fn mono(cx: &App) -> SharedString {
    cx.global::<AppSettings>().client.font.family.clone().unwrap_or_else(|| "monospace".into()).into()
}

// ---- Sections ---------------------------------------------------------------------

/// A titled group of rows.
#[derive(IntoElement)]
pub struct Section {
    ui: Ui,
    title: Option<SharedString>,
    desc: Option<SharedString>,
    action: Option<AnyElement>,
    rows: Vec<AnyElement>,
    footer: Option<SharedString>,
    /// Children go straight under the heading, without a card.
    plain: bool,
}

pub fn section(ui: Ui, title: impl Into<SharedString>) -> Section {
    Section { ui, title: Some(title.into()), desc: None, action: None, rows: Vec::new(), footer: None, plain: false }
}

/// A card of rows without a heading.
pub fn group(ui: Ui) -> Section {
    Section { ui, title: None, desc: None, action: None, rows: Vec::new(), footer: None, plain: false }
}

impl Section {
    pub fn desc(mut self, d: impl Into<SharedString>) -> Self {
        self.desc = Some(d.into());
        self
    }

    /// A control on the heading's right (a small button, a filter).
    pub fn action(mut self, a: impl IntoElement) -> Self {
        self.action = Some(a.into_any_element());
        self
    }

    pub fn child(mut self, c: impl IntoElement) -> Self {
        self.rows.push(c.into_any_element());
        self
    }

    pub fn children(mut self, c: impl IntoIterator<Item = impl IntoElement>) -> Self {
        self.rows.extend(c.into_iter().map(IntoElement::into_any_element));
        self
    }

    pub fn when_some_child<T>(self, v: Option<T>, f: impl FnOnce(T) -> AnyElement) -> Self {
        match v {
            Some(v) => self.child(f(v)),
            None => self,
        }
    }

    /// A short note under the card.
    pub fn footer(mut self, f: impl Into<SharedString>) -> Self {
        self.footer = Some(f.into());
        self
    }

    pub fn plain(mut self) -> Self {
        self.plain = true;
        self
    }
}

impl RenderOnce for Section {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ui = self.ui;
        let heading = (self.title.is_some() || self.action.is_some()).then(|| {
            div()
                .flex()
                .items_end()
                .justify_between()
                .gap_4()
                .px(px(4.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .min_w_0()
                        .when_some(self.title, |d, t| {
                            d.child(div().text_size(px(text::HEADING)).font_weight(FontWeight::SEMIBOLD).text_color(ui.text).child(t))
                        })
                        .when_some(self.desc, |d, t| d.child(div().text_size(px(text::SMALL)).text_color(ui.muted).child(t))),
                )
                .children(self.action)
        });
        let body = if self.plain {
            div().flex().flex_col().gap(px(10.)).children(self.rows)
        } else {
            let n = self.rows.len();
            let mut card = card(ui);
            for (i, r) in self.rows.into_iter().enumerate() {
                card = card.child(r);
                if i + 1 < n {
                    card = card.child(divider(ui));
                }
            }
            card
        };
        div().flex().flex_col().gap(px(10.)).children(heading).child(body).when_some(self.footer, |d, f| d.child(hint(ui, f).px(px(4.))))
    }
}

/// A rounded container on the page.
pub fn card(ui: Ui) -> Div {
    div().flex().flex_col().rounded(px(radius::CARD)).bg(ui.card).border_1().border_color(ui.divider)
}

pub fn divider(ui: Ui) -> Div {
    div().h(px(1.)).mx(px(16.)).bg(ui.divider)
}

// ---- Rows -------------------------------------------------------------------------

thread_local! {
    /// The row a search result points at: highlighted, and scrolled to once.
    static FOCUS_ROW: RefCell<Option<FocusRow>> = const { RefCell::new(None) };
}

struct FocusRow {
    label: SharedString,
    scroll: ScrollHandle,
    scrolled: bool,
}

/// Before rendering a page: the row labelled `label` is the one a search
/// result opened (`None`: no highlight).
pub fn set_focus_row(label: Option<SharedString>, scroll: &ScrollHandle) {
    FOCUS_ROW.with_borrow_mut(|f| match label {
        Some(label) => {
            if f.as_ref().is_none_or(|f| f.label != label) {
                *f = Some(FocusRow { label, scroll: scroll.clone(), scrolled: false });
            }
        }
        None => *f = None,
    });
}

fn is_focus_row(label: &str) -> bool {
    FOCUS_ROW.with_borrow(|f| f.as_ref().is_some_and(|f| f.label == label))
}

/// Scroll the page so the focused row (at `top`, window coordinates) shows
/// a little below the top, once.
fn scroll_to_focus_row(top: gpui::Pixels, window: &mut Window) {
    let scrolled = FOCUS_ROW.with_borrow_mut(|f| {
        let Some(f) = f.as_mut().filter(|f| !f.scrolled) else { return false };
        f.scrolled = true;
        let view = f.scroll.bounds();
        let offset = f.scroll.offset();
        let y = offset.y - (top - view.top() - px(96.));
        // GPUI doesn't clamp a set offset to the end: do it here.
        let max = f.scroll.max_offset().y;
        f.scroll.set_offset(point(offset.x, y.clamp(-max, px(0.))));
        true
    });
    if scrolled {
        window.refresh();
    }
}

/// What a page knows about one of its rows: problems reading the setting
/// and the `GOATTY_*` variable overriding it (#53).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RowNote {
    pub problems: Vec<SharedString>,
    pub overridden_by: Option<SharedString>,
}

thread_local! {
    /// The notes for the page being rendered, by row label, and the window
    /// they're for (other windows render rows too).
    static ROW_NOTES: RefCell<Option<(gpui::WindowId, HashMap<SharedString, RowNote>)>> = const { RefCell::new(None) };
}

/// Before rendering a page in `window`: what its rows should say.
pub fn set_row_notes(window: gpui::WindowId, notes: HashMap<SharedString, RowNote>) {
    ROW_NOTES.with_borrow_mut(|n| *n = Some((window, notes)));
}

fn row_note(window: &Window, label: &str) -> Option<RowNote> {
    ROW_NOTES.with_borrow(|n| {
        let (id, notes) = n.as_ref()?;
        (*id == window.window_handle().window_id()).then(|| notes.get(label).cloned()).flatten()
    })
}

/// One setting.
#[derive(IntoElement)]
pub struct Row {
    ui: Ui,
    label: SharedString,
    desc: Option<SharedString>,
    control: Option<AnyElement>,
    icon: Option<Icon>,
    below: Option<AnyElement>,
    disabled: bool,
    click: Option<(ElementId, Callback)>,
}

pub fn row(ui: Ui, label: impl Into<SharedString>) -> Row {
    Row { ui, label: label.into(), desc: None, control: None, icon: None, below: None, disabled: false, click: None }
}

impl Row {
    pub fn desc(mut self, d: impl Into<SharedString>) -> Self {
        self.desc = Some(d.into());
        self
    }

    pub fn desc_opt(mut self, d: Option<impl Into<SharedString>>) -> Self {
        self.desc = d.map(Into::into);
        self
    }

    pub fn control(mut self, c: impl IntoElement) -> Self {
        self.control = Some(c.into_any_element());
        self
    }

    /// A leading icon in a tinted square (navigation rows, list items).
    pub fn icon(mut self, i: Icon) -> Self {
        self.icon = Some(i);
        self
    }

    /// Full-width content under the label (a text field, a list).
    pub fn below(mut self, b: impl IntoElement) -> Self {
        self.below = Some(b.into_any_element());
        self
    }

    /// Greyed out, the control doesn't react: say why in the description.
    pub fn disabled(mut self, d: bool) -> Self {
        self.disabled = d;
        self
    }

    /// A navigation row: the whole row is clickable and ends in a chevron.
    pub fn on_click(mut self, id: impl Into<ElementId>, f: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.click = Some((id.into(), Rc::new(f)));
        self
    }
}

impl RenderOnce for Row {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let ui = self.ui;
        let focused = is_focus_row(&self.label);
        let note = row_note(window, &self.label).unwrap_or_default();
        // The environment wins over the file: editing here would change nothing.
        let disabled = self.disabled || note.overridden_by.is_some();
        let label = div().text_size(px(text::BODY)).text_color(ui.text).child(self.label.clone());
        let text_col = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .flex_1()
            .min_w_0()
            .child(label)
            .when_some(self.desc, |d, t| d.child(div().text_size(px(text::SMALL)).text_color(ui.muted).child(t)));
        let notes =
            (!note.problems.is_empty() || note.overridden_by.is_some()).then(|| {
                let line = |icon: Icon, color: gpui::Hsla, t: SharedString| {
                    div()
                        .flex()
                        .items_start()
                        .gap(px(6.))
                        .text_size(px(text::SMALL))
                        .text_color(color)
                        .child(icon.svg().flex_shrink_0().size(px(13.)).mt(px(2.)).text_color(color))
                        .child(div().min_w_0().child(t))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .mt(px(2.))
                    .children(note.overridden_by.map(|var| {
                        line(Icon::Info, ui.accent_text, format!("Set by {var} for this run; unset it to use this setting").into())
                    }))
                    .children(note.problems.into_iter().map(|p| line(Icon::TriangleAlert, ui.warning, p)))
            });
        let control = self.control.map(|c| {
            if disabled {
                // Visible but inert: a cover takes the clicks.
                div().relative().flex_shrink_0().opacity(0.45).child(c).child(div().absolute().inset_0().occlude()).into_any_element()
            } else {
                div().flex_shrink_0().child(c).into_any_element()
            }
        });
        let line = div()
            .flex()
            .items_center()
            .gap(px(16.))
            .min_h(px(58.))
            .px(px(16.))
            .py(px(10.))
            .when_some(self.icon, |d, i| {
                d.child(
                    div()
                        .flex_shrink_0()
                        .size(px(34.))
                        .rounded(px(radius::CONTROL))
                        .bg(ui.field)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(i.svg().size(px(17.)).text_color(ui.muted)),
                )
            })
            .child(text_col.when(self.disabled, |d| d.opacity(0.55)).children(notes))
            .children(control)
            .when(self.click.is_some(), |d| d.child(Icon::ChevronRight.svg().size(px(16.)).text_color(ui.faint)));
        let body = div()
            .relative()
            .flex()
            .flex_col()
            .when(focused, |d| {
                d.child(div().absolute().inset(px(4.)).rounded(px(radius::CONTROL)).bg(ui.accent_soft))
                    .child(canvas(|_, _, _| (), |bounds, _, window, _| scroll_to_focus_row(bounds.top(), window)).absolute().size_full())
            })
            .child(line)
            .when_some(self.below, |d, b| d.child(div().px(px(16.)).pb(px(14.)).child(b)));
        match self.click {
            Some((id, f)) => body
                .id(id)
                .cursor_pointer()
                .rounded(px(radius::CARD))
                .hover(|s| s.bg(ui.surface_hover))
                .on_click(move |ev, window, cx| f(ev, window, cx))
                .into_any_element(),
            None => body.into_any_element(),
        }
    }
}

// ---- Smaller pieces ---------------------------------------------------------------

/// Muted small text: notes under a card, explanations.
pub fn hint(ui: Ui, t: impl Into<SharedString>) -> Div {
    div().text_size(px(text::SMALL)).text_color(ui.muted).child(t.into())
}

/// Nothing to show: an icon, a title and a line.
pub fn empty(ui: Ui, icon: Icon, title: impl Into<SharedString>, desc: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.))
        .py(px(28.))
        .px(px(24.))
        .child(
            div()
                .size(px(44.))
                .mb(px(6.))
                .rounded_full()
                .bg(ui.field)
                .flex()
                .items_center()
                .justify_center()
                .child(icon.svg().size(px(20.)).text_color(ui.muted)),
        )
        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(ui.text).child(title.into()))
        .child(div().text_size(px(text::SMALL)).text_color(ui.muted).text_center().max_w(px(420.)).child(desc.into()))
}

/// A pill search field around a bare [`TextInput`].
pub fn search_field(ui: Ui, id: &str, input: &Entity<TextInput>, window: &Window, cx: &App) -> Stateful<Div> {
    let (focused, empty) = {
        let i = input.read(cx);
        (i.is_focused(window), i.text().is_empty())
    };
    let handle = input.focus_handle(cx);
    let clear = input.clone();
    div()
        .id(SharedString::from(format!("{id}-field")))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(38.))
        .pl(px(14.))
        .pr(px(6.))
        .rounded_full()
        // Light themes: a white pill reads better than a grey one on grey.
        .bg(if ui.dark { ui.field } else { ui.card })
        .border_1()
        .border_color(if focused {
            ui.accent
        } else if ui.dark {
            ui.field
        } else {
            ui.divider
        })
        .cursor_text()
        .on_click(move |_, window, cx| window.focus(&handle, cx))
        .child(Icon::Search.svg().size(px(15.)).text_color(if focused { ui.accent_text } else { ui.faint }))
        .child(div().flex_1().min_w_0().child(input.clone()))
        .when(!empty, |d| {
            d.child(
                div()
                    .id(SharedString::from(format!("{id}-clear")))
                    .size(px(26.))
                    .flex_shrink_0()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(ui.field_hover))
                    .child(Icon::Close.svg().size(px(14.)).text_color(ui.muted))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        clear.update(cx, |i, cx| {
                            i.set_text("", cx);
                            cx.emit(crate::ui::text_input::TextInputEvent::Changed(String::new()));
                        })
                    }),
            )
        })
}

/// A filter chip: a toggle for what a list shows.
pub fn chip(
    ui: Ui,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    count: Option<usize>,
    selected: bool,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(12.))
        .rounded(px(8.))
        .border_1()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .when(selected, |d| d.pl(px(8.)).bg(ui.accent_soft).border_color(ui.accent_soft).text_color(ui.accent_text))
        .when(!selected, |d| d.border_color(ui.border).text_color(ui.muted).hover(|s| s.bg(ui.field).text_color(ui.text)))
        .when(selected, |d| d.child(Icon::Check.svg().size(px(14.)).text_color(ui.accent_text)))
        .child(label.into())
        .when_some(count, |d, n| d.child(div().text_color(if selected { ui.accent_text } else { ui.faint }).child(n.to_string())))
        .on_click(move |_, window, cx| f(window, cx))
}

/// A dialog over the window: a scrim and a centered card.
pub fn modal(ui: Ui, id: &str, title: impl Into<SharedString>, body: impl IntoElement, footer: impl IntoElement) -> Stateful<Div> {
    div().id(SharedString::from(id.to_string())).absolute().inset_0().bg(ui.scrim).occlude().flex().items_center().justify_center().child(
        div()
            .id(SharedString::from(format!("{id}-card")))
            .w(px(520.))
            .max_w(relative(0.9))
            .flex()
            .flex_col()
            .rounded(px(22.))
            .bg(ui.card)
            .border_1()
            .border_color(ui.divider)
            .shadow_2xl()
            .occlude()
            .child(
                div()
                    .px(px(24.))
                    .pt(px(22.))
                    .pb(px(6.))
                    .text_size(px(19.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ui.text)
                    .child(title.into()),
            )
            .child(div().px(px(24.)).py(px(12.)).child(body))
            .child(div().flex().justify_end().gap(px(8.)).px(px(20.)).pt(px(8.)).pb(px(20.)).child(footer)),
    )
}

/// A visual choice: a preview over a label, ringed when selected (cursor
/// shapes, tab styles). The caller adds `on_click`.
pub fn choice_card(ui: Ui, id: impl Into<ElementId>, label: &'static str, selected: bool, preview: impl IntoElement) -> Stateful<Div> {
    div()
        .id(id)
        .flex_1()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .overflow_hidden()
        .border_2()
        .border_color(if selected { ui.accent } else { ui.divider })
        .cursor_pointer()
        .hover(|s| s.border_color(if selected { ui.accent } else { ui.border }))
        .child(preview)
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .gap(px(6.))
                .h(px(32.))
                .bg(ui.card)
                .text_size(px(text::SMALL))
                .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                .text_color(if selected { ui.accent_text } else { ui.text })
                .when(selected, |d| d.child(Icon::Check.svg().size(px(13.)).text_color(ui.accent_text)))
                .child(label),
        )
}

/// A labelled field in a form.
pub fn field(ui: Ui, label: &str, input: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(
            div()
                .text_size(px(text::SMALL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.muted)
                .child(SharedString::from(label.to_owned())),
        )
        .child(input)
}

// ---- Dropdowns --------------------------------------------------------------------

/// One choice in a [`SettingsWindow::select`].
pub struct Opt<T> {
    pub value: T,
    pub label: SharedString,
    pub desc: Option<SharedString>,
}

pub fn opt<T>(value: T, label: impl Into<SharedString>) -> Opt<T> {
    Opt { value, label: label.into(), desc: None }
}

impl<T> Opt<T> {
    pub fn desc(mut self, d: impl Into<SharedString>) -> Self {
        self.desc = Some(d.into());
        self
    }
}

/// The trigger of a dropdown: the current value and an up/down chevron.
pub fn select_trigger(ui: Ui, id: SharedString, label: impl IntoElement, open: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(10.))
        .min_w(px(150.))
        .h(px(34.))
        .pl(px(14.))
        .pr(px(10.))
        .rounded(px(radius::CONTROL))
        .bg(if open { ui.field_hover } else { ui.field })
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ui.text)
        .cursor_pointer()
        .hover(|s| s.bg(ui.field_hover))
        .child(label)
        .child(Icon::ChevronsUpDown.svg().size(px(14.)).text_color(ui.muted))
}

/// The popover under a dropdown's trigger.
pub fn menu(ui: Ui, id: impl Into<ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_col()
        .min_w(px(240.))
        .max_w(px(400.))
        .max_h(px(380.))
        .overflow_y_scroll()
        .p(px(5.))
        .gap(px(1.))
        .rounded(px(14.))
        .bg(ui.menu)
        .border_1()
        .border_color(ui.border)
        .shadow_lg()
}

/// A choice in a menu: label, optional description, a check when selected.
pub fn menu_item(ui: Ui, id: impl Into<ElementId>, label: SharedString, desc: Option<SharedString>, selected: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(9.))
        .cursor_pointer()
        .hover(|s| s.bg(ui.field))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .font_weight(if selected { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(if selected { ui.accent_text } else { ui.text })
                        .child(label),
                )
                .when_some(desc, |d, t| d.child(div().text_size(px(text::CAPTION)).text_color(ui.muted).child(t))),
        )
        .child(div().w(px(16.)).flex_shrink_0().when(selected, |d| d.child(Icon::Check.svg().size(px(16.)).text_color(ui.accent_text))))
}

impl SettingsWindow {
    /// A trigger and, while it's open, its popover menu.
    pub(super) fn dropdown(&self, id: SharedString, trigger: Stateful<Div>, menu: impl IntoElement, cx: &Context<'_, Self>) -> AnyElement {
        let open = self.menu.as_ref() == Some(&id);
        let this = cx.entity().downgrade();
        let toggle_id = id.clone();
        let trigger = trigger.on_mouse_down(MouseButton::Left, move |_, _, cx| {
            let _ = this.update(cx, |v, cx| v.toggle_menu(toggle_id.clone(), cx));
        });
        let this = cx.entity().downgrade();
        div()
            .relative()
            .flex_shrink_0()
            .child(trigger)
            .when(open, |d| {
                d.child(
                    div().absolute().top(relative(1.)).right_0().pt(px(6.)).child(
                        deferred(
                            anchored().anchor(gpui::Anchor::TopRight).snap_to_window_with_margin(px(12.)).child(
                                div()
                                    .occlude()
                                    .on_mouse_down_out(move |_, _, cx| {
                                        let _ = this.update(cx, |v, cx| v.dismiss_menu(cx));
                                    })
                                    .child(menu),
                            ),
                        )
                        .with_priority(2),
                    ),
                )
            })
            .into_any_element()
    }

    /// A dropdown of `options`; `f` gets the chosen value.
    pub(super) fn select<T: Clone + PartialEq + 'static>(
        &self,
        id: &str,
        options: Vec<Opt<T>>,
        selected: &T,
        ui: Ui,
        cx: &Context<'_, Self>,
        f: impl Fn(T, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let id: SharedString = id.to_owned().into();
        let open = self.menu.as_ref() == Some(&id);
        let current = options.iter().find(|o| &o.value == selected).map(|o| o.label.clone()).unwrap_or_default();
        let f = Rc::new(f);
        let mut list = menu(ui, SharedString::from(format!("{id}-menu")));
        if open {
            for (i, o) in options.into_iter().enumerate() {
                let on = &o.value == selected;
                let (f, value, this) = (f.clone(), o.value, cx.entity().downgrade());
                list = list.child(menu_item(ui, SharedString::from(format!("{id}-opt-{i}")), o.label, o.desc, on).on_click(
                    move |_, window, cx| {
                        f(value.clone(), window, cx);
                        let _ = this.update(cx, |v, cx| {
                            v.menu = None;
                            cx.notify();
                        });
                    },
                ));
            }
        }
        self.dropdown(id.clone(), select_trigger(ui, id, div().child(current), open), list, cx)
    }
}
