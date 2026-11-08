//! Small stateless controls for settings pages. Each takes the colors and
//! a callback; state lives in the settings, not in the control.

use std::rc::Rc;

use gpui::{AnyElement, App, Div, ElementId, FontWeight, SharedString, Window, div, prelude::*, px};

use super::style::Ui;

type Callback = Rc<dyn Fn(&mut Window, &mut App)>;

/// An on/off switch.
pub fn toggle(id: impl Into<ElementId>, on: bool, ui: Ui, f: impl Fn(&mut Window, &mut App) + 'static) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .w(px(38.))
        .h(px(22.))
        .rounded_full()
        .p(px(3.))
        .cursor_pointer()
        .bg(if on { ui.accent } else { ui.border })
        .when(on, |d| d.justify_end())
        .child(div().size(px(16.)).rounded_full().bg(if on { ui.on_accent } else { ui.bg }))
        .on_click(move |_, window, cx| f(window, cx))
}

/// A row of mutually exclusive choices.
pub fn segmented<T: Clone + PartialEq + 'static>(
    id: &str,
    options: &[(T, &str)],
    selected: &T,
    ui: Ui,
    f: impl Fn(T, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let f = Rc::new(f);
    div().flex().flex_shrink_0().rounded_md().border_1().border_color(ui.border).overflow_hidden().children(options.iter().enumerate().map(
        |(i, (value, label))| {
            let on = value == selected;
            let value = value.clone();
            let f = f.clone();
            div()
                .id(format!("{id}-{i}"))
                .px_3()
                .py_1()
                .text_sm()
                .cursor_pointer()
                .when(i > 0, |d| d.border_l_1().border_color(ui.border))
                .when(on, |d| d.bg(ui.accent).text_color(ui.on_accent))
                .when(!on, |d| d.hover(|s| s.bg(ui.surface_hover)))
                .child(SharedString::from(label.to_string()))
                .on_click(move |_, window, cx| f(value.clone(), window, cx))
        },
    ))
}

/// `[-] value [+]`.
pub fn stepper(
    id: &str,
    value: impl Into<SharedString>,
    ui: Ui,
    dec: impl Fn(&mut Window, &mut App) + 'static,
    inc: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let btn = |suffix: &str, label: &'static str, f: Callback| {
        div()
            .id(format!("{id}-{suffix}"))
            .w(px(28.))
            .flex()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(ui.surface_hover))
            .child(label)
            .on_click(move |_, window, cx| f(window, cx))
    };
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .rounded_md()
        .border_1()
        .border_color(ui.border)
        .child(btn("dec", "−", Rc::new(dec)))
        .child(div().min_w(px(64.)).flex().justify_center().text_sm().child(value.into()))
        .child(btn("inc", "+", Rc::new(inc)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    Primary,
    Normal,
    Danger,
}

pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    ui: Ui,
    f: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (bg, fg, border) = match kind {
        ButtonKind::Primary => (ui.accent, ui.on_accent, ui.accent),
        ButtonKind::Normal => (ui.surface, ui.text, ui.border),
        ButtonKind::Danger => (ui.surface, ui.danger, ui.danger.opacity(0.5)),
    };
    div()
        .id(id)
        .flex_shrink_0()
        .px_3()
        .py_1()
        .rounded_md()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_sm()
        .cursor_pointer()
        .hover(|s| s.opacity(0.85))
        .child(label.into())
        .on_click(move |_, window, cx| f(window, cx))
}

/// A setting: label and description on the left, the control on the right.
pub fn row(ui: Ui, label: &str, description: Option<&str>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .py_2()
        .border_b_1()
        .border_color(ui.border.opacity(0.5))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(div().text_color(ui.text).child(SharedString::from(label.to_string())))
                .when_some(description, |d, desc| {
                    d.child(div().text_xs().text_color(ui.muted).child(SharedString::from(desc.to_string())))
                }),
        )
        .child(control)
}

pub fn section(ui: Ui, title: &str) -> Div {
    div().pt_5().pb_1().text_xs().font_weight(FontWeight::BOLD).text_color(ui.muted).child(SharedString::from(title.to_uppercase()))
}

/// A muted explanatory line.
pub fn note(ui: Ui, text: impl Into<SharedString>) -> Div {
    div().py_1().text_xs().text_color(ui.muted).child(text.into())
}

/// A colored notice box (warnings, errors, results).
pub fn notice(ui: Ui, color: gpui::Hsla, lines: Vec<String>) -> AnyElement {
    div()
        .my_2()
        .p_2()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(0.6))
        .bg(color.opacity(0.08))
        .text_sm()
        .text_color(ui.text)
        .flex()
        .flex_col()
        .gap_1()
        .children(lines.into_iter().map(|l| div().child(SharedString::from(l))))
        .into_any_element()
}

/// `ctrl-shift-c` → `Ctrl+Shift+C`.
pub fn pretty_keys(k: &str) -> String {
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
            other if other.len() == 1 => other.to_uppercase(),
            other => {
                let mut c = other.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join("+")
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
    }
}
