//! The main window: title bar with the tab strip, toolbar, resizable sidebar and content, status bar.
//!
//! This is the skeleton's one real view; what is in it that isn't final says so. The component waves replace
//! the placeholders ([`Placeholder`]) one at a time and leave the rest alone:
//!
//! | Area                    | Now                                         | Replaced by                      |
//! |-------------------------|---------------------------------------------|----------------------------------|
//! | Title bar               | gpui-component's `TitleBar` (snap layouts work) | stays                        |
//! | Tab strip (in the bar)  | one tab-shaped chip                          | the tab strip wave               |
//! | Toolbar                 | back/forward/up, address, search            | address bar and search behavior  |
//! | Drive health button     | **temporary** (shows the exemplar's wiring) | This PC's drive tiles + banner   |
//! | Sidebar                 | placeholder, resizable                      | the places sidebar               |
//! | Content                 | placeholder                                 | the file list and grid           |
//! | Status bar              | placeholder                                 | item counts, selection, index    |
//!
//! **Patterns here for the waves to copy:**
//! - The root has `key_context("Workspace")` and tracks focus, so the key bindings in [`crate::actions`] are
//!   live; handlers are `.on_action(cx.listener(...))` on the root.
//! - Everything visible is a kit control or a themed div; text goes through `t!`, colors through
//!   [`theme::tokens`], spacing through [`space`]; rows that carry order use [`row_start`] so the Arabic
//!   interface mirrors (the sidebar goes to the other side, the back arrow points the other way).
//! - Icon-only buttons have an accessibility label *and* a tooltip, both translated.
//! - The window's background material (Mica) follows the theme: while it is on, chrome is see-through.
//! - Core events: [`core_bridge::subscribe`] wires a view to them (the copy queue's, below).

use crate::kit::prelude::*;
use crate::{actions, core_bridge, dialogs, t, theme};
use std::ops::Range;

/// The label glint-core knows the main window by.
pub const MAIN: &str = "main";

/// The sidebar's width: what it starts at, and how far it can be dragged.
const SIDEBAR: Range<Pixels> = px(160.)..px(420.);
const SIDEBAR_START: Pixels = px(240.);

/// The main window's content.
pub struct Workspace {
    focus: FocusHandle,
    address: Entity<InputState>,
    search: Entity<InputState>,
    /// Core's copy-queue events seen so far: shows the event wiring (the real queue UI replaces this).
    queue_events: usize,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    /// Builds the window's view: its inputs, its core wiring and its place in core's window list.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let address = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(t!("Type a path"))
                .default_value(t!("This PC"))
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Search this folder")));

        let mut subscriptions = Vec::new();
        // Core's events: every copy-queue change redraws the status bar. Handlers record and notify; GPUI
        // coalesces the redraw to one per frame however many events arrive.
        subscriptions.extend(core_bridge::subscribe(cx, |this: &mut Self, event, cx| {
            if event.name == "copy-queue" {
                this.queue_events += 1;
                cx.notify();
            }
        }));
        // Windows switched between light and dark: the "System" theme follows it.
        subscriptions.push(window.observe_window_appearance(|_, cx| theme::on_appearance_changed(cx)));
        // The theme changed (the setting, Windows' accent, Mica): the window's material follows, and it redraws.
        subscriptions.push(cx.observe_global_in::<theme::GlintTheme>(window, |_, window, cx| {
            window.set_background_appearance(theme::window_background(cx));
            cx.notify();
        }));

        core_bridge::register_window(cx, MAIN, window);
        // Leaving core's window list when this view goes away with its window.
        subscriptions.push(cx.on_release(|_, cx| core_bridge::unregister_window(cx, MAIN)));

