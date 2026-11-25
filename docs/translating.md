# Translating Glint

Glint's interface is written in English, and each language's translation is a gettext catalog in
[`locales/`](../locales): `zh-CN.po` is Chinese (Simplified), the first one ([#9](https://github.com/shadijhade/glint/issues/9)).
Glint shows a language when Windows' display language is that language. Any text a catalog hasn't translated yet
shows in English, so a catalog can be finished a piece at a time.

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
  usually the same: "{n} 个文件".
- **`{0}`, `{1}` in a menu's text are its keys** ("Cut ({0})" shows "Cut (Ctrl+X)"). Put them where your language
  puts them.
- **Use Windows' own words** where Windows has them: 此电脑, 回收站, 快速访问, 桌面, 文档, 下载, 图片, 音乐, 视频,
  文件资源管理器. People read Glint next to File Explorer.
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
- `GLINT_LANGUAGE=en` (or `zh-CN`) picks a language whatever Windows speaks. The e2e run pins `en`, because its
  checks find controls by their English names.

## How it works

`crates/glint-model/build.rs` compiles each `locales/<code>.po` into a table sorted by its English, so Glint reads
nothing at start, and a lookup is a binary search over static text. It leaves out an entry no one has
translated, a fuzzy one, and one it can't use. It warns about the ones it can't use, and
`every_translation_in_locales_is_used` fails on them. `crates/glint-model/src/i18n.rs` picks the language: it
takes the first of Windows' display languages that has a catalog, and English counts as one. Chinese
(Traditional), `zh-TW` and `zh-HK`, has no catalog yet, so it shows English, not Simplified.

**Adding a language:** `node scripts/i18n.mjs update de-DE` starts `locales/de-DE.po`. Then give it a `Language`
in i18n.rs (its code in `from_code`, its table in `catalog`) and add it to build.rs's `KNOWN`. Glint's counts have
two forms, one and other, which fits Chinese, English, German, French, Spanish and Japanese. A language with more
forms, such as Russian, Polish or Arabic, needs `plural` taught about them first.
