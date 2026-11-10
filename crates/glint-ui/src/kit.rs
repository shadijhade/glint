//! The kit: the one place views get their controls and layout helpers from.
//!
//! **Views import `glint_ui::kit::prelude::*` and nothing from gpui-component.** The kit re-exports the
//! gpui-component pieces Glint uses (Button, Input, Dialog, menus, tables…) under Glint's names, and adds
//! Glint's own helpers below. When gpui-component's API changes (it does), only this file changes.
//! If a view needs a control the kit lacks, add it here, don't reach around it.
//!
//! The prelude is GPUI itself (`div`, `px`, `Context`, `Render`, `SharedString`…), the kit's controls and
//! the helpers, so one `use` line is all a view needs:
//!
//! ```ignore
//! use crate::kit::prelude::*;
//! use crate::{t, theme};
//! ```
//!
//! **Layout without left and right.** The Arabic interface is mirrored, so a view never says "left" or
//! "right". It says [`row_start`] (a row that reverses in right to left), [`Logical::align_start`],
//! [`Logical::ps`] (padding at the start edge) and so on. Text that GPUI lays out is handled by the
//! RTL-fixed GPUI itself; [`isolate`] wraps a string whose first letter is right to left so DirectWrite
//! orders it as its own right-to-left paragraph (file names mixed into English sentences).
//!
//! **Spacing and icons.** [`space`]`(n)` is `n` × 0.25 rem, Tailwind's unit (`space(3.)` is `gap-3`).
//! [`icon`] is a Lucide icon at the text size; icons that point (back, forward) mirror with [`back_icon`]
//! and [`forward_icon`].

use crate::{i18n, theme};
use gpui_kit::component::{h_flex, v_flex};

/// Everything a view imports.
pub mod prelude {
    pub use super::{
        align_row, back_icon, capacity_bar, col, drive_icon, forward_icon, free_of, icon, isolate, kind_icon,
        known_icon, row, row_start, space, tooltip, Logical, Placeholder,
    };
    pub use super::{h_resizable, resizable_panel};
    pub use super::{
        Button, ButtonCustomVariant, ButtonVariants, Checkbox, Column, ContextMenuExt, DataTable, Dialog, DialogFooter,
        Disableable, Icon, IconName, Input, InputEvent, InputState, Kbd, Notification, PopupMenu, PopupMenuItem,
        ResizableState, ScrollableElement, Scrollbar, Selectable, Sizable, Spinner, Switch, TableDelegate, TableState,
        TitleBar, WindowExt,
    };
    pub use gpui_kit::prelude::*;
    pub use gpui_kit::*;
    // With gpui-kit's test-support, `gpui_kit::*` brings GPUI's own `test` macro, which would shadow Rust's
    // `#[test]` in every module that imports this prelude (and make `#[gpui_kit::test]` expand into itself
    // until the compiler gives up). An explicit re-export wins over the glob and keeps `#[test]` Rust's.
    pub use core::prelude::v1::test;
}

pub use gpui_kit::assets::IconName;

/// A one-line tooltip for any element with an id: `.tooltip(move |window, cx| kit::tooltip(text.clone(), window, cx))`.
/// Buttons take `.tooltip(text)` directly.
pub fn tooltip(text: SharedString, window: &mut Window, cx: &mut App) -> gpui_kit::AnyView {
    gpui_kit::component::tooltip::Tooltip::new(text).build(window, cx)
}

/// A known folder's icon, as the sidebar, This PC and the address bar draw it (`KNOWN_ICONS`).
pub fn known_icon(id: crate::explorer::KnownId) -> IconName {
    use crate::explorer::KnownId as K;
    match id {
        K::Home => IconName::House,
        K::Desktop => IconName::Monitor,
        K::Documents => IconName::FileText,
        K::Downloads => IconName::Download,
        K::Pictures => IconName::Image,
        K::Music => IconName::Music,
        K::Videos => IconName::Film,
    }
}

/// A drive's icon by its kind, as the sidebar, This PC and the address bar draw it (`DRIVE_ICONS`).
pub fn drive_icon(kind: crate::explorer::DriveKind) -> IconName {
    use crate::explorer::DriveKind as K;
    match kind {
        K::Fixed | K::Ram | K::Unknown => IconName::HardDrive,
        K::Removable => IconName::Usb,
        K::Network => IconName::Server,
        K::Cdrom => IconName::Disc,
        K::Wsl => IconName::SquareTerminal,
    }
}

