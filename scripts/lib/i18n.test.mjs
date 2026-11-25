// Proves scripts/lib/i18n.mjs, which finds the interface's text for the translators: what counts as a text in
// Rust (and what doesn't: tests, comments, look-alike calls), how a catalog is read and written back as gettext
// writes it, and how `update` keeps translations across a change of the code.
//
// Usage: node scripts/lib/i18n.test.mjs      (npm run test:i18n)

import assert from "node:assert/strict";
import { catalogStatus, COUNT_ONE_NOTE, COUNT_OTHER_NOTE, countForms, extractFromRust, likeness, merge, parsePo, placeholders, placeholdersDiffer, template, tokenize, writePo } from "./i18n.mjs";

const texts = (source) => extractFromRust(source).texts.map((t) => t.text);

// ------------------------------------------------------------------------------------------------- Rust

{
    const source = String.raw`
// t!("in a comment") is not a text, nor /* t!("in a block /* nested */ comment") */
fn view() {
    let a = t!("Open");
    let b = t!(
        "Delete {name}?",
        name = file
    );
    let c = crate::t!("Line one\nand \"two\"");
    let d = t!("Goes \
               on");
    let e = t!(r#"Raw "quoted""#);
    let f = format!("not a text {}", 1);
    let lifetime: &'static str = "plain";
    let ch = '"';
    let g = t!("After a char that is a quote");
    let label = english!("A column");
    let k = keyed("Cut ({0})", &[&Cut]);
    let p = plural(count, "file");
    let q = plural(count, if folders { "folder" } else { "drive" });
    let r = plural(count, kind.noun());
    let s = t!(name);
}
fn plural(n: usize, word: &str) -> String { t!("from inside plural's own body") }
macro_rules! t { ($text:expr) => { $crate::i18n::translate($text) }; }
#[cfg(test)]
mod tests {
    fn x() { t!("a test's text"); }
}
#[test]
fn y() { t!("another test's text"); }
#[cfg(test)]
pub(crate) fn helper() { t!("a test helper's text") }
#[cfg(test)]
mod testing;
`;
    const found = extractFromRust(source);
    assert.deepEqual(found.texts.map((t) => t.text), [
        "Open",
        "Delete {name}?",
        'Line one\nand "two"',
        "Goes on",
        'Raw "quoted"',
        "After a char that is a quote",
        "A column",
        "Cut ({0})",
        "{n} file",
        "{n} files",
        "{n} folder",
        "{n} folders",
        "{n} drive",
        "{n} drives",
        "from inside plural's own body",
    ]);
    assert.deepEqual(found.testModules, ["testing"]);
    assert.deepEqual(found.variables.map((v) => v.call), ["plural(n, …)", "t!(…)"]);
    assert.equal(found.texts.find((t) => t.text === "Delete {name}?").line, 6, "the string's own line");
    assert.equal(found.texts.find((t) => t.text === "{n} file").note, "a count of one: {n} is 1");
}

{
    // Notes for translators: on the lines just above a text, or above a whole item.
    const source = `
fn a() {
    // For translators: a button that
    // goes on two lines
    t!("Go");
    t!("Not noted");
}
// For translators: % is the extension
static KINDS: &[&str] = &[english!("% image"), english!("% video")];
const AFTER: &str = english!("After the table");
`;
    const notes = Object.fromEntries(extractFromRust(source).texts.map((t) => [t.text, t.note]));
    assert.deepEqual(notes, {
        Go: "a button that goes on two lines",
        "Not noted": null,
        "% image": "% is the extension",
        "% video": "% is the extension",
        "After the table": null,
    });
}

