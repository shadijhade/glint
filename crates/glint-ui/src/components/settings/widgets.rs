//! Settings' building blocks: sections, rows, switches and segmented choices (`Section`, `Row`, `SwitchRow`,
//! `Segmented` of SettingsView.tsx), drawn with the kit, theme tokens and logical (start/end) layout.
//!
//! Every control has a role and a name, and a row with a hint gives it to the control as its description, so a
//! screen reader says "Show hidden items, on. Files and folders Windows marks as hidden".

use crate::kit::prelude::*;
use crate::{t, theme};
use std::rc::Rc;

/// What a control does when changed, with the window and app it is changed in.
pub type OnChange<T> = Rc<dyn Fn(T, &mut Window, &mut App)>;

/// A name for an element: `settings-theme-0`.
pub fn id(parts: &[&str]) -> SharedString {
    SharedString::from(parts.join("-"))
}

/// A section: its title (and an optional action at the other end of it) over a card of rows divided by lines.
/// `slot` names it for the section list and for scrolling to it (`settings-about`).
pub fn section(
    slot: &str,
    title: SharedString,
    action: Option<AnyElement>,
    children: Vec<AnyElement>,
    t: &theme::Tokens,
) -> AnyElement {
    let divided = children.into_iter().enumerate().map(|(i, child)| {
        div()
            .when(i > 0, |this| this.border_t_1().border_color(t.border))
            .child(child)
    });
    col()
        .id(id(&["settings", slot]))
        .role(Role::Group)
        .aria_label(title.clone())
        .test_support()
        .w_full()
        .child(
            row_start()
                .h(space(7.))
                .px(space(1.))
                .mb(space(2.))
                .justify_between()
                .child(
                    div()
                        .text_size(theme::size::SM)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .children(action),
        )
        .child(
            col()
                .w_full()
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .overflow_hidden()
                .children(divided),
        )
        .into_any_element()
}

/// A label with a hint under it, the way every row starts.
fn label_block(label: SharedString, hint: Option<SharedString>, muted: bool, t: &theme::Tokens) -> Div {
    col()
        .min_w_0()
        .align_start()
        .child(
            div()
                .text_size(theme::size::SM)
                .text_start()
                .when(muted, |this| this.text_color(t.muted_foreground))
                .child(label),
        )
        .children(hint.map(|hint| {
            div()
                .mt(space(0.5))
                .text_size(theme::size::XS)
                .text_start()
                .text_color(t.muted_foreground)
                .child(hint)
        }))
}

/// A label and hint at the start, and what is chosen at the end; in a narrow window the choice wraps under the
/// label rather than squeezing it (`Row`).
pub fn setting_row(
    label: SharedString,
    hint: Option<SharedString>,
    controls: Vec<AnyElement>,
    t: &theme::Tokens,
) -> AnyElement {
    row_start()
        .id(id(&["settings-row", &label]))
        .role(Role::Group)
        .aria_label(label.clone())
        .when_some(hint.clone(), |this, hint| this.aria_description(hint))
        .test_support()
        .flex_wrap()
        .justify_between()
        .gap_x(space(6.))
        .gap_y(space(2.))
        .px(space(4.))
        .py(space(3.))
        .child(label_block(label, hint, false, t).flex_1().min_w(space(48.)))
        .child(row_start().flex_none().flex_wrap().gap(space(1.5)).children(controls))
        .into_any_element()
}

/// A paragraph of fine print inside a card (`px-4 py-3 text-xs text-muted-foreground`).
pub fn note(text: impl Into<SharedString>, t: &theme::Tokens) -> Div {
    div()
        .px(space(4.))
        .py(space(3.))
        .text_size(theme::size::XS)
        .text_start()
        .text_color(t.muted_foreground)
        .child(text.into())
}

/// A labelled on/off row; the label toggles it too (`SwitchRow`).
pub fn switch_row(
    slot: &str,
    label: SharedString,
    hint: Option<SharedString>,
    checked: bool,
    disabled: bool,
    on_change: impl Fn(bool, &mut Window, &mut App) + 'static,
    t: &theme::Tokens,
) -> Div {
    let on_change: OnChange<bool> = Rc::new(on_change);
    let toggled = on_change.clone();
    let switch_id = id(&["settings-switch", slot]);
    let switch = Switch::new(switch_id.clone())
        .checked(checked)
        .disabled(disabled)
        .accessibility_label(label.clone())
        .on_click(move |value, window, cx| on_change(*value, window, cx));
    let text = label_block(label, hint, disabled, t)
        .id(id(&["settings-label", slot]))
        .test_support()
        .flex_1()
        .when(!disabled, |this| this.cursor_pointer())
        .on_click(move |_, window, cx| {
            if !disabled {
                toggled(!checked, window, cx);
            }
        });
    row_start()
        .justify_between()
        .gap(space(6.))
        .px(space(4.))
        .py(space(3.))
        .child(text)
        .child(div().flex_none().child(switch))
}

/// A small outlined button, with an icon when it has one.
pub fn button(slot: &str, label: SharedString, icon_name: Option<IconName>) -> Button {
    let button = Button::new(id(&["settings-button", slot]))
        .label(label)
        .outline()
        .small();
    match icon_name {
        Some(name) => button.icon(name),
        None => button,
    }
}

/// A small filled (primary) button.
pub fn primary_button(slot: &str, label: SharedString, icon_name: Option<IconName>) -> Button {
    let button = Button::new(id(&["settings-button", slot]))
        .label(label)
        .primary()
        .small();
    match icon_name {
        Some(name) => button.icon(name),
        None => button,
    }
}

/// A small button without a border (Remove, Undo).
pub fn quiet_button(slot: &str, label: SharedString) -> Button {
    Button::new(id(&["settings-button", slot])).label(label).ghost().small()
}

/// A row of options of which one is chosen, drawn as one pill (`Segmented`). `name` is what the choice is
/// called for a screen reader; each option is a radio button that says whether it is the chosen one.
pub fn segmented<T: PartialEq + Clone + 'static>(
    slot: &str,
    name: SharedString,
    current: &T,
    options: Vec<(T, SharedString)>,
    on_change: impl Fn(T, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = theme::tokens(cx);
    let on_change: OnChange<T> = Rc::new(on_change);
    row_start()
        .id(id(&["settings-choice", slot]))
        .role(Role::RadioGroup)
        .aria_label(name)
        .test_support()
        .p(space(0.5))
        .gap(space(0.5))
        .rounded(theme::size::RADIUS)
        .bg(t.foreground.opacity(0.05))
        .children(options.into_iter().enumerate().map(|(i, (option, label))| {
            let chosen = &option == current;
            let on_change = on_change.clone();
            Button::new(id(&["settings-choice", slot, &i.to_string()]))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(if chosen { t.background } else { t.background.opacity(0.) })
                        .foreground(if chosen { t.foreground } else { t.muted_foreground })
                        .hover(if chosen { t.background } else { t.accent })
                        .active(if chosen { t.background } else { t.accent }),
                )
                .role(Role::RadioButton)
                .accessibility_label(label.clone())
                .toggled(chosen)
                .label(label)
                .xsmall()
                .on_click(move |_, window, cx| on_change(option.clone(), window, cx))
        }))
        .into_any_element()
}

