//! The sections of Settings that are settings and nothing else (or a picker and a list): Appearance, Files, Find
//! files by name, Quick search, Search text, Text search skips, Terminal, Check and History. Each control reads
//! `state::settings` and writes through `state::update`, as the TypeScript's `set({..})` did.

use super::shortcut;
use super::widgets::{self, named, setting_row, Named};
use super::{SettingsView, ABOUT};
use crate::kit::prelude::*;
use crate::persistence;
use crate::{core_bridge, state, t, theme};
use glint_model::grid::TileSize;
use glint_model::grid::TILE_SIZES;
use glint_model::paths::same_path;
use glint_model::store::{Accent, Backdrop, FontSize, Language, OrderedMap, Settings, StartFolder, Theme};

/// The presets of "Text search skips": a name, an icon and the patterns it adds (`PRESETS`).
struct Preset {
    slot: &'static str,
    icon: IconName,
    items: &'static [&'static str],
}

const PRESETS: [Preset; 6] = [
    Preset {
        slot: "folders",
        icon: IconName::Folder,
        items: &[
            "node_modules",
            ".git",
            "dist",
            "build",
            "bin",
            "obj",
            ".vs",
            ".idea",
            "__pycache__",
            ".cache",
            "vendor",
            "packages",
            ".nuget",
            "coverage",
            ".next",
            "target",
        ],
    },
    Preset {
        slot: "binaries",
        icon: IconName::Binary,
        items: &[
            "*.exe", "*.dll", "*.so", "*.dylib", "*.pdb", "*.lib", "*.a", "*.o", "*.class", "*.pyc",
        ],
    },
    Preset {
        slot: "images",
        icon: IconName::Image,
        items: &[
            "*.png", "*.jpg", "*.jpeg", "*.gif", "*.ico", "*.svg", "*.webp", "*.bmp", "*.tiff", "*.psd",
        ],
    },
    Preset {
        slot: "archives",
        icon: IconName::Archive,
        items: &["*.zip", "*.rar", "*.7z", "*.tar", "*.gz", "*.bz2", "*.xz", "*.iso"],
    },
    Preset {
        slot: "fonts",
        icon: IconName::Type,
        items: &["*.woff", "*.woff2", "*.ttf", "*.eot", "*.otf"],
    },
    Preset {
        slot: "lock-files",
        icon: IconName::Lock,
        items: &[
            "package-lock.json",
            "yarn.lock",
            "pnpm-lock.yaml",
            "Cargo.lock",
            "Gemfile.lock",
            "composer.lock",
            "poetry.lock",
        ],
    },
];

fn preset_label(slot: &str) -> SharedString {
    match slot {
        "folders" => t!("Folders"),
        "binaries" => t!("Binaries"),
        "images" => t!("Images"),
        "archives" => t!("Archives"),
        "fonts" => t!("Fonts"),
        "lock-files" => t!("Lock files"),
        _ => SharedString::default(),
    }
}

/// The patterns of `items` that `exclusions` has, and the list after pressing the preset: all of them out when
/// every one is in, else every missing one added.
fn pressed(exclusions: &[String], items: &[&str]) -> Vec<String> {
    let all = items.iter().all(|item| exclusions.iter().any(|e| e == item));
    if all {
        exclusions
            .iter()
            .filter(|e| !items.contains(&e.as_str()))
            .cloned()
            .collect()
    } else {
        let mut next = exclusions.to_vec();
        next.extend(
            items
                .iter()
                .filter(|item| !exclusions.iter().any(|e| e == **item))
                .map(|item| item.to_string()),
        );
        next
    }
}

/// What a bit of history is cleared by: the searches kept for Home, or the folders Ctrl+K ranks.
#[derive(Clone, Copy)]
pub(super) enum Clear {
    History,
    Visits,
}

