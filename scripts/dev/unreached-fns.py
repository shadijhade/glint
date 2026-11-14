#!/usr/bin/env python3
"""Functions the shipping app never reaches, found by name.

Usage:
    python scripts/dev/unreached-fns.py [--crate NAME ...] [--min-lines N]

For each crate (glint-core, glint-model and glint-ui unless --crate names others; shadui-index can be asked for), it
reads every .rs file, blanks comments, strings and character literals, and sets test code aside: files named
tests.rs, *_tests.rs, tests_*.rs, fake*.rs or testing.rs, files under a tests/ folder, modules declared under
#[cfg(test)], and #[cfg(test)] items. A function is live when its name appears in the code that uses the crate
(crates/glint/src for glint-ui; glint-ui and glint for glint-core and glint-model; glint-core, glint-ui and the
service's main.rs for shadui-index), anywhere in the crate outside function bodies (statics, macros, use lines), in
an attribute that names functions (serde's with, default, serialize_with...), or in the body of a live function;
every method of a trait impl is live. What is left is printed per file, biggest first, with their line and length;
"~" marks a function that isn't pub, and "tests" one that tests (or other unreached functions) still call.

It matches by name only, so a dead function whose name is common (start, new, render) looks alive. The second list
covers free pub functions for that: those never named with their module outside it (content::start,
use content::{start}, or through `use ... as` aliases) and not used inside it or below it (where a variable shares
the name, only calls count). Check every hit with rg before deleting anything.

Python 3.10 or newer, standard library only.
"""

import argparse
import collections
import os
import re
import sys

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
USERS = {
    "glint-core": ["crates/glint-ui/src", "crates/glint/src"],
    "glint-model": ["crates/glint-ui/src", "crates/glint/src"],
    "glint-ui": ["crates/glint/src"],
    "shadui-index": ["crates/glint-core/src", "crates/glint-ui/src", "crates/shadui-index/src/main.rs"],
}
DEFAULT_CRATES = ["glint-core", "glint-model", "glint-ui"]
WORD = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
FN = re.compile(r"(?m)^[ \t]*(pub(?:\([^)]*\))?\s+)?(?:(?:const|async|unsafe|extern)\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)")
BLOCK = re.compile(r"(?m)^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?(impl|trait|mod)\b(?:\s+([A-Za-z_]\w*))?")
# The right side is a lookahead so `a::b::c` gives both (a, b) and (b, c).
PATH = re.compile(r"\b([A-Za-z_]\w*)\s*::\s*(?=([A-Za-z_]\w*|\*|\{))")
ALIAS = re.compile(r"\b([A-Za-z_]\w*)\s+as\s+([A-Za-z_]\w*)\b")
# Attribute keys whose string names a function or a module of them: #[serde(with = "b64", default = "default_port")].
NAMING_KEY = re.compile(r'\b(?:with|serialize_with|deserialize_with|default|skip_serializing_if|getter|from|try_from|into)'
                        r'\s*=\s*"([^"]*)"')
CFG_TEST = re.compile(r"#\[(?:cfg\(test\)|test)\]")
ITEM = re.compile(r"\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\([^)]*\))?\s+)?(?:(?:unsafe|async|const|extern)\s+)*"
                  r"(mod|impl|fn|struct|enum|trait|const|static|type|use|macro_rules!)\b")
TEST_MOD = re.compile(r"#\[cfg\(test\)\]\s*(?:#\[path\s*=\s*\"([^\"]+)\"\]\s*)?(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;")


def blank(text):
    return re.sub(r"[^\n]", " ", text)