{
    // bind!(keys, context, action, "what"): the shortcuts sheet.
    assert.deepEqual(texts(`bind!("ctrl-l", "Workspace", FocusAddress, "Type a path"),`), ["Type a path"]);
    // Lifetimes and labels don't start chars; byte strings aren't texts.
    assert.deepEqual(texts(`fn f<'a>(x: &'a str) { 'outer: loop { t!("ok"); break 'outer; } let b = b"t!(\\"no\\")"; }`), ["ok"]);
    // A test item's commas don't end it early; a field under cfg(test) ends at its comma.
    assert.deepEqual(texts(`#[cfg(test)]\nfn f<A, B>() where A: X, B: Y { t!("test"); }\nfn g() { t!("kept"); }`), ["kept"]);
    assert.deepEqual(texts(`struct S { #[cfg(test)] probe: u8, label: &'static str }\nfn g() { t!("kept"); }`), ["kept"]);
    assert.equal(tokenize("a\r\nb").find((t) => t.value === "b").line, 2, "Windows line ends");
}

// --------------------------------------------------------------------------------------------------- PO

{
    assert.deepEqual(placeholders("{n} of {total}, %1, % image, {not a name}"), ["{n}", "{total}", "%1", "%"]);
    assert.equal(placeholdersDiffer("Delete {name}?", "删除 {name}？"), null);
    assert.equal(placeholdersDiffer("Delete {name}?", "删除？"), "the translation lacks {name}");
    assert.equal(placeholdersDiffer("Copy to %1", "复制到 {name}"), "the translation lacks %1 and has {name}, which the English doesn't");
    assert.ok(likeness("Open in new tab", "Open in a new tab") > 0.85);
    assert.equal(likeness("Open", "Delete everything now"), 0);
}

{
    // Read, then written back as it was: gettext's layout, comments and obsolete entries kept.
    const text = `# Glint's interface in zh-CN.
msgid ""
msgstr ""
"Project-Id-Version: Glint\\n"
"Language: zh_CN\\n"

# The translator's own note.
#. a count of one: {n} is 1
#: crates/glint-ui/src/a.rs crates/glint-ui/src/b.rs
msgid "{n} file"
msgstr "{n} 个文件"

#, fuzzy
#| msgid "Open in new tab"
msgid "Open in a new tab"
msgstr "在新标签页中打开"

msgid ""
"A text long enough that gettext cuts it into several lines at its spaces, so "
"that none of them is wider than seventy-nine characters."
msgstr ""

msgid ""
"Line one\\n"
"line two"
msgstr ""

#~ msgid "Gone"
#~ msgstr "没了"
`;
    const catalog = parsePo(text);
    assert.equal(catalog.entries.length, 4);
    assert.deepEqual(catalog.entries[0].comments, ["The translator's own note."]);
    assert.deepEqual(catalog.entries[0].refs, ["crates/glint-ui/src/a.rs", "crates/glint-ui/src/b.rs"]);
    assert.deepEqual(catalog.entries[1].flags, ["fuzzy"]);
    assert.equal(catalog.entries[1].previous, "Open in new tab");
    assert.equal(catalog.entries[3].msgid, "Line one\nline two");
    assert.deepEqual(catalog.obsolete.map((e) => [e.msgid, e.msgstr]), [["Gone", "没了"]]);
    assert.equal(catalog.header, "Project-Id-Version: Glint\nLanguage: zh_CN\n");
    assert.equal(writePo(catalog), text, "written back as it was read");
    assert.throws(() => parsePo('msgid "a"\nmsgstr "b'), /line 2: expected a string in quotes/);
}

