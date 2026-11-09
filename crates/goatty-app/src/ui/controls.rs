//! Small stateless controls shared by goatty's windows. Each takes the
//! colors and a callback; state lives in the settings, not in the control.
//!
//! The look is one system (see `.claude/rules/settings-ui.md`): pill-shaped
//! buttons and segmented controls, Material 3-style switches, keycaps for
//! shortcuts, the type scale and radii from [`super::style`].

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyView, App, Bounds, Context, DispatchPhase, Div, ElementId, FontWeight, Hsla, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Render, SharedString, Stateful, Window, canvas, div, prelude::*, px, relative,
};

use super::icons::Icon;
use super::style::{Ui, radius, text};

type Callback = Rc<dyn Fn(&mut Window, &mut App)>;

/// An on/off switch (Material 3: an outlined track with a small thumb when
/// off, a filled track with a bigger thumb when on).
pub fn toggle(id: impl Into<ElementId>, on: bool, ui: Ui, f: impl Fn(&mut Window, &mut App) + 'static) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .w(px(42.))
        .h(px(24.))
        .rounded_full()
        .border_2()
        .cursor_pointer()
        .when(on, |d| d.bg(ui.accent).border_color(ui.accent).justify_end().pr(px(2.)))
        .when(!on, |d| d.bg(ui.field).border_color(ui.border).pl(px(4.)).hover(|s| s.border_color(ui.muted)))
        .child(div().rounded_full().when(on, |d| d.size(px(16.)).bg(ui.on_accent)).when(!on, |d| d.size(px(12.)).bg(ui.muted)))
        .on_click(move |_, window, cx| f(window, cx))
}

/// A row of mutually exclusive choices.
pub fn segmented<T: Clone + PartialEq + 'static>(
    id: &str,
    options: &[(T, &str)],
    selected: &T,
    ui: Ui,
    f: impl Fn(T, &mut Window, &mut App) + 'static,
) -> Div {
    let f = Rc::new(f);
    div().flex().flex_shrink_0().p(px(3.)).gap(px(2.)).rounded_full().bg(ui.field).children(options.iter().enumerate().map(
        |(i, (value, label))| {
            let on = value == selected;
            let value = value.clone();
            let f = f.clone();
            div()
                .id(SharedString::from(format!("{id}-{i}")))
                .h(px(28.))
                .px(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(on, |d| d.bg(ui.accent).text_color(ui.on_accent))
                .when(!on, |d| d.text_color(ui.muted).hover(|s| s.bg(ui.field_hover).text_color(ui.text)))
                .child(SharedString::from(label.to_string()))
                .on_click(move |_, window, cx| f(value.clone(), window, cx))
        },
    ))
}

/// `[−] value [+]`.
pub fn stepper(
    id: &str,
    value: impl Into<SharedString>,
    ui: Ui,
    dec: impl Fn(&mut Window, &mut App) + 'static,
    inc: impl Fn(&mut Window, &mut App) + 'static,
) -> Div {
    let btn = |suffix: &str, icon: StepIcon, f: Callback| {
        div()
            .id(SharedString::from(format!("{id}-{suffix}")))
            .size(px(28.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .cursor_pointer()
            .text_color(ui.muted)
            .hover(|s| s.bg(ui.field_hover).text_color(ui.text))
            .child(match icon {
                // Drawn: matches the plus icon's 2px stroke.
                StepIcon::Minus => div().w(px(10.)).h(px(2.)).rounded_full().bg(ui.muted),
                StepIcon::Plus => div().child(Icon::Plus.svg().size(px(14.)).text_color(ui.muted)),
            })
            .on_click(move |_, window, cx| f(window, cx))
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .p(px(3.))
        .rounded_full()
        .bg(ui.field)
        .child(btn("dec", StepIcon::Minus, Rc::new(dec)))
        .child(
            div()
                .min_w(px(64.))
                .flex()
                .justify_center()
                .text_size(px(text::LABEL))
                .font_weight(FontWeight::MEDIUM)
                .text_color(ui.text)
                .child(value.into()),
        )
        .child(btn("inc", StepIcon::Plus, Rc::new(inc)))
}

enum StepIcon {
    Minus,
    Plus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Filled with the accent: the one main action of a view.
    Primary,
    /// Tonal: a filled neutral pill for ordinary actions.
    Normal,
    /// Text only until hovered: secondary actions in toolbars and rows.
    Ghost,
    /// Destructive.
    Danger,
}

/// A pill button.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    ui: Ui,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    button_with(id, None, label, kind, ui, f)
}

/// A pill button with a leading icon.
pub fn button_with(
    id: impl Into<ElementId>,
    icon: Option<Icon>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    ui: Ui,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let (bg, fg, hover) = match kind {
        ButtonKind::Primary => (ui.accent, ui.on_accent, ui.accent.opacity(0.88)),
        ButtonKind::Normal => (ui.field, ui.text, ui.field_hover),
        ButtonKind::Ghost => (ui.field.opacity(0.0), ui.text, ui.field),
        ButtonKind::Danger => (ui.danger.opacity(0.12), ui.danger, ui.danger.opacity(0.2)),
    };
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap(px(6.))
        .h(px(32.))
        .px(px(if icon.is_some() { 12. } else { 14. }))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when_some(icon, |d, i| d.child(i.svg().size(px(15.)).text_color(fg)))
        .child(label.into())
        .on_click(move |_, window, cx| f(window, cx))
}

/// A round icon-only button with a tooltip.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: Icon,
    tip: impl Into<SharedString>,
    ui: Ui,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    icon_button_tinted(id, icon, tip, None, ui, f)
}

/// [`icon_button`] with the icon in `tint` (a toggled-on state).
pub fn icon_button_tinted(
    id: impl Into<ElementId>,
    icon: Icon,
    tip: impl Into<SharedString>,
    tint: Option<Hsla>,
    ui: Ui,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let tip: SharedString = tip.into();
    let id: ElementId = id.into();
    // An svg doesn't inherit the text color: hover recolors it through the group.
    let group = SharedString::from(format!("icon-button-{}", hash_id(&id)));
    let svg = icon.svg().size(px(16.));
    let svg = match tint {
        Some(c) => svg.text_color(c),
        None => svg.text_color(ui.muted).group_hover(group.clone(), |s| s.text_color(ui.text)),
    };
    div()
        .id(id)
        .group(group)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(30.))
        .rounded_full()
        .cursor_pointer()
        .hover(|s| s.bg(ui.field))
        .child(svg)
        .tooltip(tooltip(tip))
        .on_click(move |_, window, cx| f(window, cx))
}

/// How a badge is tinted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Accent,
    Success,
    Warning,
    Danger,
}

