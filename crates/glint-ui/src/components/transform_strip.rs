//! The transform strip: what a drag of pictures or videos out of a pane delivers, chosen while it is under way.
//!
//! A drag out of a pane that holds only pictures, or only videos, offers the transforms
//! `glint_core::virtual_drag::transforms_for` lists (Clean EXIF, WebP, under 10 MB, Extract MP3) besides the
//! files as they are. An edge-docked strip in the window names them; the one in force is filled, and under it is
//! the name the drop will make. Two ways to choose, both while the button is down:
//!
//! - **Tab** (Shift+Tab goes back) cycles them. OLE's drag loop takes keyboard messages away from every window, so
//!   no key ever reaches GPUI: the drop source reads Tab in `QueryContinueDrag` (see `virtual_drag`).
//! - **The pointer** over a chip chooses it. While the drag is over the window, OLE sends the pointer to Glint's
//!   drop target (`platform::dragdrop`), which hands it to GPUI as a file drag: a chip answers
//!   `.on_drag_move::<ExternalPaths>` like any other drop zone.
//!
//! **What is on screen while the loop runs** is what the drop target's events and GPUI's tasks make of it:
//! OLE's loop dispatches the messages that are not input (GPUI's own wake-ups among them), which is how a drag over
//! the tab strip already lights its drop zones. The strip is such a zone and nothing more.
//!
//! The strip is a window-wide singleton the workspace mounts. A drag-out reaches it through [`offer`], which
//! needs no entity: a global holds a channel into it.
//!
//! **Limits.** Once the drag has left the window in a transform, coming back over a pane finds no files in the
//! data (they are hidden so a target takes the transformed copy), so the panes refuse it and hovering a chip no
//! longer works; Tab still does. Nothing is written until a target asks for the bytes.

use crate::kit::prelude::*;
use crate::{t, theme};
use glint_core::virtual_drag::{transformed_filename, transforms_for, DragSwitch, DragTransform};
use glint_model::paths::{base_name, is_remote_path};
use std::sync::Arc;

/// What the drag tells the strip.
enum Msg {
    /// A drag began with `switch`; `first` is the first dragged file's path and `count` how many there are.
    Show {
        switch: Arc<DragSwitch>,
        first: String,
        count: usize,
    },
    /// The drag's choice changed (by Tab, or the strip itself).
    Selected(usize),
    /// The drag ended.
    Hide,
}

struct Bus(async_channel::Sender<Msg>);
impl Global for Bus {}

/// A drag-out that offers transforms: keep it for as long as the drag lasts. The strip goes when it is dropped.
pub struct Offer {
    /// What a drop delivers; hand it to `virtual_drag::start_switchable_drag`.
    pub switch: Arc<DragSwitch>,
    bus: async_channel::Sender<Msg>,
}

impl Drop for Offer {
    fn drop(&mut self) {
        let _ = self.bus.try_send(Msg::Hide);
    }
}

/// The strip for a drag of `paths`, when there is something to offer: pictures only or videos only (not on a
/// remote), and a window with a strip mounted. Shows it at once. `None` means a plain drag.
pub fn offer(cx: &App, paths: &[String]) -> Option<Offer> {
    let Bus(bus) = cx.try_global::<Bus>()?;
    if paths.iter().any(|p| is_remote_path(p)) {
        return None;
    }
    let transforms = transforms_for(paths);
    if transforms.is_empty() {
        return None;
    }
    let switch = DragSwitch::new(transforms);
    let told = bus.clone();
    switch.on_change(move |index| {
        let _ = told.try_send(Msg::Selected(index));
    });
    bus.try_send(Msg::Show {
        switch: switch.clone(),
        first: paths.first().cloned().unwrap_or_default(),
        count: paths.len(),
    })
    .ok()?;
    Some(Offer {
        switch,
        bus: bus.clone(),
    })
}

struct Active {
    switch: Arc<DragSwitch>,
    first: String,
    count: usize,
    selected: usize,
}