def blank_code(text):
    """The text with comments, strings and character literals made spaces (lines and offsets kept)."""
    out, n, i = list(text), len(text), 0

    def wipe(a, b):
        for k in range(a, min(b, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            j = text.find("\n", i)
            j = n if j < 0 else j
            wipe(i, j)
            i = j
        elif text.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if text.startswith("/*", j):
                    depth, j = depth + 1, j + 2
                elif text.startswith("*/", j):
                    depth, j = depth - 1, j + 2
                else:
                    j += 1
            wipe(i, j)
            i = j
        elif c == "r" and re.match(r'r#*"', text[i:i + 300]) and not (i and (text[i - 1].isalnum() or text[i - 1] == "_")
                                                                       and not (text[i - 1] == "b" and (i < 2 or not text[i - 2].isalnum()))):
            hashes = len(re.match(r"r(#*)", text[i:]).group(1))
            close = text.find('"' + "#" * hashes, i + 2 + hashes)
            j = n if close < 0 else close + 1 + hashes
            wipe(i, j)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and text[j] != '"':
                j += 2 if text[j] == "\\" else 1
            wipe(i, j + 1)
            i = j + 1
        elif c == "'":
            if text.startswith("'\\", i):
                j = text.find("'", i + 2)
                j = n if j < 0 else j
                wipe(i, j + 1)
                i = j + 1
            elif i + 2 < n and text[i + 2] == "'":
                wipe(i, i + 3)
                i += 3
            else:
                i += 1  # a lifetime
        else:
            i += 1
    return "".join(out)


def match_brace(code, at):
    """The offset just after the brace that closes the one at `at`."""
    depth = 0
    for j in range(at, len(code)):
        if code[j] == "{":
            depth += 1
        elif code[j] == "}":
            depth -= 1
            if depth == 0:
                return j + 1
    return len(code)


def body_start(code, at):
    """The offset of the `{` that opens the item whose header starts at `at`, or None when a `;` ends it first."""
    depth = 0
    for j in range(at, len(code)):
        c = code[j]
        if c in "([":
            depth += 1
        elif c in ")]":
            depth -= 1
        elif depth <= 0 and c == ";":
            return None
        elif depth <= 0 and c == "{":
            return j
    return None


def without_test_items(code):
    """`code` with every #[cfg(test)] item blanked."""
    at = 0
    while True:
        attribute = CFG_TEST.search(code, at)
        if not attribute:
            return code
        item = ITEM.match(code, attribute.end())
        if not item:
            at = attribute.end()
            continue
        if item.group(1) == "use":
            end = code.find(";", item.end()) + 1 or len(code)
        else:
            brace = body_start(code, item.end())
            end = (code.find(";", item.end()) + 1 or len(code)) if brace is None else match_brace(code, brace)
        code = code[:attribute.start()] + blank(code[attribute.start():end]) + code[end:]
        at = end


def read_sources():
    sources = {}
    for folder, _, names in os.walk(os.path.join(REPO, "crates")):
        for name in names:
            if name.endswith(".rs"):
                path = os.path.join(folder, name)
                rel = os.path.relpath(path, REPO).replace(os.sep, "/")
                with open(path, encoding="utf-8", errors="replace") as f:
                    sources[rel] = f.read()
    return sources


def test_files(sources):
    """Files that are test code by their name, by a tests/ folder, or by a #[cfg(test)] mod declaring them."""
    found = set()
    for rel in sources:
        base = rel.rsplit("/", 1)[-1]
        if (base in ("tests.rs", "testing.rs") or base.endswith("_tests.rs") or base.startswith("tests_")
                or base.startswith("fake") or "/tests/" in rel or "/target/" in rel):
            found.add(rel)
    for rel, text in sources.items():
        folder, base = rel.rsplit("/", 1)
        stem = base[:-3]
        for declared in TEST_MOD.finditer(text):
            path, name = declared.group(1), declared.group(2)
            if path:
                candidates = [f"{folder}/{path}"]
            else:
                here = folder if stem in ("mod", "lib", "main") else f"{folder}/{stem}"
                candidates = [f"{here}/{name}.rs", f"{here}/{name}/"]
            for other in sources:
                if any(other == c or (c.endswith("/") and other.startswith(c)) for c in candidates):
                    found.add(other)
    return found


def module_names(rel, crate_ident):
    """The names a file's items are reached through: its module's own name (or the crate's, for lib.rs)."""
    folder, base = rel.rsplit("/", 1)
    stem = base[:-3]
    if stem in ("lib", "main") and folder.endswith("/src"):
        return [crate_ident, "crate"]
    return [folder.rsplit("/", 1)[-1]] if stem == "mod" else [stem]


def child_prefix(rel):
    folder, base = rel.rsplit("/", 1)
    stem = base[:-3]
    return folder + "/" if stem in ("mod", "lib", "main") else f"{folder}/{stem}/"


def functions(code):
    """Outermost functions with a body: (start, end, name, is_pub, in_trait_impl, in_impl_or_trait, inline_module)."""
    blocks = []
    for block in BLOCK.finditer(code):
        brace = body_start(code, block.end())
        if brace is not None:
            kind, header = block.group(1), code[block.end():brace]
            trait_impl = kind == "impl" and re.search(r"\bfor\b(?!\s*<)", header) is not None
            blocks.append((block.start(), match_brace(code, brace), kind, trait_impl, block.group(2)))
    found, last_end = [], -1
    for match in FN.finditer(code):
        if match.start() < last_end:
            continue  # nested in a function found already
        brace = body_start(code, match.end())
        if brace is None:
            continue  # a declaration (in a trait, or extern)
        end = match_brace(code, brace)
        inside = [b for b in blocks if b[0] <= match.start() < b[1]]
        modules = [b for b in inside if b[2] == "mod"]
        found.append((match.start(), end, match.group(2), bool(match.group(1)), any(b[3] for b in inside),
                      any(b[2] != "mod" for b in inside), modules[-1][4] if modules else None))
        last_end = end
    return found


def attribute_words(original, code):
    """The functions and modules attributes name in strings (#[serde(with = "b64", default = "default_port")])."""
    words = set()
    for opening in re.finditer(r"#!?\[", code):
        depth, end = 0, len(code)
        for j in range(opening.end() - 1, len(code)):
            depth += {"[": 1, "]": -1}.get(code[j], 0)
            if depth == 0:
                end = j + 1
                break
        for value in NAMING_KEY.findall(original[opening.start():end]):
            words.update(WORD.findall(value))
    return words


def paths_used(code):
    """What a text names through paths: (module, name) pairs, modules globbed (m::*), and aliases (m as a)."""
    pairs, globs = set(), set()
    for match in PATH.finditer(code):
        left, right = match.group(1), match.group(2)
        if right == "*":
            globs.add(left)
        elif right == "{":
            group = code[match.end():match_brace(code, match.end())]
            pairs.update((left, word) for word in WORD.findall(group))
            # `use glint_model::i18n::{self as model, ...}` names the module `model`.
            for alias in re.findall(r"\bself\s+as\s+([A-Za-z_]\w*)", group):
                pairs.add(("self as", left + " " + alias))
        else:
            pairs.add((left, right))
    aliases = collections.defaultdict(set)
    for match in ALIAS.finditer(code):
        aliases[match.group(1)].add(match.group(2))
    for kind, both in [p for p in pairs if p[0] == "self as"]:
        module, alias = both.split()
        aliases[module].add(alias)
    return pairs, globs, aliases


def mentioned(text, name):
    """Whether `text` uses the function `name`: any mention, or only calls where a variable or field shares it."""
    if re.search(rf"(?<![.\w]){name}\s*(?:::\s*<[^;{{}}]*?>\s*)?\(", text):
        return True
    shadowed = (rf"\blet\s+(?:mut\s+)?{name}\b|\bfor\s+{name}\b|\|\s*(?:mut\s+)?{name}\s*[|,:]|,\s*(?:mut\s+)?{name}\s*\|"
                rf"|\b(?:Some|Ok|Err)\(\s*{name}\s*\)|\b{name}\s*:(?!:)")
    return not re.search(shadowed, text) and re.search(rf"\b{name}\b", text) is not None


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--crate", action="append", choices=sorted(USERS), help="crates to look at (repeatable)")
    parser.add_argument("--min-lines", type=int, default=0, help="leave out functions shorter than this")
    args = parser.parse_args()
    sources = read_sources()
    tests = test_files(sources)
    shipping = {rel: ("" if rel in tests else without_test_items(blank_code(text))) for rel, text in sources.items()}
    named = set()
    for rel, code in shipping.items():
        if code:
            named |= attribute_words(sources[rel], code)
    everywhere = "\n".join(shipping.values())
    paths = paths_used(everywhere)
    # Words of test code: test files whole, and what is left of the others once their shipping code is taken out.
    tested = set()
    for rel, text in sources.items():
        code = blank_code(text)
        kept = shipping[rel] or " " * len(code)
        tested.update(WORD.findall("".join(c if k == " " else " " for c, k in zip(code, kept))))
    for crate in args.crate or DEFAULT_CRATES:
        report(crate, sources, shipping, named, paths, tested, args.min_lines)


def report(crate, sources, shipping, named, paths, tested, min_lines):
    prefix = f"crates/{crate}/src/"
    ident = crate.replace("-", "_")
    files = {rel: code for rel, code in shipping.items() if rel.startswith(prefix) and code.strip()}
    defs, outside = [], []
    for rel, code in files.items():
        kept, previous = [], 0
        for start, end, name, is_pub, trait_impl, in_block, module in functions(code):
            kept.append(code[previous:start])
            previous = end
            defs.append((rel, start, end, name, is_pub, trait_impl, in_block, module))
        kept.append(code[previous:])
        outside.append("".join(kept))

    live = {"main"} | named
    for root in USERS[crate]:
        for rel, code in shipping.items():
            if rel == root or rel.startswith(root.rstrip("/") + "/"):
                live.update(WORD.findall(code))
    for text in outside:
        live.update(WORD.findall(text))
    bodies = collections.defaultdict(list)
    for rel, start, end, name, _, trait_impl, _, inline in defs:
        bodies[name].append(set(WORD.findall(shipping[rel][start:end])) - {name})
        # A trait's methods are called through it; a module an attribute names (serde's `with`) is used whole.
        if trait_impl or ({inline} if inline else set(module_names(rel, ident))) & named:
            live.add(name)
    todo = [n for n in live if n in bodies]
    done = set(todo)
    while todo:
        for words in bodies[todo.pop()]:
            for word in words:
                live.add(word)
                if word in bodies and word not in done:
                    done.add(word)
                    todo.append(word)

    lines = lambda rel, start, end: sources[rel][start:end].count("\n") + 1  # noqa: E731
    line_of = lambda rel, at: sources[rel].count("\n", 0, at) + 1  # noqa: E731
    # Names other unreached functions use: those stay for whatever keeps the callers (tests, say).
    dead_names = {d[3] for d in defs if d[3] not in live}
    used_by_dead = set()
    for rel, start, end, name, *_ in defs:
        if name in dead_names:
            used_by_dead.update(set(WORD.findall(shipping[rel][start:end])) - {name})
    unreached = collections.defaultdict(list)
    for rel, start, end, name, is_pub, _, _, _ in defs:
        if name not in live and lines(rel, start, end) >= min_lines:
            kept_by = name in tested or name in used_by_dead
            unreached[rel].append((lines(rel, start, end), name, is_pub, line_of(rel, start), kept_by))
    total = sum(f[0] for found in unreached.values() for f in found)
    count = sum(len(found) for found in unreached.values())
    print(f"=== {crate}: {len(defs)} functions, {count} never reached by name (about {total} lines)")
    for rel, found in sorted(unreached.items(), key=lambda kv: -sum(f[0] for f in kv[1])):
        names = ", ".join(f"{'' if p else '~'}{name}:{at}({n}{', tests' if t else ''})"
                          for n, name, p, at, t in sorted(found, reverse=True))
        print(f"  {rel[len(prefix):]} [{sum(f[0] for f in found)}]: {names}")

    # Free pub functions never named with their module outside it, nor used inside it or below it.
    pairs, globs, aliases = paths
    exported = set()
    for code in files.values():
        for statement in re.findall(r"\bpub(?:\([^)]*\))?\s+use\b[^;]*;", code):
            exported.update(WORD.findall(statement))
    suspects = []
    for rel, start, end, name, is_pub, _, in_block, inline in defs:
        if not is_pub or in_block or name == "main" or name not in live or name in exported or name in named:
            continue  # unreached ones are listed above already; re-exported or attribute-named ones are used
        code = files[rel]
        if mentioned(code[:start] + code[end:], name):
            continue
        below = child_prefix(rel)
        if not inline and any(other != rel and other.startswith(below) and mentioned(text, name)
                              for other, text in files.items()):
            continue
        modules = {inline} if inline else set(module_names(rel, ident))
        modules |= {alias for module in list(modules) for alias in aliases.get(module, ())}
        if modules & (globs | named) or any((module, name) in pairs for module in modules):
            continue
        suspects.append((rel, name, lines(rel, start, end)))
    if suspects:
        print(f"  never named with its module ({len(suspects)}; likely unreached, check each):")
        for rel, name, n in sorted(suspects):
            print(f"    {rel[len(prefix):]}: {name}({n})")
    print()


if __name__ == "__main__":
    sys.exit(main())