/// Clears the searches kept for Home, or the folders the command palette ranks, with Undo (`clearWithUndo`).
fn clear_with_undo(what: Clear, window: &mut Window, cx: &mut App) {
    let Some(app) = persistence::app(cx) else { return };
    let (history, visits) = (app.history.clone(), app.visits.clone());
    persistence::update_app(cx, |app| match what {
        Clear::History => app.history.clear(),
        Clear::Visits => app.visits = OrderedMap::new(),
    });
    let said = match what {
        Clear::History => t!("Cleared the search history"),
        Clear::Visits => t!("Cleared the folders remembered"),
    };
    window.push_notification(
        Notification::success(said).action(move |_, _, cx| {
            let (history, visits) = (history.clone(), visits.clone());
            Button::new("settings-undo-clear")
                .label(t!("Undo"))
                .on_click(cx.listener(move |this, _, window, cx| {
                    restore(what, history.clone(), visits.clone(), cx);
                    this.dismiss(window, cx);
                }))
        }),
        cx,
    );
}

/// Puts back what [`clear_with_undo`] cleared.
pub(super) fn restore(
    what: Clear,
    history: Vec<glint_model::store::HistoryItem>,
    visits: OrderedMap<glint_model::rank::Visit>,
    cx: &mut App,
) {
    persistence::update_app(cx, move |app| match what {
        Clear::History => app.history = history,
        Clear::Visits => app.visits = visits,
    });
}

impl SettingsView {
    /// A switch row writing a flag of the settings.
    fn flag(
        &self,
        slot: &str,
        label: SharedString,
        hint: Option<SharedString>,
        checked: bool,
        set: fn(&mut Settings, bool),
        t: &theme::Tokens,
    ) -> AnyElement {
        widgets::switch_row(
            slot,
            label,
            hint,
            checked,
            false,
            move |on, _, cx| state::update(cx, |settings| set(settings, on)),
            t,
        )
        .into_any_element()
    }

