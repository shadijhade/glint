//! The search bar and the cards under it (Search.tsx).
//!
//! | TypeScript                                | Here                                                           |
//! |-------------------------------------------|----------------------------------------------------------------|
//! | `TOGGLES`, `MATCHING`, `Toggle`           | [`TOGGLES`], [`is_disabled`]: seven switches, each with its Alt+letter |
//! | `SearchBar`                               | `TextSearch::bar`: the query, the switches, Replace, the folder, Search and Stop |
//! | the query's `placeholder`                 | [`query_placeholder`]                                           |
//! | the replacement's `placeholder`           | [`replacement_placeholder`]                                     |
//! | `RipgrepCard`, `AstGrepCard`              | `settings::tools::needed`, the card Settings' model draws        |
//! | `MeaningCard`                             | `TextSearch::meaning_card`                                       |
//!
//! Not ported: the folder box's `<datalist>` of recent folders (the browser drew it; the recent searches
//! list offers the same folders).

use super::TextSearch;
use crate::components::settings::tools;
use crate::kit::prelude::*;
use crate::kit::Textarea;
use crate::{core_bridge, i18n, t, theme};
use glint_model::store::{SearchOptionKey, SearchOptions};

/// One switch of the search: what it is, how it looks, and what a screen reader and the tooltip say.
pub struct Toggle {
    pub key: SearchOptionKey,
    pub icon: IconName,
    /// English text, the `t!` key; the Alt+letter that flips it is in it.
    pub label: &'static str,
}

/// The seven switches, in the TypeScript's order (`TOGGLES`).
pub const TOGGLES: [Toggle; 7] = [
    Toggle {
        key: SearchOptionKey::CaseSensitive,
        icon: IconName::CaseSensitive,
        label: "Match case (Alt+C)",
    },
    Toggle {
        key: SearchOptionKey::WholeWord,
        icon: IconName::WholeWord,
        label: "Whole word (Alt+W)",
    },
    Toggle {
        key: SearchOptionKey::Regex,
        icon: IconName::Regex,
        label: "Regular expression (Alt+R)",
    },
    Toggle {
        key: SearchOptionKey::Code,
        icon: IconName::Braces,
        label: "Code pattern, by its structure (Alt+A): console.log($MSG), $X.unwrap() (ast-grep)",
    },
    Toggle {
        key: SearchOptionKey::Documents,
        icon: IconName::FileText,
        label: "Documents (Alt+D): the text in PDF, Word, Excel, PowerPoint, OpenDocument and EPUB files",
    },
    Toggle {
        key: SearchOptionKey::Ocr,
        icon: IconName::ScanText,
        label: "Pictures too, with Documents (Alt+O): the text in them, read by Windows' OCR",
    },
    Toggle {
        key: SearchOptionKey::Meaning,
        icon: IconName::Sparkles,
        label: "Meaning (Alt+M): documents and notes ranked by what they're about, not by their words (a small model, downloaded once)",
    },
];

/// Whether a switch is on.
pub fn is_on(options: &SearchOptions, key: SearchOptionKey) -> bool {
    match key {
        SearchOptionKey::CaseSensitive => options.case_sensitive,
        SearchOptionKey::WholeWord => options.whole_word,
        SearchOptionKey::Regex => options.regex,
        SearchOptionKey::Code => options.code,
        SearchOptionKey::Documents => options.documents,
        SearchOptionKey::Ocr => options.ocr,
        SearchOptionKey::Meaning => options.meaning,
    }
}

/// Whether a switch can't be used now: a code pattern has no case, words or regular expression of its own, and
/// nor has meaning; a code search reads no pictures; pictures come with Documents.
pub fn is_disabled(options: &SearchOptions, key: SearchOptionKey) -> bool {
    let matching = matches!(
        key,
        SearchOptionKey::CaseSensitive | SearchOptionKey::WholeWord | SearchOptionKey::Regex
    );
    ((options.code || options.meaning) && matching)
        || (options.code && key == SearchOptionKey::Ocr)
        || (key == SearchOptionKey::Ocr && !options.documents)
}

/// What the empty query box says, for the kind of search that's on.
pub fn query_placeholder(options: &SearchOptions) -> SharedString {
    if options.code {
        t!("A code pattern…  console.log($MSG), $X.unwrap()  ($$$ for any number)")
    } else if options.meaning {
        t!("What you're looking for, in your own words…  (documents and notes ranked by meaning)")
    } else if options.documents {
        t!("Search inside documents…  PDF, Word, Excel, PowerPoint, OpenDocument, EPUB")
    } else {
        t!("Search for text…  (Shift+Enter adds a line)")
    }
}