{
    // update: translations kept, a changed English kept as a fuzzy guess, a dropped text made obsolete.
    const old = parsePo(`msgid ""
msgstr "Language: zh_CN\\n"

msgid "Open"
msgstr "打开"

msgid "Open in new tab"
msgstr "在新标签页中打开"

msgid "Removed for good"
msgstr "永久删除"

msgid "Untranslated and gone"
msgstr ""
`);
    const tmpl = template([
        { msgid: "Open", refs: ["a.rs"], notes: [] },
        { msgid: "Open in a new tab", refs: ["a.rs"], notes: [] },
        { msgid: "Brand new", refs: ["b.rs"], notes: [] },
    ]);
    const merged = merge(tmpl, old, "zh-CN");
    assert.deepEqual(
        merged.entries.map((e) => [e.msgid, e.msgstr, e.flags.join(","), e.previous]),
        [
            ["Open", "打开", "", null],
            ["Open in a new tab", "在新标签页中打开", "fuzzy", "Open in new tab"],
            ["Brand new", "", "", null],
        ],
    );
    assert.deepEqual(merged.obsolete.map((e) => e.msgid), ["Open in new tab", "Removed for good"]);
    const s = catalogStatus(tmpl, parsePo(writePo(merged)));
    assert.deepEqual([s.total, s.translated, s.fuzzy, s.untranslated, s.obsolete, s.problems.length], [3, 1, 1, 1, 2, 0]);
    // A new catalog gets the language's header.
    assert.match(merge(tmpl, null, "zh-CN").header, /Language: zh_CN/);
    // Problems are found as build.rs finds them.
    const bad = parsePo('msgid "Delete {name}?"\nmsgstr "删除？"\n\nmsgctxt "verb"\nmsgid "Open"\nmsgstr "打开"\n');
    assert.deepEqual(catalogStatus(tmpl, bad).problems.map((p) => [p.line, p.why]), [
        [1, "the translation lacks {name}"],
        [5, "its msgctxt isn't a count's form (zero, two, few, many), and Glint's texts have no other context"],
    ]);
}

{
    // A language with more forms for a count (Arabic): update gives each count's "other" text its forms, as
    // entries with the form as msgctxt, keeping what was translated; status counts them and checks them.
    const tmpl = template([
        { msgid: "{n} file", refs: ["a.rs"], notes: [COUNT_ONE_NOTE] },
        { msgid: "{n} files", refs: ["a.rs"], notes: [COUNT_OTHER_NOTE] },
        { msgid: "Open", refs: ["a.rs"], notes: [] },
    ]);
    const old = parsePo(`msgid "{n} file"
msgstr "ملف واحد"

msgid "{n} files"
msgstr "{n} ملف"

msgctxt "two"
msgid "{n} files"
msgstr "ملفان"
`);
    assert.deepEqual(countForms("ar"), ["zero", "two", "few", "many"]);
    assert.deepEqual(countForms("ar-SA"), ["zero", "two", "few", "many"]);
    assert.deepEqual(countForms("de"), []);
    const merged = merge(tmpl, old, "ar");
    assert.deepEqual(
        merged.entries.map((e) => [e.context, e.msgid, e.msgstr]),
        [
            [null, "{n} file", "ملف واحد"],
            [null, "{n} files", "{n} ملف"],
            ["zero", "{n} files", ""],
            ["two", "{n} files", "ملفان"],
            ["few", "{n} files", ""],
            ["many", "{n} files", ""],
            [null, "Open", ""],
        ],
    );
    const written = writePo(merged);
    assert.match(written, /msgctxt "two"\nmsgid "\{n\} files"\nmsgstr "ملفان"/);
    const s = catalogStatus(tmpl, parsePo(written));
    assert.deepEqual([s.total, s.translated, s.forms.total, s.forms.translated, s.problems.length], [3, 2, 4, 1, 0], "one and two may leave {n} out");
    // German has no other forms: the same template gives it none.
    assert.equal(merge(tmpl, null, "de").entries.length, 3);
    // A form that needs its number, and a context that isn't a form, are problems.
    const bad = parsePo('msgctxt "many"\nmsgid "{n} files"\nmsgstr "ملفات كثيرة"\n\nmsgctxt "few"\nmsgid "Open"\nmsgstr "فتح"\n');
    assert.deepEqual(catalogStatus(tmpl, bad).problems.map((p) => [p.line, p.msgid, p.why]), [
        [2, "{n} files (many)", "the translation lacks {n}"],
        [6, "Open (few)", "a count's form, but the English has no {n} to count"],
    ]);
}

console.log("i18n: all checks passed");