    /// A row of segmented choices writing a setting.
    fn choice<T: PartialEq + Clone + 'static>(
        &self,
        name: Named,
        current: T,
        options: Vec<(T, SharedString)>,
        set: fn(&mut Settings, T),
        t: &theme::Tokens,
        cx: &App,
    ) -> AnyElement {
        let Named { slot, label, hint } = name;
        let control = widgets::segmented(
            &slot,
            label.clone(),
            &current,
            options,
            move |value, _, cx| state::update(cx, |settings| set(settings, value)),
            cx,
        );
        setting_row(label, hint, vec![control], t).into_any_element()
    }

    /// Every section in the order the page draws them. Sections that draw nothing until their service has
    /// answered once (content index, photos and recordings, devices) are left out until then, as the
    /// TypeScript's `return null` left them.
    pub(super) fn page(&mut self, t: &theme::Tokens, cx: &mut Context<Self>) -> Vec<(&'static str, AnyElement)> {
        let s = state::settings(cx).clone();
        let mut page: Vec<(&'static str, AnyElement)> = vec![
            ("appearance", self.appearance(&s, t, cx).into_any_element()),
            ("files", self.files(&s, t, cx).into_any_element()),
            (
                "instant-search",
                widgets::section(
                    "instant-search",
                    t!("Instant search"),
                    None,
                    vec![self.instant.clone().into_any_element()],
                    t,
                )
                .into_any_element(),
            ),
        ];
        if self.content.read(cx).status().is_some() {
            page.push(("search-inside-files-instantly", self.content.clone().into_any_element()));
        }
        if self.media.read(cx).status().is_some() {
            page.push(("photos-and-recordings", self.media.clone().into_any_element()));
        }
        if self.devices.read(cx).status().is_some() {
            page.push(("devices", self.devices.clone().into_any_element()));
        }
        page.extend([
            ("find-files-by-name", self.find(&s, t, cx).into_any_element()),
            ("quick-search", self.quick_search(&s, t, cx).into_any_element()),
            ("ai-agents", self.agents.clone().into_any_element()),
            ("search-text-ripgrep", self.search_text(&s, t, cx).into_any_element()),
            ("text-search-skips", self.text_skips(&s, t, cx).into_any_element()),
            ("terminal", self.terminal(&s, t, cx).into_any_element()),
            ("check", self.check(&s, t, cx).into_any_element()),
            ("clouds", self.clouds.clone().into_any_element()),
            ("tools", self.tools.clone().into_any_element()),
            ("history", self.history(&s, t, cx).into_any_element()),
            (
                ABOUT.strip_prefix("settings-").unwrap_or("about"),
                self.about.clone().into_any_element(),
            ),
        ]);
        page
    }

    fn appearance(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let mica_missing = s.backdrop == Backdrop::Mica && !theme::backdrop_on(cx);
        let rows = vec![
            self.choice(
                named("theme", t!("Theme"), None),
                s.theme,
                vec![(Theme::Light, t!("Light")), (Theme::Dark, t!("Dark")), (Theme::System, t!("System"))],
                |s, v| s.theme = v,
                t,
                cx,
            ),
            self.choice(
                named("language", t!("Language"), Some(t!("The interface's language. Glint restarts its window to change it; what isn't translated yet stays in English."))),
                s.language,
                vec![
                    (Language::System, t!("Windows' language")),
                    (Language::En, "English".into()),
                    (Language::Ar, "العربية".into()),
                ],
                |s, v| s.language = v,
                t,
                cx,
            ),
            self.choice(
                named(
                    "backdrop",
                    t!("Backdrop"),
                    Some(if mica_missing {
                        t!("Mica needs Windows 11: the window stays solid here")
                    } else {
                        t!("Windows 11's Mica behind the title bar and the sidebar, as in File Explorer")
                    }),
                ),
                s.backdrop,
                vec![(Backdrop::Mica, t!("Mica")), (Backdrop::Solid, t!("Solid"))],
                |s, v| s.backdrop = v,
                t,
                cx,
            ),
            self.choice(
                named("accent", t!("Accent color"), Some(t!("Glint's pink, or the color chosen in Windows' Settings → Personalization → Colors"))),
                s.accent,
                vec![(Accent::Glint, t!("Glint")), (Accent::Windows, t!("Windows"))],
                |s, v| s.accent = v,
                t,
                cx,
            ),
            self.choice(
                named("font-size", t!("Text size"), None),
                s.font_size,
                vec![
                    (FontSize::Small, t!("Small")),
                    (FontSize::Medium, t!("Medium")),
                    (FontSize::Large, t!("Large")),
                ],
                |s, v| s.font_size = v,
                t,
                cx,
            ),
            self.flag(
                "line-numbers",
                t!("Line numbers in preview"),
                None,
                s.show_line_numbers,
                |s, v| s.show_line_numbers = v,
                t,
            ),
        ];
        widgets::section("appearance", t!("Appearance"), None, rows, t)
    }

    fn files(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let mut rows: Vec<AnyElement> = vec![
            self.flag(
                "restore-tabs",
                t!("Reopen my tabs"),
                Some(t!("Each pane's tabs, as they were when the app closed")),
                s.restore_tabs,
                |s, v| s.restore_tabs = v,
                t,
            ),
            self.choice(
                named("start-folder", if s.restore_tabs { t!("Otherwise open") } else { t!("Open on start") }, None),
                s.start_folder,
                vec![
                    (StartFolder::Home, t!("Home")),
                    (StartFolder::Recent, t!("Recent")),
                    (StartFolder::ThisPc, t!("This PC")),
                    (StartFolder::Last, t!("Last folder")),
                ],
                |s, v| s.start_folder = v,
                t,
                cx,
            ),
            self.flag(
                "show-hidden",
                t!("Show hidden items"),
                Some(t!("Files and folders Windows marks as hidden · Ctrl+H")),
                s.show_hidden,
                |s, v| s.show_hidden = v,
                t,
            ),
            self.flag("folders-first", t!("Folders first"), None, s.folders_first, |s, v| s.folders_first = v, t),
            self.flag(
                "per-folder-views",
                t!("Each folder keeps its own view"),
                Some(t!("Its columns, sort and list or grid, as you last set them there; until then, its kind's (a folder of pictures shows dimensions and when each was taken). Right-click the list's header for the columns")),
                s.per_folder_views,
                |s, v| s.per_folder_views = v,
                t,
            ),
            self.flag(
                "preview-pane",
                t!("Preview pane"),
                Some(t!("Alt+P")),
                s.show_preview_pane,
                |s, v| s.show_preview_pane = v,
                t,
            ),
            self.flag(
                "folder-tree",
                t!("Folders in the sidebar"),
                Some(t!("Drives, Quick access and pinned folders open with their arrow to show the folders in them")),
                s.folder_tree,
                |s, v| s.folder_tree = v,
                t,
            ),
            widgets::switch_row(
                "tree-follows",
                t!("Open the sidebar to the folder shown"),
                Some(t!("Its drive opens down to it as you go, as File Explorer's Expand to open folder")),
                s.tree_follows,
                !s.folder_tree,
                |on, _, cx| state::update(cx, |s| s.tree_follows = on),
                t,
            )
            .into_any_element(),
        ];
        rows.extend(self.gallery_folders(s, t, cx));
        rows.extend([
            self.flag(
                "copy-queue",
                t!("Copy and move with Glint's queue"),
                Some(t!("Paste, drops and the other pane's copies wait their turn in the copy queue, with Pause and Resume, instead of Windows' copy dialog. It copies small files several at a time, so folders of thousands of them go much faster. Paste with the copy queue is in the folder's menu either way")),
                s.copy_queue,
                |s, v| s.copy_queue = v,
                t,
            ),
            self.flag(
                "verify-copies",
                t!("Check each copy the queue makes"),
                Some(t!("Each copy is read back from the disk and compared with its original (BLAKE3): it takes longer, and catches a disk or a cable that changes what it's given")),
                s.verify_copies,
                |s, v| s.verify_copies = v,
                t,
            ),
            self.flag(
                "list-thumbnails",
                t!("Thumbnails in the list"),
                Some(t!("Pictures and videos show a small one in place of their icon. The grid view (Ctrl+Shift+2) shows them for every file Windows has one of.")),
                s.list_thumbnails,
                |s, v| s.list_thumbnails = v,
                t,
            ),
            self.flag(
                "show-readme",
                t!("A folder's README above the list"),
                Some(t!("Its README.md, folded to its title until you open it")),
                s.show_readme,
                |s, v| s.show_readme = v,
                t,
            ),
            self.choice(
                named("tile-size", t!("Tile size in the grid"), Some(t!("Ctrl+wheel, Ctrl+= and Ctrl+- in the grid"))),
                s.tile_size,
                TILE_SIZES
                    .iter()
                    .map(|spec| (spec.size, tile_label(spec.size)))
                    .collect::<Vec<(TileSize, SharedString)>>(),
                |s, v| s.tile_size = v,
                t,
                cx,
            ),
            self.flag(
                "confirm-delete",
                t!("Ask before moving to the Recycle Bin"),
                Some(t!("Deleting permanently (Shift+Del) always asks")),
                s.confirm_delete,
                |s, v| s.confirm_delete = v,
                t,
            ),
            self.integration.clone().into_any_element(),
        ]);
        widgets::section("files", t!("Files"), None, rows, t)
    }

    /// The gallery's folders besides Pictures and Videos: added with a folder picker, each taken out with its
    /// cross (`GalleryFolders`).
    fn gallery_folders(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows: Vec<AnyElement> = vec![setting_row(
            t!("Gallery folders"),
            Some(t!(
                "Besides Pictures and Videos: their photos and videos show in the Gallery, with everything below them"
            )),
            vec![widgets::button("gallery-add", t!("Add a folder"), Some(IconName::Plus))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.browse(
                        t!("A folder for the gallery"),
                        String::new(),
                        window,
                        cx,
                        |s, picked| {
                            if !s.gallery_folders.iter().any(|f| same_path(f, &picked)) {
                                s.gallery_folders.push(picked);
                            }
                        },
                    )
                }))
                .into_any_element()],
            t,
        )
        .into_any_element()];
        let amber = theme::tailwind("amber-500").unwrap_or(t.primary);
        for (i, folder) in s.gallery_folders.iter().enumerate() {
            let removed = folder.clone();
            rows.push(
                row_start()
                    .id(widgets::id(&["settings-gallery", &i.to_string()]))
                    .role(Role::ListItem)
                    .aria_label(SharedString::from(folder.clone()))
                    .test_support()
                    .gap(space(2.))
                    .px(space(4.))
                    .py(space(1.5))
                    .text_size(theme::size::SM)
                    .child(icon(IconName::Folder).size_4().flex_none().text_color(amber))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_start()
                            .child(isolate(folder.clone())),
                    )
                    .child(
                        Button::new(widgets::id(&["settings-gallery-remove", &i.to_string()]))
                            .ghost()
                            .small()
                            .icon(IconName::X)
                            .tooltip(t!("Take it out of the gallery"))
                            .accessibility_label(t!("Take {folder} out of the gallery", folder = folder))
                            .on_click(move |_, _, cx| {
                                let removed = removed.clone();
                                state::update(cx, |s| s.gallery_folders.retain(|f| *f != removed));
                            }),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    /// Asks for a folder and hands it to `apply` (which changes the settings) when one is picked (`browse`).
    pub(crate) fn browse(
        &mut self,
        title: SharedString,
        start: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        apply: fn(&mut Settings, String),
    ) {
        let owner = core_bridge::hwnd_of(window).map_or(0, |h| h.0 as isize);
        let picked = self
            .backend
            .pick_folder(title.to_string(), Some(start).filter(|s| !s.is_empty()), owner);
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Some(folder)) = picked.await {
                cx.update(|_, cx| state::update(cx, |settings| apply(settings, folder)))
                    .ok();
            }
        })
        .detach();
    }

    fn find(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let rows = vec![
            self.choice(
                named("find-max-results", t!("Result limit"), Some(t!("Stops after this many files and folders"))),
                s.find_max_results,
                vec![
                    (Some(10_000), "10k".into()),
                    (Some(50_000), "50k".into()),
                    (Some(200_000), "200k".into()),
                    (None, t!("None")),
                ],
                |s, v| s.find_max_results = v,
                t,
                cx,
            ),
            self.flag(
                "find-respect-ignore",
                t!("Skip what .gitignore lists"),
                Some(t!("Leaves out build output and dependencies in code folders. Searches with fd: only fd reads .gitignore files.")),
                s.find_respect_ignore,
                |s, v| s.find_respect_ignore = v,
                t,
            ),
            col()
                .gap(space(3.))
                .p(space(4.))
                .child(
                    div()
                        .text_size(theme::size::XS)
                        .text_start()
                        .text_color(t.muted_foreground)
                        .child(t!("Folders and patterns name searches skip. Hidden items are searched only while they're shown.")),
                )
                .child(self.find_exclusions.clone())
                .into_any_element(),
        ];
        widgets::section("find-files-by-name", t!("Find files by name"), None, rows, t)
    }

    fn quick_search(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        // Off and the three presets, as the TypeScript offered; a shortcut set otherwise (typed below, or
        // imported) is shown too, as the chosen one.
        let mut options: Vec<(Option<String>, SharedString)> = vec![(None, t!("Off"))];
        for preset in shortcut::PRESETS {
            options.push((Some(preset.to_string()), preset.into()));
        }
        if let Some(custom) = s
            .quick_shortcut
            .as_ref()
            .filter(|c| !shortcut::PRESETS.contains(&c.as_str()))
        {
            options.push((Some(custom.clone()), custom.clone().into()));
        }
        let hint = shortcut::hint(self.quick_taken.as_deref(), s.quick_shortcut.as_deref());
        let has_custom = !self.shortcut_box.read(cx).value().trim().is_empty();
        let rows = vec![
            self.choice(
                named("quick-shortcut", t!("Shortcut"), Some(hint)),
                s.quick_shortcut.clone(),
                options,
                |s, v| s.quick_shortcut = v,
                t,
                cx,
            ),
            col()
                .gap(space(1.5))
                .px(space(4.))
                .py(space(3.))
                .child(
                    row_start()
                        .gap(space(2.))
                        .child(
                            div()
                                .w(space(56.))
                                .child(Input::new(&self.shortcut_box).aria_label(t!("Another quick search shortcut"))),
                        )
                        .child(
                            widgets::button("quick-shortcut-set", t!("Set"), None)
                                .disabled(!has_custom)
                                .on_click(cx.listener(|this, _, window, cx| this.set_custom_shortcut(window, cx))),
                        ),
                )
                .children(self.shortcut_error.clone().map(|error| {
                    div()
                        .id("settings-shortcut-error")
                        .role(Role::Alert)
                        .aria_label(error.clone())
                        .test_support()
                        .text_size(theme::size::XS)
                        .text_start()
                        .text_color(t.destructive)
                        .child(error)
                }))
                .into_any_element(),
            self.flag(
                "keep-in-tray",
                t!("Keep running in the tray"),
                Some(t!(
                    "Closing the window leaves Glint in the notification area, so the shortcut works anytime"
                )),
                s.keep_in_tray,
                |s, v| s.keep_in_tray = v,
                t,
            ),
            widgets::switch_row(
                "start-with-windows",
                t!("Start with Windows"),
                Some(t!("In the tray, when you sign in")),
                s.start_with_windows && s.keep_in_tray,
                !s.keep_in_tray,
                |on, _, cx| state::update(cx, |s| s.start_with_windows = on),
                t,
            )
            .into_any_element(),
        ];
        widgets::section("quick-search", t!("Quick search"), None, rows, t)
    }

    /// A text box with a Browse button that picks a folder into the same setting.
    fn path_row(
        &self,
        name: Named,
        input: &Entity<InputState>,
        picker: (SharedString, String),
        apply: fn(&mut Settings, String),
        t: &theme::Tokens,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Named { slot, label, hint } = name;
        let (title, start) = picker;
        setting_row(
            label.clone(),
            hint,
            vec![
                div()
                    .w(space(52.))
                    .child(Input::new(input).aria_label(label))
                    .into_any_element(),
                widgets::button(&format!("{slot}-browse"), t!("Browse…"), None)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.browse(title.clone(), start.clone(), window, cx, apply)
                    }))
                    .into_any_element(),
            ],
            t,
        )
        .into_any_element()
    }

    fn search_text(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let chosen = self
            .editors
            .iter()
            .find(|e| e.path.to_lowercase() == s.editor_path.trim().to_lowercase())
            .map(|e| e.id);
        let editor_picker: Vec<AnyElement> = if self.editors.is_empty() {
            Vec::new()
        } else {
            vec![widgets::segmented(
                "editor-picker",
                t!("An editor found on this PC"),
                &chosen,
                self.editors
                    .iter()
                    .map(|e| (Some(e.id), SharedString::from(e.name)))
                    .collect::<Vec<_>>(),
                {
                    let editors = self.editors.clone();
                    move |picked, _, cx| {
                        if let Some(editor) = editors.iter().find(|e| Some(e.id) == picked) {
                            let path = editor.path.clone();
                            state::update(cx, move |s| s.editor_path = path);
                        }
                    }
                },
                cx,
            )]
        };
        let mut editor_controls = vec![div()
            .w(space(52.))
            .child(Input::new(&self.editor_path).aria_label(t!("Open files with")))
            .into_any_element()];
        editor_controls.extend(editor_picker);
        let rows = vec![
            self.path_row(
                named("default-search-path", t!("Default folder"), Some(t!("Used when no folder is filled in"))),
                &self.default_search_path,
                (t!("A folder for text searches"), s.default_search_path.clone()),
                |s, v| s.default_search_path = v,
                t,
                cx,
            ),
            self.choice(
                named("max-results", t!("Result limit"), Some(t!("Stops searching after this many matching lines"))),
                s.max_results,
                vec![
                    (Some(1_000), "1k".into()),
                    (Some(10_000), "10k".into()),
                    (Some(50_000), "50k".into()),
                    (None, t!("None")),
                ],
                |s, v| s.max_results = v,
                t,
                cx,
            ),
            self.flag(
                "auto-open-preview",
                t!("Preview the first result"),
                Some(t!("Show the first match as soon as results arrive")),
                s.auto_open_preview,
                |s, v| s.auto_open_preview = v,
                t,
            ),
            self.flag(
                "confirm-before-replace",
                t!("Confirm before replacing"),
                None,
                s.confirm_before_replace,
                |s, v| s.confirm_before_replace = v,
                t,
            ),
            setting_row(
                t!("Open files with"),
                Some(t!("A program name or path, e.g. code or notepad++ (results open at their line), or an editor found on this PC")),
                editor_controls,
                t,
            )
            .into_any_element(),
        ];
        widgets::section("search-text-ripgrep", t!("Search text (ripgrep)"), None, rows, t)
    }

    fn text_skips(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let remove_all = (!s.exclusions.is_empty()).then(|| {
            widgets::quiet_button("exclusions-remove-all", t!("Remove all"))
                .on_click(|_, _, cx| state::update(cx, |s| s.exclusions.clear()))
                .into_any_element()
        });
        let presets = PRESETS.iter().map(|preset| {
            let active = preset
                .items
                .iter()
                .filter(|i| s.exclusions.iter().any(|e| e == **i))
                .count();
            let all = active == preset.items.len();
            let label = preset_label(preset.slot);
            let items = preset.items;
            Button::new(widgets::id(&["settings-preset", preset.slot]))
                .custom(
                    ButtonCustomVariant::new(cx)
                        .color(if all {
                            t.primary.opacity(0.1)
                        } else {
                            t.background.opacity(0.)
                        })
                        .foreground(t.foreground)
                        .hover(t.accent)
                        .active(t.accent),
                )
                .selected(all)
                .accessibility_label(t!(
                    "{name}, {active} of {total}",
                    name = label,
                    active = active,
                    total = items.len()
                ))
                .tooltip(items.join("  "))
                .min_w(space(40.))
                .flex_1()
                .h_auto()
                .py(space(2.))
                .border_1()
                .border_color(if all {
                    t.primary.opacity(0.4)
                } else if active > 0 {
                    t.ring.opacity(0.6)
                } else {
                    t.border
                })
                .on_click(move |_, _, cx| {
                    state::update(cx, |s| s.exclusions = pressed(&s.exclusions, items));
                })
                .child(
                    row_start()
                        .w_full()
                        .gap(space(2.))
                        .child(
                            icon(preset.icon)
                                .size_4()
                                .text_color(if all { t.primary } else { t.muted_foreground }),
                        )
                        .child(div().child(label))
                        .child(
                            div()
                                .ms(0.)
                                .flex_1()
                                .text_end()
                                .text_size(theme::size::XS)
                                .text_color(t.muted_foreground)
                                .child(format!("{active}/{}", items.len())),
                        ),
                )
        });
        let rows = vec![col()
            .gap(space(4.))
            .p(space(4.))
            .child(
                div()
                    .text_size(theme::size::XS)
                    .text_start()
                    .text_color(t.muted_foreground)
                    .child(t!("Glob patterns every text search skips. Files ignored by .gitignore, hidden files and binary files are skipped too.")),
            )
            .child(row_start().flex_wrap().gap(space(2.)).children(presets))
            .child(self.exclusions.clone())
            .into_any_element()];
        widgets::section("text-search-skips", t!("Text search skips"), remove_all, rows, t)
    }

    fn terminal(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let current = self
            .shells
            .iter()
            .find(|shell| shell.id == s.terminal_shell)
            .or_else(|| self.shells.first())
            .map(|shell| shell.id.to_string())
            .unwrap_or_default();
        let options: Vec<(String, SharedString)> = self
            .shells
            .iter()
            .map(|shell| (shell.id.to_string(), SharedString::from(shell.name)))
            .collect();
        let rows = vec![
            self.choice(
                named("terminal-shell", t!("Shell"), Some(t!("New terminals start it. Ctrl+` opens one under the files, in the folder shown"))),
                current,
                options,
                |s, v| s.terminal_shell = v,
                t,
                cx,
            ),
            self.flag(
                "terminal-follow",
                t!("Follow the folder shown"),
                Some(t!("A cd in the terminal opens that folder here, and a folder opened here is cd'd to while the shell waits at its prompt")),
                s.terminal_follow,
                |s, v| s.terminal_follow = v,
                t,
            ),
        ];
        widgets::section("terminal", t!("Terminal"), None, rows, t)
    }

    fn check(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let rows = vec![self.path_row(
            named(
                "yara-rules",
                t!("YARA rules"),
                Some(t!("A folder of .yar and .yara files (its subfolders too), for Check with YARA rules: YARA-X compiles them each time")),
            ),
            &self.yara_rules,
            (t!("A folder of YARA rules (.yar, .yara)"), s.yara_rules.clone()),
            |s, v| s.yara_rules = v,
            t,
            cx,
        )];
        widgets::section("check", t!("Check"), None, rows, t)
    }

    fn history(&self, s: &Settings, t: &theme::Tokens, cx: &mut Context<Self>) -> AnyElement {
        let (searches, visited) = persistence::app(cx).map_or((0, 0), |app| (app.history.len(), app.visits.len()));
        let zoxide = self.tools.read(cx).tool("zoxide").and_then(|tool| tool.path.clone());
        let rows = vec![
            self.flag(
                "save-search-history",
                t!("Save search history"),
                None,
                s.save_search_history,
                |s, v| s.save_search_history = v,
                t,
            ),
            self.choice(
                named("max-history-items", t!("Keep up to"), None),
                s.max_history_items,
                vec![
                    (100, "100".into()),
                    (500, "500".into()),
                    (1000, glint_model::format::format_count(1000).into()),
                    (5000, glint_model::format::format_count(5000).into()),
                ],
                |s, v| s.max_history_items = v,
                t,
                cx,
            ),
            setting_row(
                if searches == 1 {
                    t!("{n} search in the history", n = glint_model::format::format_count(searches))
                } else {
                    t!("{n} searches in the history", n = glint_model::format::format_count(searches))
                },
                None,
                vec![widgets::button("history-clear", t!("Clear history"), None)
                    .disabled(searches == 0)
                    .on_click(|_, window, cx| clear_with_undo(Clear::History, window, cx))
                    .into_any_element()],
                t,
            )
            .into_any_element(),
            setting_row(
                if visited == 1 {
                    t!("{n} folder remembered", n = glint_model::format::format_count(visited))
                } else {
                    t!("{n} folders remembered", n = glint_model::format::format_count(visited))
                },
                Some(t!("The command palette (Ctrl+K) offers the folders you use most first")),
                vec![widgets::button("visits-clear", t!("Clear folders"), None)
                    .disabled(visited == 0)
                    .on_click(|_, window, cx| clear_with_undo(Clear::Visits, window, cx))
                    .into_any_element()],
                t,
            )
            .into_any_element(),
            widgets::switch_row(
                "zoxide",
                t!("Share folder history with zoxide"),
                Some(if zoxide.is_some() {
                    t!("Ctrl+K and quick search rank the folders you cd to in a terminal too, and z there finds the folders you open here")
                } else {
                    t!("zoxide, the smarter cd for terminals, isn't installed: Tools above installs it")
                }),
                s.zoxide,
                zoxide.is_none() && !s.zoxide,
                |on, _, cx| {
                    // `refreshZoxide` needs no call: the palette and quick search ask for zoxide's folders each
                    // time they open, while this setting is on.
                    state::update(cx, |s| s.zoxide = on)
                },
                t,
            )
            .into_any_element(),
        ];
        widgets::section("history", t!("History"), None, rows, t)
    }
}

/// A grid tile size's name (`TILE_SIZES`' labels).
fn tile_label(size: TileSize) -> SharedString {
    match size {
        TileSize::Small => t!("Small"),
        TileSize::Medium => t!("Medium"),
        TileSize::Large => t!("Large"),
        TileSize::XLarge => t!("Extra large"),
        TileSize::Huge => t!("Huge"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_preset_adds_what_is_missing_then_takes_them_all_out() {
        let items = ["*.zip", "*.rar", "*.7z"];
        // Partly there: the rest is added and what was already there keeps its place.
        assert_eq!(
            pressed(&list(&["a", "*.rar"]), &items),
            list(&["a", "*.rar", "*.zip", "*.7z"])
        );
        // All there: all go, and nothing else does.
        assert_eq!(pressed(&list(&["*.zip", "a", "*.rar", "*.7z"]), &items), list(&["a"]));
        assert_eq!(pressed(&[], &items), list(&items));
    }
}
