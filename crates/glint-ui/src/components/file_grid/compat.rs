//! Two controls the kit doesn't export yet, reached through gpui-kit's own `component` module until it does.
//!
//! TODO(coordinator): from kit (`kit::tooltip`, `kit::ScrollableElement`). Both are used in this folder only
//! through this file, so the swap is one place.

use crate::kit::prelude::*;

pub use gpui_kit::component::scroll::ScrollableElement;

/// A tooltip with `text` (`title` in the TypeScript's tiles).
pub fn tooltip(text: SharedString, window: &mut Window, cx: &mut App) -> AnyView {
    gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
}