/// The used-space bar of a drive (or a cloud), red when nearly full like Explorer's (`CapacityBar`): `height`
/// tall, filling from the reading direction's start. None when the total isn't known.
pub fn capacity_bar(total: u64, free: u64, height: Rems, cx: &App) -> Option<Div> {
    if total == 0 {
        return None;
    }
    let t = theme::tokens(cx);
    let used = (total.saturating_sub(free) as f64 / total as f64).clamp(0., 1.);
    let fill = if used > 0.9 {
        t.destructive
    } else {
        t.primary.opacity(0.7)
    };
    Some(
        row_start()
            .h(height)
            .w_full()
            .overflow_hidden()
            .rounded_full()
            .bg(t.muted)
            .child(
                div()
                    .h_full()
                    .w(gpui_kit::relative(used as f32))
                    .rounded_full()
                    .bg(fill),
            ),
    )
}

/// "12 GB free of 476 GB" (`freeOf`), or nothing when the total isn't known.
pub fn free_of(total: u64, free: u64) -> SharedString {
    if total == 0 {
        return SharedString::default();
    }
    crate::t!(
        "{free} free of {total}",
        free = glint_model::format::format_size(free),
        total = glint_model::format::format_size(total)
    )
}

/// The icon a kind of file gets (`glint_model::file_types::kind_of(entry).icon`), as the web app drew it. Its
/// colors come from `theme::kind_tone`.
pub fn kind_icon(icon: glint_model::file_types::Icon) -> IconName {
    use glint_model::file_types::Icon as K;
    match icon {
        K::AppWindow => IconName::AppWindow,
        K::BookOpen => IconName::BookOpen,
        K::Database => IconName::Database,
        K::Disc => IconName::Disc,
        K::File => IconName::File,
        K::FileArchive => IconName::FileArchive,
        K::FileAudio => IconName::FileHeadphone, // lucide renamed file-audio
        K::FileCode => IconName::FileCode,
        K::FileCog => IconName::FileCog,
        K::FileImage => IconName::FileImage,
        K::FileJson => IconName::FileBraces, // lucide renamed file-json
        K::FileLock => IconName::FileLock,
        K::FileSpreadsheet => IconName::FileSpreadsheet,
        K::FileText => IconName::FileText,
        K::FileType => IconName::FileType,
        K::FileVideo => IconName::FilePlay, // lucide renamed file-video
        K::Folder => IconName::Folder,
        K::Link => IconName::Link,
        K::Presentation => IconName::Presentation,
        K::SquareTerminal => IconName::SquareTerminal,
    }
}
/// A scrollable element's scrollbar (`.overflow_y_scrollbar()` and friends) and the base Scrollbar.
pub use gpui_kit::base::Scrollbar;
pub use gpui_kit::component::button::{Button, ButtonCustomVariant, ButtonVariants};
pub use gpui_kit::component::checkbox::Checkbox;
pub use gpui_kit::component::dialog::{Dialog, DialogFooter};
/// A read-only (or editable) code view with syntax highlighting (syntect's grammars), virtualized: the text
/// and code previews. And a multi-line text box.
pub use gpui_kit::component::input::{Editor, EditorState, Textarea, TextareaState};
pub use gpui_kit::component::input::{Input, InputEvent, InputState};
pub use gpui_kit::component::kbd::Kbd;
pub use gpui_kit::component::menu::{ContextMenuExt, PopupMenu, PopupMenuItem};
pub use gpui_kit::component::notification::Notification;
pub use gpui_kit::component::scroll::ScrollableElement;
pub use gpui_kit::component::spinner::Spinner;
pub use gpui_kit::component::switch::Switch;
pub use gpui_kit::component::table::{Column, DataTable, TableDelegate, TableState};
/// Rendered Markdown (CommonMark and GitHub's extensions) and plain rich text: the Markdown preview, notes.
pub use gpui_kit::component::text::{TextView, TextViewState};
pub use gpui_kit::component::{h_resizable, resizable_panel, ResizableState};
pub use gpui_kit::component::{Disableable, Icon, Selectable, Sizable, TitleBar, WindowExt};

use gpui_kit::prelude::*;
use gpui_kit::{div, rems, App, Div, IntoElement, Rems, Role, SharedString, Window};

/// A horizontal flex row, children centred vertically (`flex items-center`).
pub fn row() -> Div {
    h_flex()
}