        Self {
            focus: cx.focus_handle(),
            address,
            search,
            queue_events: 0,
            _subscriptions: subscriptions,
        }
    }

    /// The Ctrl+L action: into the address bar.
    fn focus_address(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.address.update(cx, |input, cx| input.focus(window, cx));
    }

    /// The Ctrl+F action: into the search box.
    fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.focus(window, cx));
    }

    /// The marker for an action whose component isn't built yet: a toast saying so, so the wiring is visible.
    fn not_yet(window: &mut Window, cx: &mut App, what: SharedString) {
        window.push_notification(
            Notification::info(t!("{what} comes with its part of the port.", what = what)),
            cx,
        );
    }

    fn title_bar(&self, t: &theme::Tokens) -> impl IntoElement {
        // The tab strip's place: one tab-shaped chip, named for screen readers, until the tab strip wave.
        TitleBar::new().child(
            row_start()
                .gap(space(1.))
                .child(
                    row_start()
                        .id("tab-strip-placeholder")
                        .role(Role::Tab)
                        .aria_label(t!("This PC"))
                        .gap(space(2.))
                        .h(space(7.))
                        .px(space(3.))
                        .rounded_t(theme::size::RADIUS)
                        .bg(t.background)
                        .text_size(theme::size::SM)
                        .child(icon(IconName::HardDrive).size_4().text_color(t.muted_foreground))
                        .child(t!("This PC")),
                )
                .child(
                    Button::new("new-tab")
                        .ghost()
                        .small()
                        .icon(IconName::Plus)
                        .accessibility_label(t!("New tab"))
                        .tooltip(t!("New tab"))
                        .on_click(|_, window, cx| Self::not_yet(window, cx, t!("Tabs"))),
                ),
        )
    }

    fn toolbar(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let nav = |id: &'static str, glyph: IconName, name: SharedString| {
            Button::new(id)
                .ghost()
                .small()
                .icon(glyph)
                .accessibility_label(name.clone())
                .tooltip(name)
        };
        row_start()
            .id("toolbar")
            .role(Role::Toolbar)
            .aria_label(t!("Navigation"))
            .test_support()
            .gap(space(1.))
            .px(space(2.))
            .py(space(1.5))
            .bg(theme::chrome_fill(cx))
            .border_b_1()
            .border_color(t.border)
            .child(
                nav("nav-back", back_icon(), t!("Back"))
                    .on_click(|_, window, cx| Self::not_yet(window, cx, t!("Back"))),
            )
            .child(
                nav("nav-forward", forward_icon(), t!("Forward"))
                    .on_click(|_, window, cx| Self::not_yet(window, cx, t!("Forward"))),
            )
            .child(
                nav("nav-up", IconName::ArrowUp, t!("Up"))
                    .on_click(|_, window, cx| Self::not_yet(window, cx, t!("Up"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .mx(space(1.))
                    .child(Input::new(&self.address).aria_label(t!("Address"))),
            )
            .child(
                div().w(space(64.)).flex_none().child(
                    Input::new(&self.search)
                        .aria_label(t!("Search"))
                        .prefix(icon(IconName::Search).size_4().text_color(t.muted_foreground))
                        .cleanable(true),
                ),
            )
            // TEMPORARY: shows how a dialog is opened. This PC's drive tiles and banner open it for real.
            .child(
                Button::new("drive-health")
                    .ghost()
                    .small()
                    .icon(IconName::HeartPulse)
                    .accessibility_label(t!("Drive health"))
                    .tooltip(t!("Drive health"))
                    .on_click(|_, window, cx| dialogs::drive_health::open(window, cx, None)),
            )
    }

    fn body(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = resizable_panel().size(SIDEBAR_START).size_range(SIDEBAR).fixed().child(
            div()
                .id("sidebar")
                .role(Role::Complementary)
                .aria_label(t!("Places"))
                .test_support()
                .size_full()
                .bg(theme::chrome_fill(cx))
                .child(Placeholder::new(
                    t!("Sidebar"),
                    t!("Replaced by the places sidebar: favorites, drives, tags"),
                )),
        );
        let content = resizable_panel().child(
            div()
                .id("content")
                .role(Role::Main)
                .aria_label(t!("Files"))
                .test_support()
                .size_full()
                .bg(t.background)
                .child(Placeholder::new(
                    t!("File list"),
                    t!("Replaced by the file list and grid"),
                )),
        );
        // The sidebar sits at the start edge: left in English, right in Arabic.
        let group = h_resizable("body");
        if crate::i18n::is_rtl() {
            group.child(content).child(sidebar)
        } else {
            group.child(sidebar).child(content)
        }
    }

    fn status_bar(&self, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let queue = (self.queue_events > 0).then(|| t!("Copy queue changed {n} times", n = self.queue_events));
        row_start()
            .id("status-bar")
            .role(Role::Status)
            .aria_label(t!("Status bar"))
            .test_support()
            .gap(space(3.))
            .h(space(6.))
            .px(space(3.))
            .bg(theme::chrome_fill(cx))
            .border_t_1()
            .border_color(t.border)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(t!(
                "Status bar: item counts, selection size and index state arrive here"
            ))
            .children(queue)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        // Over Mica the window itself is the backdrop; otherwise it is the page color.
        let fill = if theme::backdrop_on(cx) {
            hsla(0., 0., 0., 0.)
        } else {
            t.background
        };
        // Pixels in a rem follow the text-size setting; rem-based sizes (the type scale, `space`) scale with it.
        let rem = theme::rem_size(cx);
        if window.rem_size() != rem {
            window.set_rem_size(rem);
        }
        col()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .bg(fill)
            .text_color(t.foreground)
            .text_size(theme::size::SM)
            // Text starts at the reading direction's start edge (text alignment is inherited by everything below).
            .text_start()
            .on_action(cx.listener(|this, _: &actions::FocusAddress, window, cx| this.focus_address(window, cx)))
            .on_action(cx.listener(|this, _: &actions::SearchHere, window, cx| this.focus_search(window, cx)))
            .on_action(
                cx.listener(|_, _: &actions::ShowDriveHealth, window, cx| {
                    dialogs::drive_health::open(window, cx, None)
                }),
            )
            .on_action(cx.listener(|_, _: &actions::ShowShortcuts, window, cx| {
                Self::not_yet(window, cx, t!("Keyboard shortcuts"))
            }))
            .on_action(
                cx.listener(|_, _: &actions::OpenSettings, window, cx| Self::not_yet(window, cx, t!("Settings"))),
            )
            .on_action(cx.listener(|_, _: &actions::CommandPalette, window, cx| {
                Self::not_yet(window, cx, t!("The command palette"))
            }))
            .child(self.title_bar(&t))
            .child(self.toolbar(&t, cx))
            .child(div().flex_1().min_h_0().child(self.body(&t, cx)))
            .child(self.status_bar(&t, cx))
    }
}

/// Opens the main window, as Glint's first run or a launch does. Mica material, minimum size, the title bar
/// the kit draws. Window placement (the last session's size and place) comes with the placement port.
pub fn open_main_window(cx: &mut App) -> gpui_kit::Result<()> {
    let bounds = Bounds::centered(None, size(px(1240.), px(780.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Glint".into()),
            ..TitleBar::title_bar_options()
        }),
        window_background: theme::window_background(cx),
        window_min_size: Some(size(px(520.), px(360.))),
        ..TitleBar::window_options()
    };
    gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| Workspace::new(window, cx))).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{Bounds, TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext, rtl: bool) -> VisualTestContext {
        cx.update(testing::init);
        // After init, which starts every test in English.
        testing::set_rtl(rtl);
        testing::open_window(cx, |window, cx| cx.new(|cx| Workspace::new(window, cx))).0
    }

    /// The bounds of an element by its id (labels change with the language), from a freshly completed frame.
    fn bounds_of(cx: &mut VisualTestContext, id: &'static str) -> Bounds<Pixels> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find(id).bounds()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    #[gpui_kit::test]
    fn main_window_has_its_named_regions_and_the_drive_health_button_opens_the_dialog(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, false);
        assert_eq!(role_of(&mut visual, "Navigation"), Some(Role::Toolbar));
        assert_eq!(role_of(&mut visual, "Places"), Some(Role::Complementary));
        assert_eq!(role_of(&mut visual, "Files"), Some(Role::Main));
        assert_eq!(role_of(&mut visual, "Status bar"), Some(Role::Status));
        for icon_only in ["Back", "Forward", "Up", "New tab", "Drive health"] {
            assert_eq!(role_of(&mut visual, icon_only), Some(Role::Button), "{icon_only}");
        }
        assert_eq!(role_of(&mut visual, "Address"), Some(Role::TextInput));
        assert_eq!(role_of(&mut visual, "Search"), Some(Role::TextInput));
        // English reads left to right: back before forward, the sidebar before the files.
        assert!(bounds_of(&mut visual, "nav-back").origin.x < bounds_of(&mut visual, "nav-forward").origin.x);
        assert!(bounds_of(&mut visual, "sidebar").origin.x < bounds_of(&mut visual, "content").origin.x);

        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("drive-health", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn arabic_mirrors_the_window_and_still_names_everything(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = open(cx, true);
        // Back is to the right of Forward in a mirrored toolbar, and the sidebar goes to the other side.
        assert!(bounds_of(&mut visual, "nav-back").origin.x > bounds_of(&mut visual, "nav-forward").origin.x);
        assert!(bounds_of(&mut visual, "sidebar").origin.x > bounds_of(&mut visual, "content").origin.x);
        testing::set_rtl(false);
    }
}
