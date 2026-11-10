//! Files dragged over a Glint window from File Explorer or another app, handed to GPUI as its own file drops,
//! so a view takes them with GPUI's element listeners. Glint's drop target (`platform::dragdrop`) decides the
//! effect (copy by default, Shift moves, Alt links) and shows Windows' cursor and drag picture; this module
//! turns its events into GPUI's, and carries the view's answer back.
//!
//! **In a view** (the payload is GPUI's `ExternalPaths`):
//! - `.drag_over::<ExternalPaths>(|style, _, _, _| style.bg(..))` highlights the place under the pointer.
//! - `.on_drag_move::<ExternalPaths>(cx.listener(|this, event: &DragMoveEvent<ExternalPaths>, _, cx| ..))` fires on
//!   every move for every listener (GPUI's capture phase): when `event.bounds.contains(&event.event.position)`,
//!   answer what a drop there would do with [`answer`] (`Answer::Accept(Under { name, copy, moves, always_move })`
//!   or `Answer::Refuse`). Nothing answering means no drop there: each move starts refused.
//! - `.on_drop::<ExternalPaths>(cx.listener(|this, drop: &ExternalPaths, _, cx| ..))` takes the drop; [`effect`]
//!   says what it does (copy, move or link), [`paths`] gives the files as strings. An effect of `None` never
//!   reaches a view: a refused drop isn't handed over.

use crate::platform::dragdrop::{Answer, DropAnswers, DropEffect, DropEvent};
use gpui_kit::{point, px, App, ExternalPaths, FileDropEvent, Global, PlatformInput, Window};

/// The drag in progress over a window: what a drop would do, and where the views' answers go.
struct Drops {
    answers: DropAnswers,
    effect: DropEffect,
}

impl Global for Drops {}

/// What the drop being made does: copy, move or link (from the keys held and what the source allows).
pub fn effect(cx: &App) -> DropEffect {
    cx.try_global::<Drops>().map_or(DropEffect::None, |drops| drops.effect)
}

/// What a drop at the pointer would do, from the view under it (call it from `on_drag_move` when the pointer is
/// inside your element). The next answer to Windows uses it: the cursor and the drag picture's words follow.
pub fn answer(cx: &App, answer: Answer) {
    if let Some(drops) = cx.try_global::<Drops>() {
        drops.answers.set(answer);
    }
}

/// The dropped files as strings, as the explorer takes them.
pub fn paths(drop: &ExternalPaths) -> Vec<String> {
    drop.paths().iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

/// Hands one of the drop target's events to GPUI. Call it with the window, outside any entity update (GPUI
/// runs the views' listeners from here).
pub(crate) fn dispatch(event: DropEvent, answers: &DropAnswers, window: &mut Window, cx: &mut App) {
    let at = |x: f32, y: f32| point(px(x), px(y));
    let input = match event {
        DropEvent::Enter { paths, x, y, effect } => {
            cx.set_global(Drops {
                answers: answers.clone(),
                effect,
            });
            answers.set(Answer::Refuse);
            let paths = ExternalPaths(paths.into_iter().map(std::path::PathBuf::from).collect());
            FileDropEvent::Entered {
                position: at(x, y),
                paths,
            }
        }
        DropEvent::Over { x, y, effect } => {
            if cx.has_global::<Drops>() {
                cx.global_mut::<Drops>().effect = effect;
            }
            // Only the view under the pointer answers; anywhere else takes nothing.
            answers.set(Answer::Refuse);
            FileDropEvent::Pending { position: at(x, y) }
        }
        DropEvent::Leave => FileDropEvent::Exited,
        DropEvent::Drop { x, y, effect, .. } => {
            if effect == DropEffect::None {
                // Refused where it landed: nothing to hand over.
                FileDropEvent::Exited
            } else {
                if cx.has_global::<Drops>() {
                    cx.global_mut::<Drops>().effect = effect;
                }
                window.dispatch_event(
                    PlatformInput::FileDrop(FileDropEvent::Submit { position: at(x, y) }),
                    cx,
                );
                FileDropEvent::Exited
            }
        }
    };
    window.dispatch_event(PlatformInput::FileDrop(input), cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kit::prelude::*;
    use crate::platform::dragdrop::Under;
    use crate::testing;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{DragMoveEvent, TestAppContext};

    /// A view that takes drops over its whole area and answers for them, as the file list will.
    struct Target {
        dropped: Vec<(Vec<String>, DropEffect)>,
    }

    impl Render for Target {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("target")
                .size_full()
                .on_drag_move::<ExternalPaths>(cx.listener(|_, event: &DragMoveEvent<ExternalPaths>, _, cx| {
                    if event.bounds.contains(&event.event.position) {
                        answer(
                            cx,
                            Answer::Accept(Under {
                                name: "here".into(),
                                copy: "Copy to %1".into(),
                                moves: "Move to %1".into(),
                                always_move: false,
                            }),
                        );
                    }
                }))
                .on_drop::<ExternalPaths>(cx.listener(|this, drop: &ExternalPaths, _, cx| {
                    this.dropped.push((paths(drop), effect(cx)));
                }))
        }
    }

    #[gpui_kit::test]
    fn a_drop_reaches_the_view_under_the_pointer_with_its_effect(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        cx.update(testing::init);
        let (mut visual, view) = testing::open_window(cx, |_, cx| cx.new(|_| Target { dropped: Vec::new() }));
        let answers = DropAnswers::default();
        let files = vec![r"C:\Work.txt".to_string(), r"C:\Work\ملف.txt".to_string()];
        visual.update(|window, cx| {
            window.render_frame(cx);
            dispatch(
                DropEvent::Enter {
                    paths: files.clone(),
                    x: 50.,
                    y: 50.,
                    effect: DropEffect::Copy,
                },
                &answers,
                window,
                cx,
            );
            dispatch(
                DropEvent::Over {
                    x: 60.,
                    y: 60.,
                    effect: DropEffect::Move,
                },
                &answers,
                window,
                cx,
            );
        });
        assert!(
            matches!(answers.get(), Answer::Accept(_)),
            "the view under the pointer answered"
        );
        visual.update(|window, cx| {
            dispatch(
                DropEvent::Drop {
                    paths: files.clone(),
                    x: 60.,
                    y: 60.,
                    effect: DropEffect::Move,
                },
                &answers,
                window,
                cx,
            );
        });
        visual.run_until_parked();
        let dropped = visual.update(|_, cx| view.read(cx).dropped.clone());
        assert_eq!(dropped, vec![(files, DropEffect::Move)]);
    }
}