/// A vertical flex column (`flex flex-col`).
pub fn col() -> Div {
    v_flex()
}

/// A row that starts at the reading direction's start edge: left to right in English, right to left in
/// Arabic. Use it wherever a row's order carries meaning (an icon then its label, back/forward/up, a
/// toolbar). Children are written in logical order; the row reverses itself.
pub fn row_start() -> Div {
    let row = h_flex();
    if i18n::is_rtl() {
        row.flex_row_reverse()
    } else {
        row
    }
}

/// A row with its children at the start edge, spread as `gap` apart, in logical order.
pub fn align_row(gap: f32) -> Div {
    row_start().gap(space(gap))
}

/// `n` × 0.25 rem: Tailwind's spacing unit, so `space(3.)` is `gap-3`: 12 px at the usual 16 px rem, and it
/// scales with the text-size setting as the web UI's spacing did.
pub fn space(n: f32) -> Rems {
    rems(n * 0.25)
}

/// A Lucide icon at the surrounding text size, in the surrounding text color.
pub fn icon(name: IconName) -> Icon {
    Icon::from(name)
}

/// The "back" arrow: points left in English, right in Arabic.
pub fn back_icon() -> IconName {
    if i18n::is_rtl() {
        IconName::ArrowRight
    } else {
        IconName::ArrowLeft
    }
}

/// The "forward" arrow: points right in English, left in Arabic.
pub fn forward_icon() -> IconName {
    if i18n::is_rtl() {
        IconName::ArrowLeft
    } else {
        IconName::ArrowRight
    }
}

/// Layout in the reading direction's terms. Implemented for every styled element, so
/// `col().align_start().ps(3.)` reads the same in both directions.
pub trait Logical: Styled + Sized {
    /// Children of a column sit at the start edge (left in English, right in Arabic).
    fn align_start(self) -> Self {
        if i18n::is_rtl() {
            self.items_end()
        } else {
            self.items_start()
        }
    }

    /// Children of a column sit at the end edge.
    fn align_end(self) -> Self {
        if i18n::is_rtl() {
            self.items_start()
        } else {
            self.items_end()
        }
    }

    /// Text starts at the start edge.
    fn text_start(self) -> Self {
        if i18n::is_rtl() {
            self.text_right()
        } else {
            self.text_left()
        }
    }

    /// Text ends at the end edge (numbers in a column, for one).
    fn text_end(self) -> Self {
        if i18n::is_rtl() {
            self.text_left()
        } else {
            self.text_right()
        }
    }

    /// Padding at the start edge, in 4 px units (Tailwind's `ps-*`).
    fn ps(self, n: f32) -> Self {
        if i18n::is_rtl() {
            self.pr(space(n))
        } else {
            self.pl(space(n))
        }
    }

    /// Padding at the end edge (`pe-*`).
    fn pe(self, n: f32) -> Self {
        if i18n::is_rtl() {
            self.pl(space(n))
        } else {
            self.pr(space(n))
        }
    }

    /// Margin at the start edge (`ms-*`).
    fn ms(self, n: f32) -> Self {
        if i18n::is_rtl() {
            self.mr(space(n))
        } else {
            self.ml(space(n))
        }
    }

    /// Margin at the end edge (`me-*`).
    fn me(self, n: f32) -> Self {
        if i18n::is_rtl() {
            self.ml(space(n))
        } else {
            self.mr(space(n))
        }
    }
}

impl<T: Styled + Sized> Logical for T {}

/// Whether `ch` is a letter of a right-to-left script (Hebrew, Arabic, Syriac, Thaana, NKo and their
/// presentation forms).
fn is_rtl_letter(ch: char) -> bool {
    matches!(ch as u32,
        0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF | 0x10800..=0x10FFF | 0x1E800..=0x1EFFF)
        && ch.is_alphabetic()
}

/// Right-to-left isolate: RLI … PDI around `text` when its first letter is right to left, so DirectWrite
/// orders it as its own right-to-left paragraph whatever surrounds it (an Arabic file name in an English
/// row). Text that starts with a left-to-right letter, or has none, comes back unchanged. Visual text only:
/// never put the wrapped string in a path, an input's value or an accessibility label.
pub fn isolate(text: impl Into<SharedString>) -> SharedString {
    let text = text.into();
    let first_strong = text.chars().find(|ch| ch.is_alphabetic());
    if first_strong.is_some_and(is_rtl_letter) {
        format!("\u{2067}{text}\u{2069}").into()
    } else {
        text
    }
}