/// What the empty Replacement box says.
pub fn replacement_placeholder(options: &SearchOptions) -> SharedString {
    if options.documents {
        t!("Documents aren't changed here: replace works in text files")
    } else if options.code {
        t!("Rewrite to…  logger.info($MSG)  (Enter shows what each match becomes)")
    } else if options.regex {
        t!("Replace with…  ($1 inserts a capture group)")
    } else {
        t!("Replace with…")
    }
}

impl TextSearch {
    /// The bar: the field (query, switches, Replace, folder) and Search or Stop beside it.
    pub(super) fn bar(&mut self, t: &theme::Tokens, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let options = Self::options(cx);
        let searching = self.search.read(cx).searching();
        let can_replace = self.can_replace(cx);
        let query_empty = self.query.read(cx).value().is_empty();
        let in_field = self.in_a_field(window, cx);

        let toggles = TOGGLES.iter().enumerate().map(|(i, toggle)| {
            let on = is_on(&options, toggle.key);
            let key = toggle.key;
            let label = i18n::translate(toggle.label);
            let button = Button::new(("text-toggle", i)).small().icon(toggle.icon);
            let button = if on {
                button.custom(
                    ButtonCustomVariant::new(cx)
                        .color(t.primary.opacity(0.15))
                        .foreground(t.primary)
                        .hover(t.primary.opacity(0.2))
                        .active(t.primary.opacity(0.25)),
                )
            } else {
                button.ghost().text_color(t.muted_foreground)
            };
            button
                .toggled(on)
                .disabled(is_disabled(&options, key))
                .tooltip(label.clone())
                .accessibility_label(label)
                .on_click(cx.listener(move |this, _, window, cx| this.toggle_option(key, window, cx)))
        });
        let replace_toggle = {
            let on = self.replace_open;
            let button = Button::new("text-replace-toggle").small().icon(IconName::Replace);
            let button = if on {
                button.custom(
                    ButtonCustomVariant::new(cx)
                        .color(t.primary.opacity(0.15))
                        .foreground(t.primary)
                        .hover(t.primary.opacity(0.2))
                        .active(t.primary.opacity(0.25)),
                )
            } else {
                button.ghost().text_color(t.muted_foreground)
            };
            button
                .toggled(on)
                .tooltip(t!("Replace (Ctrl+H)"))
                .accessibility_label(t!("Toggle replace"))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.replace_open = !this.replace_open;
                    cx.notify();
                }))
        };

        let query_row = row_start()
            .items_start()
            .pe(1.)
            .child(
                icon(IconName::Search)
                    .size_4()
                    .flex_none()
                    .text_color(t.muted_foreground)
                    .ms(3.)
                    .mt(space(2.5)),
            )
            .child(
                div()
                    .key_context("TextQuery")
                    .flex_1()
                    .min_w_0()
                    .font_family(theme::mono_font(cx))
                    .child(
                        Textarea::new(&self.query)
                            .appearance(false)
                            .bordered(false)
                            .aria_label(t!("Search query")),
                    ),
            )
            .child(
                row_start()
                    .flex_none()
                    .gap(space(0.5))
                    .pt(space(1.))
                    .children(toggles)
                    .child(div().mx(space(1.)).h(space(4.)).w(px(1.)).bg(t.border))
                    .child(replace_toggle),
            );

        let replace_row = self.replace_open.then(|| {
            row_start()
                .items_center()
                .pe(1.)
                .border_t_1()
                .border_color(t.border)
                .child(
                    icon(IconName::Replace)
                        .size_4()
                        .flex_none()
                        .text_color(t.muted_foreground)
                        .ms(3.),
                )
                .child(
                    div().flex_1().min_w_0().font_family(theme::mono_font(cx)).child(
                        Input::new(&self.replacement)
                            .appearance(false)
                            .bordered(false)
                            .aria_label(t!("Replacement")),
                    ),
                )
                .child(
                    Button::new("text-replace-all")
                        .ghost()
                        .small()
                        .label(t!("Replace all"))
                        .disabled(!can_replace || options.documents)
                        .tooltip(t!("Replace in every file of the current results"))
                        .on_click(cx.listener(|this, _, window, cx| this.replace_all(window, cx))),
                )
        });

        let folder_row = row_start()
            .items_center()
            .pe(1.)
            .border_t_1()
            .border_color(t.border)
            .child(
                icon(IconName::FolderOpen)
                    .size_4()
                    .flex_none()
                    .text_color(t.muted_foreground)
                    .ms(3.),
            )
            .child(
                div().flex_1().min_w_0().child(
                    Input::new(&self.folder)
                        .appearance(false)
                        .bordered(false)
                        .aria_label(t!("Folder to search")),
                ),
            )
            .child(
                Button::new("text-browse")
                    .ghost()
                    .small()
                    .label(t!("Browse…"))
                    .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx))),
            );

        let field = col()
            .id("text-search-field")
            .test_support()
            .relative()
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .bg(if in_field { t.field_focus } else { t.field })
            .child(query_row)
            .children(replace_row)
            .child(folder_row)
            // Fluent's edge: a thin line, and a 2 px accent line while typing.
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left(px(0.))
                    .right(px(0.))
                    .h(if in_field { px(2.) } else { px(1.) })
                    .bg(if in_field { t.primary } else { t.field_edge }),
            );

        let go = if searching {
            Button::new("text-stop")
                .outline()
                .icon(IconName::Square)
                .label(t!("Stop"))
                .tooltip(t!("Stop (Esc)"))
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("text-search")
                .primary()
                .icon(IconName::Search)
                .label(t!("Search"))
                .disabled(query_empty)
                .tooltip(t!("Search (Enter)"))
                .on_click(cx.listener(|this, _, window, cx| this.search_now(window, cx)))
        };

        row_start()
            .id("text-search-bar")
            .role(Role::Group)
            .aria_label(t!("Search bar"))
            .test_support()
            .flex_none()
            .items_start()
            .gap(space(2.))
            .px(space(3.))
            .py(space(2.5))
            .border_b_1()
            .border_color(t.border)
            .bg(theme::chrome_fill(cx))
            .child(field)
            .child(go)
            .child(
                Button::new("text-history-open")
                    .ghost()
                    .icon(IconName::ClockArrowLeft)
                    .tooltip(t!("Recent searches"))
                    .accessibility_label(t!("Recent searches"))
                    .on_click(cx.listener(|this, _, window, cx| this.show_history(window, cx))),
            )
    }

    /// Browse… (`browse`): a folder picked with Windows' picker, starting where the box says.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let start = Some(self.folder.read(cx).value().to_string()).filter(|p| !p.trim().is_empty());
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picking = self
            .source
            .pick_folder(t!("Folder to search").to_string(), start, owner);
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Some(folder)) = picking.await {
                this.update_in(cx, |this, window, cx| {
                    this.set_folder(&folder, window, cx);
                    this.save_now(cx);
                })
                .ok();
            }
        })
        .detach();
    }

    /// The cards under the bar: a missing ripgrep or ast-grep offers to install it; Meaning offers its model.
    pub(super) fn cards(&mut self, t: &theme::Tokens, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let options = Self::options(cx);
        let mut cards = Vec::new();
        let Some(tools) = self.tools.clone() else {
            return cards;
        };
        // ripgrep, which searches inside files, may be missing (a security tool quarantined it, a portable copy).
        cards.extend(tools::needed(
            &tools,
            "ripgrep",
            t!("ripgrep, which searches inside files, is missing from this copy of Glint."),
            IconName::TextSearch,
            cx,
        ));
        if options.code {
            cards.extend(tools::needed(
                &tools,
                "ast-grep",
                t!("Code patterns are searched by ast-grep, which knows 20-odd languages' syntax: install it to search by code's structure."),
                IconName::Braces,
                cx,
            ));
        }
        if options.meaning {
            cards.extend(self.meaning_card(&tools, t, cx));
        }
        cards
    }

    /// Shown while Meaning is on and its model isn't downloaded: says what it is, and gets it with one click
    /// (`MeaningCard`).
    fn meaning_card(
        &mut self,
        tools: &Entity<tools::Tools>,
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let model = tools.read(cx);
        // Not known yet, or there: nothing to offer.
        if !model.loaded() || model.meaning() != (false, false) {
            return None;
        }
        let why = t!(
            "Meaning search ranks passages by what they're about, with a small model (Model2Vec's potion-base-8M, 30 MB, MIT) downloaded once from Hugging Face and checked against its pinned checksum. What you search for never leaves this PC."
        );
        let control = if self.meaning_downloading {
            row_start()
                .gap(space(2.))
                .child(
                    div()
                        .id("text-meaning-progress")
                        .role(Role::Status)
                        .aria_label(t!("Downloading…"))
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(t!("Downloading…")),
                )
                .child(
                    Button::new("text-meaning-stop")
                        .outline()
                        .small()
                        .label(t!("Stop"))
                        .on_click(cx.listener(|this, _, _, cx| this.stop_meaning(cx))),
                )
                .into_any_element()
        } else {
            Button::new("text-meaning-download")
                .primary()
                .small()
                .icon(IconName::Download)
                .label(t!("Download it"))
                .on_click(cx.listener(|this, _, window, cx| this.download_meaning(window, cx)))
                .into_any_element()
        };
        Some(
            row_start()
                .id("text-meaning-card")
                .role(Role::Group)
                .aria_label(why.clone())
                .test_support()
                .m(space(3.))
                .gap(space(3.))
                .px(space(4.))
                .py(space(3.))
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.border)
                .bg(t.primary.opacity(0.05))
                .child(icon(IconName::Sparkles).size_4().flex_none().text_color(t.primary))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(theme::size::SM)
                        .text_start()
                        .child(why),
                )
                .child(control)
                .into_any_element(),
        )
    }
}