/// The strip: nothing until a drag offers transforms.
pub struct TransformStrip {
    active: Option<Active>,
    /// Which drag the strip is showing for: keys its arrival, so it plays once for each.
    arrivals: u64,
    _bus: Task<()>,
}

impl TransformStrip {
    /// Makes the strip and opens the channel drags reach it by.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (sender, receiver) = async_channel::unbounded();
        cx.set_global(Bus(sender));
        let bus = cx.spawn(async move |this, cx| {
            while let Ok(msg) = receiver.recv().await {
                if this.update(cx, |this, cx| this.hear(msg, cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            active: None,
            arrivals: 0,
            _bus: bus,
        }
    }

    fn hear(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Show { switch, first, count } => {
                self.arrivals += 1;
                self.active = Some(Active {
                    selected: switch.selected(),
                    switch,
                    first,
                    count,
                });
            }
            Msg::Selected(index) => match &mut self.active {
                Some(active) if active.selected != index => active.selected = index,
                _ => return,
            },
            Msg::Hide => {
                if self.active.take().is_none() {
                    return;
                }
            }
        }
        cx.notify();
    }

    /// The pointer is over chip `index`: that is what the drop delivers now.
    fn pick(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(active) = &mut self.active else { return };
        if active.selected == index {
            return;
        }
        active.switch.select(index);
        active.selected = index;
        cx.notify();
    }

    /// Whether the strip is up (a drag offering transforms is under way).
    pub fn shown(&self) -> bool {
        self.active.is_some()
    }

    /// The index of the choice in force, while the strip is up.
    pub fn selected(&self) -> Option<usize> {
        self.active.as_ref().map(|a| a.selected)
    }
}

/// A choice's name on its chip.
fn label(option: Option<&DragTransform>) -> SharedString {
    match option {
        None => t!("As they are"),
        Some(DragTransform::StripMetadata) => t!("Clean EXIF"),
        Some(DragTransform::ConvertWebp) => t!("WebP"),
        Some(DragTransform::ConvertPng) => t!("PNG"),
        Some(DragTransform::ShrinkUnderMb(mb)) => t!("Under {mb} MB", mb = mb),
        Some(DragTransform::ExtractAudioMp3) => t!("Extract MP3"),
    }
}

/// What a choice does, for a screen reader and the chip's tooltip.
fn detail(option: Option<&DragTransform>) -> SharedString {
    match option {
        None => t!("Drop the files themselves"),
        Some(DragTransform::StripMetadata) => t!("A copy without camera, place or editing details"),
        Some(DragTransform::ConvertWebp) => t!("A copy as a WebP picture"),
        Some(DragTransform::ConvertPng) => t!("A copy as a PNG picture"),
        Some(DragTransform::ShrinkUnderMb(_)) => t!("A smaller copy of the video"),
        Some(DragTransform::ExtractAudioMp3) => t!("The sound as an MP3"),
    }
}

/// The name the drop makes for the first dragged file under `option`.
fn output_name(first: &str, option: Option<&DragTransform>) -> String {
    match option {
        Some(transform) => transformed_filename(first, transform),
        None => base_name(first).to_string(),
    }
}

impl Render for TransformStrip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(active) = &self.active else {
            // Nothing in the layout while no drag offers anything.
            return div().absolute().size_0().into_any_element();
        };
        let t = theme::tokens(cx).clone();
        let options = active.switch.options();
        let current = active.selected.min(options.len().saturating_sub(1));
        let chosen = options.get(current).and_then(Option::as_ref);

        let chips: Vec<_> = options
            .iter()
            .enumerate()
            .map(|(i, option)| {
                let on = i == current;
                let name = label(option.as_ref());
                row_start()
                    .id(("transform-chip", i))
                    .role(Role::RadioButton)
                    .aria_label(name.clone())
                    .aria_description(detail(option.as_ref()))
                    .aria_selected(on)
                    .test_support()
                    .h(space(8.))
                    .px(space(2.5))
                    .gap(space(2.))
                    .items_center()
                    .rounded(theme::size::RADIUS_ROW)
                    .when(on, |chip| chip.bg(t.selected).font_weight(FontWeight::MEDIUM))
                    .text_color(if on { t.foreground } else { t.muted_foreground })
                    // The mark's slot is there whether or not it is drawn: choosing never moves a word.
                    .child(div().size(space(4.)).flex_none().when(on, |slot| {
                        slot.child(icon(IconName::Check).small().text_color(t.primary))
                    }))
                    .child(div().whitespace_nowrap().child(name))
                    .on_drag_move::<ExternalPaths>(cx.listener(
                        move |this, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                            if event.bounds.contains(&event.event.position) {
                                this.pick(i, cx);
                            }
                        },
                    ))
            })
            .collect();