impl Tone {
    pub fn color(self, ui: Ui) -> Hsla {
        match self {
            Tone::Neutral => ui.muted,
            Tone::Accent => ui.accent_text,
            Tone::Success => ui.success,
            Tone::Warning => ui.warning,
            Tone::Danger => ui.danger,
        }
    }
}

/// A small tinted label: status, counts.
pub fn badge(label: impl Into<SharedString>, tone: Tone, ui: Ui) -> Div {
    let c = tone.color(ui);
    div()
        .flex_shrink_0()
        .h(px(20.))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(if tone == Tone::Neutral { ui.field } else { c.opacity(0.14) })
        .text_color(c)
        .text_size(px(text::CAPTION))
        .font_weight(FontWeight::MEDIUM)
        .child(label.into())
}

/// A colored banner: an icon, a title and lines (warnings, errors, results).
pub fn banner(ui: Ui, tone: Tone, title: impl Into<SharedString>, lines: Vec<String>) -> Div {
    let c = tone.color(ui);
    let icon = match tone {
        Tone::Success => Icon::CircleCheck,
        Tone::Warning => Icon::TriangleAlert,
        Tone::Danger => Icon::CircleX,
        Tone::Neutral | Tone::Accent => Icon::Info,
    };
    div()
        .flex()
        .gap(px(12.))
        .p(px(14.))
        .rounded(px(radius::CARD))
        .bg(c.opacity(if ui.dark { 0.1 } else { 0.08 }))
        .border_1()
        .border_color(c.opacity(0.28))
        .child(icon.svg().size(px(18.)).mt(px(1.)).text_color(c))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .flex_1()
                .min_w_0()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).text_color(ui.text).child(title.into()))
                .children(lines.into_iter().map(|l| div().text_size(px(text::SMALL)).text_color(ui.muted).child(SharedString::from(l)))),
        )
}

/// A keyboard shortcut as keycaps: `ctrl-shift-t` → [Ctrl] [Shift] [T].
pub fn keycaps(keystroke: &str, ui: Ui) -> Div {
    div().flex().items_center().gap(px(3.)).children(key_parts(keystroke).into_iter().map(|k| keycap(k, ui)))
}

