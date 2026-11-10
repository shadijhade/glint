// Whether a crate's license expression (the `license` field of its Cargo.toml: "MIT OR Apache-2.0",
// "(MIT OR Apache-2.0) AND Unicode-3.0", "Apache-2.0 WITH LLVM-exception", the old "MIT/Apache-2.0") can be
// met with the licenses deny.toml allows. It is what cargo-deny's [licenses] check does with the expression,
// without cargo-deny: `npm run licenses -- --allowed` runs it over the crates of the Windows build while
// cargo-deny isn't installed. It reads Cargo.toml's text only (no license files, no confidence threshold), so
// a crate with no `license` field is reported, not judged.

/** The licenses in deny.toml's `allow = [ ... ]` list, as written there. */
export function allowedLicenses(denyToml) {
    const list = denyToml.match(/^\s*allow\s*=\s*\[([\s\S]*?)\]/m)?.[1] ?? "";
    return new Set([...list.replace(/#[^\n]*/g, "").matchAll(/"([^"]+)"/g)].map((m) => m[1]));
}

/** Splits an expression into ids, WITH/AND/OR and parentheses. */
const tokens = (expression) => expression.replace(/\//g, " OR ").match(/\(|\)|[^\s()]+/g) ?? [];

/**
 * Whether `expression` is met by the `allowed` set. An id with an exception ("Apache-2.0 WITH LLVM-exception") counts
 * when that exact text is allowed. Returns false for an expression it can't read.
 */
export function satisfied(expression, allowed) {
    const list = tokens(String(expression ?? ""));
    let at = 0;
    const atom = () => {
        if (list[at] === "(") {
            at++;
            const value = or();
            if (list[at] === ")") at++;
            return value;
        }
        const id = (list[at++] ?? "").replace(/\+$/, "");
        let name = id;
        if (list[at] === "WITH") {
            name = `${id} WITH ${list[at + 1] ?? ""}`;
            at += 2;
        }
        return allowed.has(name);
    };
    const and = () => {
        let value = atom();
        while (list[at] === "AND") {
            at++;
            const next = atom();
            value = value && next;
        }
        return value;
    };
    const or = () => {
        let value = and();
        while (list[at] === "OR") {
            at++;
            const next = and();
            value = value || next;
        }
        return value;
    };
    if (!list.length) return false;
    const value = or();
    return at >= list.length && value;
}
