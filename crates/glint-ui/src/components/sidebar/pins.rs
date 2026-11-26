//! A pinned place's name in the sidebar ([#10](https://github.com/shadijhade/glint/issues/10): a NAS share,
//! `\\192.168.1.6\share`, as "nShare"). **Rename in sidebar…** on a pinned place's menu ([`Task::NamePin`]) asks
//! for it here. The name is the saved state's (`AppState::pin_names`) and only the sidebar shows it: the place
//! keeps its path, which its tooltip and description show, and unpinning forgets the name.
//!
//! [`Task::NamePin`]: crate::components::menus::Task::NamePin

use super::*;

/// "Rename in sidebar…": asks what the pinned place `path` is called in the sidebar. Nothing, or the place's own
/// name, gives it back its own.
pub fn ask_name(window: &mut Window, cx: &mut App, path: String) {
    let given = persistence::app(cx).and_then(|app| app.pin_name(&path).map(str::to_string));
    let view = cx.new(|cx| PinName::new(path, given, window, cx));
    let input = view.read(cx).input.clone();
    crate::dialogs::show(window, cx, t!("Rename in sidebar"), px(420.), view);
    window.focus(&input.focus_handle(cx), cx);
}

/// The box that asks for the name.
struct PinName {
    path: String,
    /// What the place is called without a name of its own.
    own: String,
    input: Entity<InputState>,
    _subscription: Subscription,
}

impl PinName {
    fn new(path: String, given: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let own = rows::own_name(&path);
        let shown = given.unwrap_or_else(|| own.clone());
        let placeholder = own.clone();
        let input = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder(placeholder);
            state.set_value(shown, window, cx);
            state.select_all(window, cx);
            state
        });
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.accept(window, cx);
            }
        });
        PinName {
            path,
            own,
            input,
            _subscription: subscription,
        }
    }

    /// Keeps the name typed and closes the box. The place's own name is no name of its own.
    fn accept(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = glint_model::store::clean_pin_name(&self.input.read(cx).value());
        let name = if typed == self.own { String::new() } else { typed };
        let path = self.path.clone();
        window.close_dialog(cx);
        persistence::update_app(cx, |app| {
            app.name_pin(&path, &name);
        });
    }
}

impl Render for PinName {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        col()
            .id("pin-name")
            .role(Role::Group)
            .aria_label(t!("Rename in sidebar"))
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::SM)
            .text_start()
            .pt(space(1.))
            // Which place it is: its path, as its tooltip says it.
            .child(
                div()
                    .id("pin-name-path")
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .truncate()
                    .child(crate::kit::ltr_unit(self.path.clone())),
            )
            .child(Input::new(&self.input).aria_label(t!("Name in the sidebar")))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("Leave it empty to use its own name.")),
            )
            .child(
                row_start()
                    .justify_end()
                    .gap(space(2.))
                    .child(
                        Button::new("pin-name-cancel")
                            .ghost()
                            .label(t!("Cancel"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("pin-name-ok")
                            .primary()
                            .label(t!("Rename"))
                            .on_click(cx.listener(|this, _, window, cx| this.accept(window, cx))),
                    ),
            )
    }
}