pub fn keycap(label: impl Into<SharedString>, ui: Ui) -> Div {
    div()
        .flex_shrink_0()
        .min_w(px(22.))
        .h(px(22.))
        .px(px(6.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(radius::SMALL))
        .bg(ui.field)
        .border_1()
        .border_b_2()
        .border_color(ui.border)
        .text_size(px(text::CAPTION))
        .font_weight(FontWeight::MEDIUM)
        .text_color(ui.text)
        .child(label.into())
}

/// A horizontal slider over `0.0..=1.0`. `f(value, done)` runs while
/// dragging (`done == false`, for a live preview) and once more when the
/// mouse is released (`done == true`, to save).
pub fn slider(id: impl Into<ElementId>, value: f32, ui: Ui, f: impl Fn(f32, bool, &mut Window, &mut App) + 'static) -> impl IntoElement {
    let id: ElementId = id.into();
    let key = hash_id(&id);
    let f = Rc::new(f);
    let t = value.clamp(0.0, 1.0);
    const THUMB: f32 = 18.0;
    div()
        .id(id)
        .relative()
        .w(px(180.))
        .h(px(28.))
        .flex_shrink_0()
        .cursor_pointer()
        .child(
            // Track, inset by half a thumb on both ends.
            div()
                .absolute()
                .left(px(THUMB / 2.))
                .right(px(THUMB / 2.))
                .top(px(11.))
                .h(px(6.))
                .rounded_full()
                .bg(ui.field_hover)
                .child(div().h_full().w(relative(t)).rounded_full().bg(ui.accent)),
        )
        .child(
            div().absolute().top(px(5.)).left(px(THUMB / 2.)).right(px(THUMB / 2.)).h(px(THUMB)).child(
                div()
                    .absolute()
                    .left(relative(t))
                    .ml(px(-THUMB / 2.))
                    .size(px(THUMB))
                    .rounded_full()
                    .bg(ui.accent)
                    .border_3()
                    .border_color(ui.card)
                    .shadow_sm(),
            ),
        )
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let f = f.clone();
                    slider_listeners(window, bounds, key, THUMB, f);
                },
            )
            .absolute()
            .size_full(),
        )
}

thread_local! {
    /// The slider being dragged (only one at a time; the UI is one thread).
    static DRAGGING: Cell<Option<u64>> = const { Cell::new(None) };
}

type SliderFn = Rc<dyn Fn(f32, bool, &mut Window, &mut App)>;

fn slider_listeners(window: &mut Window, bounds: Bounds<Pixels>, key: u64, thumb: f32, f: SliderFn) {
    let at = move |x: Pixels| {
        let w = f32::from(bounds.size.width) - thumb;
        ((f32::from(x - bounds.left()) - thumb / 2.0) / w.max(1.0)).clamp(0.0, 1.0)
    };
    let g = f.clone();
    window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left && bounds.contains(&ev.position) {
            DRAGGING.set(Some(key));
            g(at(ev.position.x), false, window, cx);
        }
    });
    let g = f.clone();
    window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
        if phase != DispatchPhase::Bubble || DRAGGING.get() != Some(key) {
            return;
        }
        if ev.pressed_button == Some(MouseButton::Left) {
            g(at(ev.position.x), false, window, cx);
        } else {
            // Released somewhere we couldn't hear.
            DRAGGING.set(None);
            g(at(ev.position.x), true, window, cx);
        }
    });
    window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
        if phase == DispatchPhase::Bubble && ev.button == MouseButton::Left && DRAGGING.get() == Some(key) {
            DRAGGING.set(None);
            f(at(ev.position.x), true, window, cx);
        }
    });
}

fn hash_id(id: &ElementId) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut h);
    h.finish()
}

/// A tooltip builder for `.tooltip(…)`.
pub fn tooltip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| cx.new(|_| Tooltip(text.clone())).into()
}

pub struct Tooltip(SharedString);

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let ui = Ui::get(cx);
        div()
            .font_family(cx.global::<crate::settings::AppSettings>().ui_font.clone())
            .max_w(px(320.))
            .px(px(10.))
            .py(px(6.))
            .rounded(px(8.))
            .bg(ui.menu)
            .border_1()
            .border_color(ui.border)
            .shadow_md()
            .text_size(px(text::SMALL))
            .text_color(ui.text)
            .child(self.0.clone())
    }
}

/// The parts of a keystroke as shown on keycaps: `ctrl-shift--` → Ctrl, Shift, -.
pub fn key_parts(k: &str) -> Vec<String> {
    let parts: Vec<&str> = match k.strip_suffix("--") {
        Some(rest) => {
            let mut p: Vec<&str> = rest.split('-').collect();
            p.push("-");
            p
        }
        None => k.split('-').collect(),
    };
    parts
        .iter()
        .map(|p| match *p {
            "ctrl" => "Ctrl".to_string(),
            "shift" => "Shift".to_string(),
            "alt" => "Alt".to_string(),
            "super" | "cmd" | "platform" => "Super".to_string(),
            "pageup" => "PageUp".to_string(),
            "pagedown" => "PageDown".to_string(),
            "escape" => "Esc".to_string(),
            other if other.len() == 1 => other.to_uppercase(),
            other => {
                let mut c = other.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect()
}

/// `ctrl-shift-c` → `Ctrl+Shift+C`.
pub fn pretty_keys(k: &str) -> String {
    key_parts(k).join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        assert_eq!(pretty_keys("ctrl-shift-c"), "Ctrl+Shift+C");
        assert_eq!(pretty_keys("ctrl-shift--"), "Ctrl+Shift+-");
        assert_eq!(pretty_keys("shift-pageup"), "Shift+PageUp");
        assert_eq!(pretty_keys("ctrl-,"), "Ctrl+,");
        assert_eq!(key_parts("alt-shift-left"), ["Alt", "Shift", "Left"]);
    }
}
