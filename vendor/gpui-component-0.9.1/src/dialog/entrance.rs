use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Window, point, px,
};
use serde::{Deserialize, Serialize};

/// The entrance presentation shared by Dialog and AlertDialog.
///
/// Closing animates the surface and backdrop back out. Reduced motion displays
/// the final state for every option, and [`DialogEntrance::None`] opens and
/// closes in a single frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DialogEntrance {
    /// Slide down from the window top while fading in, preserving the legacy entrance.
    SlideDown,
    /// Fade in at the final position without movement.
    Fade,
    /// Fade in while settling from just above the final position.
    ///
    /// Travel comes from the theme's short motion distance and is limited by
    /// the space above the resolved surface inside the window.
    ///
    /// Glint: the default. The legacy slide travelled from the top of the window
    /// for a quarter of a second, which made every dialog feel late; this one is
    /// in place from the start and settles within the theme's normal duration.
    #[default]
    FadeSlide,
    /// Show the surface and backdrop immediately without entrance animation.
    None,
}

/// Glint: how small a surface is drawn at the start of its entrance.
const ENTRANCE_POP: f32 = 0.955;

/// Applies presentation travel after Positioner has resolved the resting bounds.
/// Paint, hit testing, and accessibility follow the same offset.
pub(super) struct EntranceSurface {
    child: AnyElement,
    travel: Pixels,
    top_limit: Pixels,
    pub(super) progress: f32,
    /// Glint: what the dialog opens out of (window pixels), if anything: see `Dialog::opens_from`.
    pub(super) from: Option<Bounds<Pixels>>,
}

impl EntranceSurface {
    pub(super) fn new(child: AnyElement, travel: Pixels, top_limit: Pixels) -> Self {
        Self {
            child,
            travel,
            top_limit,
            progress: 0.,
            from: None,
        }
    }
}

impl IntoElement for EntranceSurface {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EntranceSurface {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let available = (bounds.top() - self.top_limit).max(px(0.));
        let travel = self.travel.max(px(0.)).min(available);
        let offset = point(px(0.), -travel * (1. - self.progress.clamp(0., 1.)));
        window.with_element_offset(offset, |window| self.child.prepaint(window, cx));
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        // Glint: the surface pops: it is drawn a few percent small at first and grows about its
        // middle as it arrives (and back as it leaves). Paint only: its layout and what the
        // pointer hits are where it rests. None under reduced motion, where progress is whole.
        let progress = self.progress.clamp(0., 1.);
        let available = (bounds.top() - self.top_limit).max(px(0.));
        let travel = self.travel.max(px(0.)).min(available);
        let middle = bounds.center() + point(px(0.), -travel * (1. - progress));
        let pop = (progress < 1.).then(|| match self.from {
            // Out of the thing it shows: as small as that thing at first and on it, growing by equal
            // ratios to where it rests (and back into the thing as it leaves).
            Some(from) if bounds.size.width > px(0.) && bounds.size.height > px(0.) => {
                let small = (from.size.width / bounds.size.width)
                    .max(from.size.height / bounds.size.height)
                    .clamp(0.04, 1.);
                let scale = small.powf(1. - progress);
                let along = if small < 0.999 {
                    (scale - small) / (1. - small)
                } else {
                    progress
                };
                let at = from.center() + (middle - from.center()) * along;
                gpui::PaintTransform {
                    scale,
                    translate: point(at.x - middle.x * scale, at.y - middle.y * scale),
                }
            }
            _ => gpui::PaintTransform::scale_about(
                ENTRANCE_POP + (1. - ENTRANCE_POP) * progress,
                middle,
            ),
        });
        window.with_paint_transform(pop, |window| self.child.paint(window, cx));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Anchor, Context, InteractiveElement as _, ParentElement as _, Render,
        StatefulInteractiveElement as _, Styled as _, TestAppContext, div, size,
    };

    struct SurfaceProbe {
        progress: f32,
        requested_y: Pixels,
        clicks: usize,
    }

    impl Render for SurfaceProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut surface = EntranceSurface::new(
                div()
                    .id("surface")
                    .w(px(200.))
                    .h(px(300.))
                    .debug_selector(|| "entrance-surface".into())
                    .on_click(cx.listener(|probe, _, _, _| probe.clicks += 1))
                    .into_any_element(),
                px(8.),
                px(16.),
            );
            surface.progress = self.progress;
            div().size_full().child(
                gpui_base::Positioner::corner(Anchor::TopLeft, point(px(100.), self.requested_y))
                    .margin(px(16.))
                    .child(surface),
            )
        }
    }

    #[gpui::test]
    fn fade_slide_uses_resolved_bounds_and_keeps_the_surface_inside_the_window(
        cx: &mut TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, _| SurfaceProbe {
            progress: 0.,
            requested_y: px(200.),
            clicks: 0,
        });
        cx.simulate_resize(size(px(400.), px(400.)));
        // Requested y=200 resolves to y=84 because the surface is 300px tall.
        for (requested_y, positions) in [
            (px(200.), [px(76.), px(80.), px(84.)]),
            (px(20.), [px(16.), px(18.), px(20.)]),
            (px(0.), [px(16.), px(16.), px(16.)]),
        ] {
            for (progress, expected_y) in [0., 0.5, 1.].into_iter().zip(positions) {
                cx.update(|window, cx| {
                    view.update(cx, |probe, cx| {
                        probe.progress = progress;
                        probe.requested_y = requested_y;
                        cx.notify();
                    });
                    window.draw(cx).clear(cx);
                });
                let bounds = cx.debug_bounds("entrance-surface").unwrap();
                assert_eq!(bounds.top(), expected_y);
                assert!(bounds.top() >= px(16.) && bounds.bottom() <= px(384.));
                // At the beginning of the clamped entrance this point is
                // above the resting surface, so the hitbox must travel too.
                cx.simulate_click(bounds.origin + point(px(10.), px(2.)), Default::default());
            }
        }
        cx.update(|_, cx| assert_eq!(view.read(cx).clicks, 9));
    }
}
