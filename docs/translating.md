# Translating Glint

Glint's interface is written in English, and each language's translation is a gettext catalog in
[`locales/`](../locales):

| Catalog | Language |
| --- | --- |
| `zh-CN.po` | Chinese (Simplified), the first ([#9](https://github.com/shadijhade/glint/issues/9)) |
| `de.po` | German |
| `ar.po` | Arabic, written right to left: the window mirrors itself for it |

Glint shows a language when Windows' display language is that language. Any text a catalog hasn't translated yet
shows in English, so a catalog can be finished a piece at a time. The German and Arabic catalogs are complete
first drafts: corrections from people who speak them are welcome.

## Translating

1. **Get the catalog up to date with the code.** Fork and clone the repository, then run
   `node scripts/i18n.mjs update zh-CN` (Node 18 or later). It adds the texts that came in since the catalog was
   last updated. Where an English text changed, it keeps your old translation as a *fuzzy* guess. It moves the
   texts the code no longer has to the end of the file as obsolete (`#~`).
2. **Translate `locales/zh-CN.po`** in [Poedit](https://poedit.net/), any other PO editor, or a text editor.
   Each entry is the English (`msgid`) and your translation (`msgstr`). The `#:` lines say which source files use
   it, and `#.` lines are notes for you.
3. **See it in Glint.** You don't need to build Glint. Quit Glint (from its tray icon too), then start it with
   your file:

   ```powershell
   $env:GLINT_LANGUAGE = "D:\src\glint\locales\zh-CN.po"; & "$env:LOCALAPPDATA\Glint\Glint.exe"
   ```

   That's the setup program's Glint. Glint reads the file at start in place of the catalog built into it, so you
   see your latest work. Start it again to see later changes.
4. **Check it**: `node scripts/i18n.mjs status zh-CN` shows how much is translated and lists any translation
   Glint can't use (see the rules below). Glint shows the English for those until they're fixed.
5. **Send a pull request** with the `.po` file.

## Rules

- **Keep the placeholders.** `{name}`, `{n}`, `{count}`… are filled in by Glint, `%1` by Windows, and `%` in the
  Type column's words is the file's extension in capitals ("% image" reads "PNG image"). Keep every one the
  English has, in whatever order your language needs, and add none. A translation that loses or adds one is left
  out, and the English shows instead.
- **Counts are two texts:** "{n} file" (for 1) and "{n} files" (for any other number). In Chinese both are
  usually the same: "{n} 个文件". The text for 1 may leave `{n}` out ("eine Datei", "ملف واحد").
- **Arabic counts have four more forms.** In `ar.po`, each count's "{n} files" is followed by entries of the same
  English with a `msgctxt`. Glint picks the form by CLDR's rules for the number:

  | `msgctxt` | Numbers | Example |
  | --- | --- | --- |
  | `zero` | 0 | لا ملفات |
  | `two` | 2 | ملفان |
  | `few` | 3 to 10, 103 to 110… | {n} ملفات |
  | `many` | 11 to 99, 111 to 199… | {n} ملفًا |

  The entry without a context is "other": 100 to 102, 200 to 202…, and any form left empty. `zero` and `two` may
  leave `{n}` out. Poedit shows the context beside each one. `update` adds these entries to every count.
- **`{0}`, `{1}` in a menu's text are its keys** ("Cut ({0})" shows "Cut (Ctrl+X)"). Put them where your language
  puts them.
- **Use Windows' own words** where Windows has them: 此电脑, 回收站, 快速访问, 桌面, 文档, 下载, 图片, 音乐, 视频,
  文件资源管理器 (Dieser PC, Papierkorb, Schnellzugriff; هذا الكمبيوتر، سلة المحذوفات، الوصول السريع). People read
  Glint next to File Explorer.
- **Label, tag and comment are three different things,** and "marks" is all of them. A label is the one colored
  label a file can have, and tags are any number of free words. Give each its own word.
- **Leave the search box's words in English:** `ext:`, `tag:`, `size:`, `label:`… are what people type. Glint
  already reads them typed with a Chinese keyboard's `：` and `，`.
- **Fuzzy entries aren't used.** When you've checked one, remove its `fuzzy` mark (Poedit: "Needs work" off).
- **A few texts are fragments** ("on", "to", "By") that sit between other words. Translate them for their place,
  shown in the source file named in `#:`.

## In the code

- Every text the window shows is `t!("English text")`. Use `t!("Delete {name}?", name = file)` for a text with
  placeholders. The English is the key.
- Text written in a table and translated where it's shown (a column's label, a known folder's name) is
  `english!("…")` where it's written. The view shows it with `i18n::translate(text)`.
- Counts go through `glint_model::format::plural(n, "file")`, with the noun written as text right there. The
  extractor turns it into "{n} file" and "{n} files".
- A comment that starts `// For translators:` just above a text goes to the translators with it. Just above a
  table, it goes with every text in the table.
- `node scripts/i18n.mjs extract --verbose` writes [`locales/glint.pot`](../locales/glint.pot), every text in the
  code, and lists the calls that translate a text from a variable. Each of those texts needs `english!` or `t!`
  where it's written. Run `node scripts/i18n.mjs update` before a release so the catalogs carry the new texts.
- `GLINT_LANGUAGE=qps-ploc` shows every text accented, a third longer and in brackets: `[Ďéĺéťé ··]`. A text that
  stays plain skips the catalog. A text that lost its `]` is cut off where a longer language would be.
- `GLINT_LANGUAGE=en` (or `zh-CN`, `de`, `ar`) picks a language whatever Windows speaks. The e2e run pins `en`,
  because its checks find controls by their English names.
- Arabic mirrors the window: views lay out with the kit's start/end helpers and ask `i18n::is_rtl()` where they
  must. The view tests check them mirrored (`testing::set_rtl`).

## How it works

`crates/glint-model/build.rs` compiles each `locales/<code>.po` into a table sorted by its English, so Glint reads
nothing at start, and a lookup is a binary search over static text. It leaves out an entry no one has
translated, a fuzzy one, and one it can't use. It warns about the ones it can't use, and
`every_translation_in_locales_is_used` fails on them. `crates/glint-model/src/i18n.rs` picks the language: it
takes the first of Windows' display languages that has a catalog, and English counts as one. Chinese
(Traditional), `zh-TW` and `zh-HK`, has no catalog yet, so it shows English, not Simplified. A count's other
forms are keyed `few\u{4}{n} files` in the same table, and `count_form` says which form a number takes.

**Adding a language:** `node scripts/i18n.mjs update fr` starts `locales/fr.po`. Then:

1. Give it a `Language` in i18n.rs: its code in `code` and `from_code`, its table in `catalog`, its byte in
   `from_byte`.
2. Add it to build.rs's `KNOWN`.
3. If it's written right to left, add it to `is_rtl`.
4. If its counts have more forms than one and other (Russian, Polish, Czech…), add them:
   - its rules in `count_form`;
   - its forms in `COUNT_FORMS` in scripts/lib/i18n.mjs.

   CLDR's names are `zero`, `two`, `few` and `many`.