/// A stand-in for a component a later wave builds: says what it is and what replaces it. Placeholders use
/// `t!`, theme tokens and the kit like everything else.
#[derive(IntoElement)]
pub struct Placeholder {
    name: SharedString,
    replaced_by: SharedString,
}

impl Placeholder {
    /// `name` is what the area is ("File list"); `replaced_by` says what takes its place and when.
    pub fn new(name: impl Into<SharedString>, replaced_by: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            replaced_by: replaced_by.into(),
        }
    }
}

impl RenderOnce for Placeholder {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let t = theme::tokens(cx);
        col()
            .id(SharedString::from(format!("placeholder-{}", self.name)))
            .role(Role::Group)
            .aria_label(self.name.clone())
            .size_full()
            .items_center()
            .justify_center()
            .gap(space(1.))
            .p(space(4.))
            .text_color(t.muted_foreground)
            .child(div().text_size(theme::size::SM).child(self.name))
            .child(div().text_size(theme::size::XS).child(self.replaced_by))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_is_a_quarter_rem_a_unit() {
        assert_eq!(space(0.), rems(0.));
        assert_eq!(space(3.), rems(0.75));
        assert_eq!(space(0.5), rems(0.125));
    }

    #[test]
    fn every_icon_the_skeleton_draws_is_in_the_embedded_assets() {
        use gpui_kit::assets::AllAssets;
        use gpui_kit::AssetSource as _;
        // A missing icon draws nothing and says nothing, so each one the views use is checked here: add yours.
        for name in [
            IconName::House,
            IconName::Monitor,
            IconName::Download,
            IconName::Image,
            IconName::Music,
            IconName::Film,
            IconName::Usb,
            IconName::Server,
            IconName::AppWindow,
            IconName::BookOpen,
            IconName::Database,
            IconName::Disc,
            IconName::File,
            IconName::FileArchive,
            IconName::FileHeadphone,
            IconName::FileCode,
            IconName::FileCog,
            IconName::FileImage,
            IconName::FileBraces,
            IconName::FileLock,
            IconName::FileSpreadsheet,
            IconName::FileText,
            IconName::FileType,
            IconName::FilePlay,
            IconName::Folder,
            IconName::Link,
            IconName::Presentation,
            IconName::SquareTerminal,
            IconName::ArrowLeft,
            IconName::ArrowRight,
            IconName::ArrowUp,
            IconName::HardDrive,
            IconName::HeartPulse,
            IconName::Plus,
            IconName::Search,
            IconName::ShieldAlert,
            IconName::Thermometer,
            IconName::TriangleAlert,
            IconName::FolderOpen,
            IconName::TextSearch,
            IconName::LayoutDashboard,
            IconName::Share2,
        ] {
            let path = name.path();
            assert!(matches!(AllAssets.load(&path), Ok(Some(_))), "{path} is not embedded");
        }
    }

    #[test]
    fn isolate_wraps_only_text_that_starts_right_to_left() {
        // Arabic and Hebrew names get RLI … PDI.
        assert_eq!(isolate("تقرير الربع 1.pdf"), "\u{2067}تقرير الربع 1.pdf\u{2069}");
        assert_eq!(isolate("קובץ.txt"), "\u{2067}קובץ.txt\u{2069}");
        // A digit or symbol first doesn't decide: the first letter does.
        assert_eq!(isolate("1 - صلاة الفجر.mp3"), "\u{2067}1 - صلاة الفجر.mp3\u{2069}");
        // English, empty, and letterless text are left alone, even with Arabic later.
        assert_eq!(isolate("Quarterly report"), "Quarterly report");
        assert_eq!(isolate("Report تقرير"), "Report تقرير");
        assert_eq!(isolate(""), "");
        assert_eq!(isolate("12345"), "12345");
    }

    #[test]
    fn back_and_forward_arrows_mirror_in_arabic() {
        // Direction is process-wide state; i18n's own test flips it, so only the English side is asserted
        // here without holding the test lock the view tests use.
        let _guard = crate::testing::lock();
        crate::testing::set_rtl(false);
        assert!(matches!(back_icon(), IconName::ArrowLeft) && matches!(forward_icon(), IconName::ArrowRight));
        crate::testing::set_rtl(true);
        assert!(matches!(back_icon(), IconName::ArrowRight) && matches!(forward_icon(), IconName::ArrowLeft));
        crate::testing::set_rtl(false);
    }
}