        let result = isolate(output_name(&active.first, chosen));
        let more = active.count.saturating_sub(1);
        let footer = col()
            .gap(space(0.5))
            .pt(space(1.5))
            .px(space(2.5))
            .border_t_1()
            .border_color(t.stroke)
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(div().max_w(space(44.)).truncate().child(result))
            .when(more > 0, |footer| {
                footer.child(div().child(t!("and {count} more", count = more)))
            });

        let header = row_start()
            .gap(space(2.))
            .px(space(2.5))
            .pb(space(1.))
            .items_center()
            .justify_between()
            .text_size(theme::size::XS)
            .text_color(t.muted_foreground)
            .child(div().child(t!("Drop as")))
            // The badge: the key that cycles, and where the choice is in the cycle.
            .child(
                row_start()
                    .id("transform-badge")
                    .role(Role::Group)
                    .aria_label(t!("Tab changes it"))
                    .test_support()
                    .gap(space(1.5))
                    .items_center()
                    .children(Keystroke::parse("tab").ok().map(Kbd::new))
                    .child(div().child(format!("{}/{}", current + 1, options.len()))),
            );

        let card = col()
            .id("transform-strip")
            .role(Role::Group)
            .aria_label(t!("Drop as"))
            .test_support()
            .py(space(2.))
            .gap(space(0.5))
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.stroke)
            .bg(t.popover)
            .shadow(theme::elevation::floating(cx))
            .text_size(theme::size::SM)
            .child(header)
            .child(col().px(space(1.)).gap(space(0.5)).children(chips))
            .child(footer);

        let card = if cx.reduce_motion() {
            card.into_any_element()
        } else {
            use gpui_kit::{Animation, AnimationExt as _};
            let rtl = crate::i18n::is_rtl();
            let arriving = Animation::new(theme::motion::FAST).with_easing(theme::motion::enter);
            card.relative()
                .with_animation(
                    ("transform-strip-in", self.arrivals as usize),
                    arriving,
                    move |card, delta| {
                        // From the edge it sits on, ten pixels at most, readable from the first frame.
                        let away = px(10. * (1. - delta));
                        let card = card.opacity(0.4 + 0.6 * delta);
                        if rtl {
                            card.right(away)
                        } else {
                            card.left(away)
                        }
                    },
                )
                .into_any_element()
        };

        // Docked to the end edge, in the middle of its height, over the sheet but clear of the dock and the bar.
        let dock = div()
            .absolute()
            .top(space(12.))
            .bottom(space(16.))
            .flex()
            .items_center()
            .px(space(3.));
        let dock = if crate::i18n::is_rtl() {
            dock.left_0()
        } else {
            dock.right_0()
        };
        dock.child(card).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{TestAppContext, VisualTestContext};

    fn open(cx: &mut TestAppContext) -> (VisualTestContext, Entity<TransformStrip>) {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(TransformStrip::new))
    }

    fn photos() -> Vec<String> {
        vec![r"C:\Trips\a.jpg".to_string(), r"C:\Trips\b.png".to_string()]
    }

    fn drag_of(cx: &mut VisualTestContext, paths: &[String]) -> Option<Offer> {
        cx.update(|_, cx| offer(cx, paths))
    }

    fn chips(cx: &mut VisualTestContext) -> Vec<(String, bool)> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            (0..6usize)
                .filter_map(|i| window.try_find(("transform-chip", i)))
                .map(|chip| {
                    (
                        chip.label().unwrap_or_default().to_string(),
                        chip.selected().unwrap_or(false),
                    )
                })
                .collect()
        })
    }

    #[gpui_kit::test]
    fn the_strip_follows_a_drag_that_offers_transforms(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, strip) = open(cx);
        // Nothing to look at until a drag offers something.
        assert!(chips(&mut visual).is_empty());
        let drag = drag_of(&mut visual, &photos()).expect("pictures offer transforms");
        visual.run_until_parked();
        assert!(strip.read_with(&visual, |s, _| s.shown()));
        assert_eq!(
            chips(&mut visual),
            [
                ("As they are".to_string(), true),
                ("Clean EXIF".to_string(), false),
                ("WebP".to_string(), false),
            ]
        );
        // Tab (the drop source's cycle) moves the choice, and the strip shows it.
        drag.switch.cycle(false);
        visual.run_until_parked();
        assert_eq!(strip.read_with(&visual, |s, _| s.selected()), Some(1));
        assert_eq!(
            chips(&mut visual).iter().map(|c| c.1).collect::<Vec<_>>(),
            [false, true, false]
        );
        // The name the drop will make, under the chips.
        visual.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("transform-strip").is_some());
        });
        drag.switch.cycle(false);
        drag.switch.cycle(false);
        visual.run_until_parked();
        assert_eq!(strip.read_with(&visual, |s, _| s.selected()), Some(0));
        // The drag ends: the strip goes.
        drop(drag);
        visual.run_until_parked();
        assert!(!strip.read_with(&visual, |s, _| s.shown()));
        assert!(chips(&mut visual).is_empty());
    }

    #[gpui_kit::test]
    fn the_pointer_over_a_chip_chooses_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, strip) = open(cx);
        let drag = drag_of(&mut visual, &[r"C:\v\clip.mp4".to_string()]).expect("a video offers transforms");
        visual.run_until_parked();
        assert_eq!(chips(&mut visual).len(), 3);
        // The strip's own pick (what a drag move over the chip does) sets the drag's switch too.
        strip.update_in(&mut visual, |strip, _, cx| strip.pick(2, cx));
        assert_eq!(drag.switch.selected(), 2);
        assert_eq!(
            chips(&mut visual),
            [
                ("As they are".to_string(), false),
                ("Under 10 MB".to_string(), false),
                ("Extract MP3".to_string(), true),
            ]
        );
    }

    #[gpui_kit::test]
    fn drags_with_nothing_to_offer_leave_the_strip_down(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (mut visual, strip) = open(cx);
        // Mixed, documents and remote files: the plain drag.
        for paths in [
            vec![r"C:\a\x.jpg".to_string(), r"C:\a\y.mp4".to_string()],
            vec![r"C:\a\notes.txt".to_string()],
            vec!["remote:abc/pics/a.jpg".to_string()],
        ] {
            assert!(drag_of(&mut visual, &paths).is_none(), "{paths:?}");
        }
        visual.run_until_parked();
        assert!(!strip.read_with(&visual, |s, _| s.shown()));
    }

    #[gpui_kit::test]
    fn the_strip_works_mirrored_with_arabic_names(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        testing::set_rtl(true);
        let (mut visual, strip) = open(cx);
        let drag = drag_of(&mut visual, &[r"C:\صور\رحلة.jpg".to_string()]).expect("a picture offers transforms");
        visual.run_until_parked();
        assert!(strip.read_with(&visual, |s, _| s.shown()));
        assert_eq!(chips(&mut visual).len(), 3);
        drag.switch.select(1);
        visual.run_until_parked();
        // The new name is a whole Arabic name.
        assert_eq!(output_name(r"C:\صور\رحلة.jpg", drag.switch.current()), "رحلة_clean.jpg");
        drop(drag);
        testing::set_rtl(false);
    }
}