/// A chip: a pattern or a name with a cross that takes it out (`PatternEditor`'s chips).
pub fn chip(
    slot: &str,
    text: SharedString,
    remove_label: SharedString,
    mono: SharedString,
    on_remove: impl Fn(&mut Window, &mut App) + 'static,
    t: &theme::Tokens,
) -> Div {
    row_start()
        .gap(space(1.))
        .py(space(0.5))
        .ps(2.)
        .pe(0.5)
        .rounded(theme::size::RADIUS)
        .border_1()
        .border_color(t.border)
        .bg(t.background)
        .text_size(theme::size::XS)
        .font_family(mono)
        .child(text)
        .child(
            Button::new(id(&["settings-chip", slot]))
                .ghost()
                .xsmall()
                .icon(IconName::X)
                .accessibility_label(remove_label)
                .on_click(move |_, window, cx| on_remove(window, cx)),
        )
}

/// Says something went well: a title, and a line under it when there's more to say (`toast.success`).
pub fn good(window: &mut Window, cx: &mut App, title: SharedString, description: Option<SharedString>) {
    let note = Notification::success(description.unwrap_or_default()).title(title);
    window.push_notification(note, cx);
}

/// Says something failed, and why (`toast.error(title, { description })`).
pub fn bad(window: &mut Window, cx: &mut App, title: SharedString, reason: impl Into<SharedString>) {
    window.push_notification(Notification::error(reason).title(title), cx);
}

/// A switch's handler that changes the view it was made in: `toggler(cx, |this, on, window, cx| ..)`.
pub fn toggler<V: 'static>(
    cx: &Context<V>,
    change: impl Fn(&mut V, bool, &mut Window, &mut Context<V>) + 'static,
) -> impl Fn(bool, &mut Window, &mut App) + 'static {
    let this = cx.weak_entity();
    move |on, window, cx| {
        this.update(cx, |this, cx| change(this, on, window, cx)).ok();
    }
}

/// A folder or a path on its own line, cut short with an ellipsis, with a button at the end (a folder to
/// take out, a PC to forget). `i` numbers the line among its kind.
pub fn path_line(slot: &str, i: usize, text: String, action: Button) -> AnyElement {
    let title: SharedString = text.clone().into();
    row_start()
        .id(id(&["settings-path", slot, &i.to_string()]))
        .role(Role::ListItem)
        .aria_label(title.clone())
        .test_support()
        .justify_between()
        .gap(space(2.))
        .text_size(theme::size::SM)
        .child(div().flex_1().min_w_0().truncate().text_start().child(isolate(title)))
        .child(div().flex_none().child(action))
        .into_any_element()
}

/// Asks a question in a dialog above Settings and runs `on_ok` only if the person says yes (the Tauri
/// `confirm`). Escape and Cancel close it and do nothing.
pub fn confirm(
    window: &mut Window,
    cx: &mut App,
    title: SharedString,
    message: SharedString,
    ok_label: SharedString,
    on_ok: impl Fn(&mut Window, &mut App) + 'static,
) {
    let on_ok = Rc::new(on_ok);
    window.open_dialog(cx, move |dialog, _window, _cx| {
        let on_ok = on_ok.clone();
        dialog
            .title(title.clone())
            .w(px(440.))
            .child(
                div()
                    .id("settings-confirm")
                    .role(Role::Group)
                    .aria_label(title.clone())
                    .aria_description(message.clone())
                    .test_support()
                    .text_start()
                    .child(message.clone()),
            )
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("settings-confirm-cancel")
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("settings-confirm-ok")
                            .primary()
                            .label(ok_label.clone())
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                on_ok(window, cx);
                            }),
                    ),
            )
    });
}

/// What a row is called: its element name, its label and the hint under it.
pub struct Named {
    pub slot: String,
    pub label: SharedString,
    pub hint: Option<SharedString>,
}

/// A row's name, label and hint together (they travel as one).
pub fn named(slot: &str, label: SharedString, hint: Option<SharedString>) -> Named {
    Named {
        slot: slot.to_string(),
        label,
        hint,
    }
}
