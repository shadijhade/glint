#!/usr/bin/env python3
"""How much of Glint.exe each crate is, and what removing a dependency would take out.

Usage:
    python scripts/dev/size-by-crate.py [Glint.exe] [--top N] [--group NAME=dep,dep ...] [--by-dep]
                                        [--json FILE] [--compare FILE] [--no-metadata]

Without an exe it takes target/ui/release/Glint.exe (or CARGO_TARGET_DIR's). The exe's PDB must sit beside it, as
cargo leaves it. Windows' own dbghelp.dll reads it; Python 3.10 or newer, standard library only.

How it counts: each public symbol in .text is as big as the gap to the next one, and is credited to the first crate
its mangled name names (Rust's v0 or legacy mangling; C and C++ symbols by their prefix), skipping the standard
library's crates. So a generic function counts for the crate that defines it unless that is core, alloc or std:
tokio::spawn::<a glint-ui future> is tokio's, core::ptr::drop_in_place::<a glint-ui type> is glint-ui's. The "made
for the workspace's types" line says how much of other crates' code names a workspace crate.

With cargo on PATH (unless --no-metadata) it also lists, for each direct dependency of the workspace's crates, what
removing it would take out of .text: that crate and every crate only it brings in (cargo metadata's graph for
x86_64-pc-windows-msvc). Dependencies whose names share the part before the first "-" (typst, typst-pdf,
typst-render...) are counted together unless --by-dep; --group NAME=dep,dep counts any set together. A crate present
in several versions counts only when every version goes.

Thin LTO hides private symbols and folds identical functions, so shares are a few percent off: to decide on a removal,
build with and without it and compare the files. --json saves the numbers; --compare prints the change from a saved
run. Sizes are in MB (1,000,000 bytes).
"""

import argparse
import collections
import ctypes
import json
import os
import re
import struct
import subprocess
import sys
from ctypes import wintypes as W

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# Where the module is loaded for reading: any address works, as long as the sections are read against the same one.
BASE = 0x140000000
# The standard library's crates: a symbol is credited to the first crate after these (the type it was made for).
STD = {"core", "alloc", "std", "compiler_builtins", "std_detect", "panic_unwind", "hashbrown"}
WORKSPACE = ("glint_ui", "glint_core", "glint_model", "shadui_index")
# C and C++ symbols, by prefix: the -sys crates that compile them.
C_PREFIXES = (
    ("sqlite3", "libsqlite3_sys"),
    ("ZSTD", "zstd_sys"), ("HUF_", "zstd_sys"), ("FSE_", "zstd_sys"), ("ZDICT", "zstd_sys"), ("POOL_", "zstd_sys"),
    ("HIST_", "zstd_sys"), ("ERR_getErrorString", "zstd_sys"),
    ("mi_", "libmimalloc_sys"), ("_mi_", "libmimalloc_sys"),
    ("ring_core_", "ring"), ("GFp_", "ring"), ("ecp_nistz256", "ring"), ("bn_", "ring"), ("aes_hw_", "ring"),
    ("aes_nohw_", "ring"), ("vpaes_", "ring"), ("ChaCha20", "ring"), ("chacha20_", "ring"), ("x25519_", "ring"),
    ("gcm_", "ring"), ("sha256_block", "ring"), ("sha512_block", "ring"), ("LIMBS_", "ring"), ("p256_", "ring"),
    ("p384_", "ring"), ("CRYPTO_", "ring"), ("OPENSSL_", "ring"), ("L$", "ring"),
    ("blake3_", "blake3"),
    ("?", "c++ (unrar, runtime)"),
    ("__rust", "std"), ("rust_", "std"),
)
V0_CRATE = re.compile(rb"C(?:s[0-9A-Za-z]*_)(\d+)")
IDENT = re.compile(rb"[A-Za-z_][A-Za-z0-9_]*")


def default_exe():
    target = os.environ.get("CARGO_TARGET_DIR") or os.path.join(REPO, "target", "ui")
    return os.path.join(target, "release", "Glint.exe")


def read_sections(exe):
    """The PE's sections: name -> (start, end) at BASE, and the bytes each takes in the file."""
    with open(exe, "rb") as f:
        head = f.read(16384)
    pe = struct.unpack_from("<I", head, 0x3C)[0]
    if head[pe:pe + 4] != b"PE\0\0":
        sys.exit(f"{exe} is not a Windows program")
    count = struct.unpack_from("<H", head, pe + 6)[0]
    optional = struct.unpack_from("<H", head, pe + 20)[0]
    table = pe + 24 + optional
    spans, raw = {}, {}
    for i in range(count):
        entry = head[table + 40 * i:table + 40 * (i + 1)]
        name = entry[:8].rstrip(b"\0").decode("ascii", "replace")
        virtual_size, address, raw_size, _ = struct.unpack_from("<IIII", entry, 8)
        spans[name] = (BASE + address, BASE + address + virtual_size)
        raw[name] = raw_size
    return spans, raw


class SYMBOL_INFO(ctypes.Structure):
    _fields_ = [
        ("SizeOfStruct", W.ULONG), ("TypeIndex", W.ULONG), ("Reserved", ctypes.c_uint64 * 2), ("Index", W.ULONG),
        ("Size", W.ULONG), ("ModBase", ctypes.c_uint64), ("Flags", W.ULONG), ("Value", ctypes.c_uint64),
        ("Address", ctypes.c_uint64), ("Register", W.ULONG), ("Scope", W.ULONG), ("Tag", W.ULONG),
        ("NameLen", W.ULONG), ("MaxNameLen", W.ULONG), ("Name", ctypes.c_char * 1),
    ]


def read_symbols(exe):
    """Every symbol of the exe's PDB: (address at BASE, mangled name)."""
    if sys.platform != "win32":
        sys.exit("This reads the PDB with Windows' dbghelp.dll: run it on Windows.")
    dbghelp = ctypes.WinDLL("dbghelp.dll", use_last_error=True)
    process = ctypes.c_void_p(0x676C696E)  # any value: no real process is read
    callback_type = ctypes.WINFUNCTYPE(W.BOOL, ctypes.POINTER(SYMBOL_INFO), W.ULONG, ctypes.c_void_p)
    dbghelp.SymSetOptions.argtypes = [W.DWORD]
    dbghelp.SymInitializeW.argtypes = [ctypes.c_void_p, W.LPCWSTR, W.BOOL]
    dbghelp.SymInitializeW.restype = W.BOOL
    dbghelp.SymLoadModuleExW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, W.LPCWSTR, W.LPCWSTR, ctypes.c_uint64,
                                         W.DWORD, ctypes.c_void_p, W.DWORD]
    dbghelp.SymLoadModuleExW.restype = ctypes.c_uint64
    dbghelp.SymEnumSymbols.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p, callback_type, ctypes.c_void_p]
    dbghelp.SymEnumSymbols.restype = W.BOOL
    dbghelp.SymCleanup.argtypes = [ctypes.c_void_p]
    # 0: names stay mangled (no SYMOPT_UNDNAME) and the PDB is read at once (no SYMOPT_DEFERRED_LOADS).
    dbghelp.SymSetOptions(0)
    if not dbghelp.SymInitializeW(process, os.path.dirname(os.path.abspath(exe)), False):
        sys.exit(f"dbghelp couldn't start (error {ctypes.get_last_error()})")
    symbols = []
    name_at = SYMBOL_INFO.Name.offset

    def collect(info, _size, _context):
        symbol = info.contents
        symbols.append((symbol.Address, ctypes.string_at(ctypes.addressof(symbol) + name_at, symbol.NameLen)))
        return True

    try:
        base = dbghelp.SymLoadModuleExW(process, None, os.path.abspath(exe), None, BASE, 0, None, 0)
        if not base:
            sys.exit(f"dbghelp couldn't load {exe} (error {ctypes.get_last_error()})")
        dbghelp.SymEnumSymbols(process, base, b"*", callback_type(collect), None)
    finally:
        dbghelp.SymCleanup(process)
    if len(symbols) < 1000:
        sys.exit(f"Only {len(symbols)} symbols: is {os.path.splitext(exe)[0]}.pdb beside the exe?")
    return symbols


def rust_crates(name):
    """The crates a v0-mangled name names, in order."""
    found = []
    for match in V0_CRATE.finditer(name):
        length, start = int(match.group(1)), match.end()
        ident = name[start:start + length]
        if len(ident) == length and IDENT.fullmatch(ident):
            found.append(ident.decode())
    return found


def legacy_crates(name):
    """The crates a legacy-mangled name (_ZN...E) names: its first segment, or the crates of an <impl ...> segment."""
    digits = re.match(rb"_ZN(\d+)", name)
    if not digits:
        return []
    start = digits.end()
    segment = name[start:start + int(digits.group(1))]
    inside = re.findall(rb"([a-z_][a-z0-9_]*)\.\.", segment)
    return [w.decode() for w in inside] if inside else [segment.decode("latin1")]


def is_rust(name):
    return name.startswith(b"_R") or name.startswith(b"_ZN")


def owner(name):
    if is_rust(name):
        crates = rust_crates(name) if name.startswith(b"_R") else legacy_crates(name)
        return next((c for c in crates if c not in STD), "std")
    text = name.decode("latin1")
    return next((crate for prefix, crate in C_PREFIXES if text.startswith(prefix)), "c (other)")


def credit_code(symbols, start, end):
    """Bytes of [start, end) per crate, and per workspace crate the bytes of other crates' code naming it."""
    best = {}
    for address, name in symbols:
        if start <= address < end:
            old = best.get(address)
            if old is None or (is_rust(name) and not is_rust(old)):
                best[address] = name
    addresses = sorted(best)
    sizes, for_types = collections.Counter(), collections.Counter()
    for i, address in enumerate(addresses):
        size = (addresses[i + 1] if i + 1 < len(addresses) else end) - address
        name = best[address]
        crate = owner(name)
        sizes[crate] += size
        if name.startswith(b"_R"):
            named = set(rust_crates(name))
            for workspace_crate in WORKSPACE:
                if workspace_crate in named and crate != workspace_crate:
                    for_types[workspace_crate] += size
    return sizes, for_types, len(addresses)


def dependency_groups(extra, by_dep):
    """Per group of direct dependencies: their names and the crates removing them would take out of the build."""
    command = ["cargo", "metadata", "--format-version", "1", "--locked", "--offline",
               "--filter-platform", "x86_64-pc-windows-msvc"]
    try:
        done = subprocess.run(command, cwd=REPO, capture_output=True, text=True, encoding="utf-8", check=True)
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or error
        print(f"\n(no dependency groups: cargo metadata failed: {str(detail).strip()[:300]})")
        return None
    meta = json.loads(done.stdout)
    crate_of, package_of, macro = {}, {}, set()
    for package in meta["packages"]:
        libs = [t for t in package["targets"] if set(t["kind"]) & {"lib", "rlib", "dylib", "staticlib", "cdylib"}]
        crate_of[package["id"]] = (libs[0]["name"] if libs else package["name"]).replace("-", "_")
        package_of[package["id"]] = package["name"]
        if any("proc-macro" in t["kind"] for t in package["targets"]):
            macro.add(package["id"])
    edges = {node["id"]: [dep["pkg"] for dep in node["deps"] if any(k["kind"] is None for k in dep["dep_kinds"])]
             for node in meta["resolve"]["nodes"]}
    members = set(meta["workspace_members"])
    root = next((i for i in members if package_of[i] == "glint"), None)
    if root is None:
        print("\n(no dependency groups: the workspace has no glint package)")
        return None

    def reachable(without=frozenset()):
        seen, todo = {root}, [root]
        while todo:
            at = todo.pop()
            for to in edges.get(at, ()):
                if (at in members and to in without) or to in seen:
                    continue
                seen.add(to)
                todo.append(to)
        return seen

    everything = reachable()
    direct = sorted({to for m in members & everything for to in edges.get(m, ())
                     if to not in members and to not in macro}, key=lambda p: package_of[p])
    groups = collections.OrderedDict()
    for package in direct:
        name = package_of[package]
        groups.setdefault(name if by_dep else name.split("-")[0], set()).add(package)
    for spec in extra:
        label, _, names = spec.partition("=")
        wanted = {n.strip() for n in names.split(",") if n.strip()}
        packages = {p for p in direct if package_of[p] in wanted}
        missing = wanted - {package_of[p] for p in packages}
        if missing:
            print(f"(--group {label}: not a direct dependency: {', '.join(sorted(missing))})")
        groups[label.strip()] = packages
    result = {}
    for label, packages in groups.items():
        if not packages:
            continue
        left = reachable(frozenset(packages))
        still = {crate_of[p] for p in left}
        gone = {crate_of[p] for p in everything - left if p not in macro} - still
        result[label] = {"deps": sorted({package_of[p] for p in packages}), "crates": sorted(gone)}
    return result


def mb(n):
    return n / 1e6


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("exe", nargs="?", default=default_exe())
    parser.add_argument("--top", type=int, default=30, help="rows per list (default 30)")
    parser.add_argument("--group", action="append", default=[], metavar="NAME=dep,dep",
                        help="count these direct dependencies together")
    parser.add_argument("--by-dep", action="store_true", help="one row per direct dependency, no grouping by stem")
    parser.add_argument("--json", metavar="FILE", help="save the numbers")
    parser.add_argument("--compare", metavar="FILE", help="print the change from a run saved with --json")
    parser.add_argument("--no-metadata", action="store_true", help="skip the dependency groups (no cargo)")
    args = parser.parse_args()
    if not os.path.isfile(args.exe):
        sys.exit(f"No {args.exe}: build it (cargo build --release -p glint) or name an exe.")

    spans, raw = read_sections(args.exe)
    symbols = read_symbols(args.exe)
    start, end = spans[".text"]
    sizes, for_types, counted = credit_code(symbols, start, end)
    total = sum(sizes.values())
    file_size = os.path.getsize(args.exe)

    print(f"{args.exe}: {file_size:,} bytes ({mb(file_size):.1f} MB)")
    print("Sections (MB in the file): " + "  ".join(f"{n} {mb(s):.1f}" for n, s in raw.items()))
    print(f"Code credited to crates: {mb(total):.1f} MB, {counted:,} symbols, {len(sizes)} crates")
    print("Other crates' code made for the workspace's types: "
          + ", ".join(f"{c} {mb(for_types[c]):.1f} MB" for c in WORKSPACE if for_types[c]))
    print(f"\nBiggest crates in .text (MB, share):")
    for crate, size in sizes.most_common(args.top):
        print(f"  {mb(size):7.2f}  {100 * size / total:4.1f}%  {crate}")

    groups = None if args.no_metadata else dependency_groups(args.group, args.by_dep)
    if groups:
        for group in groups.values():
            group["bytes"] = sum(sizes.get(c, 0) for c in group["crates"])
        print("\nWhat removing a direct dependency would take out of .text "
              "(MB; it and every crate only it brings in; estimates):")
        ranked = sorted(groups.items(), key=lambda kv: -kv[1]["bytes"])
        for label, group in ranked[:args.top]:
            deps = ", ".join(group["deps"])
            print(f"  {mb(group['bytes']):7.2f}  {label:<22} {len(group['crates']):3} crates  ({deps})")

    saved = {"exe": os.path.abspath(args.exe), "bytes": file_size, "sections": raw, "code": total,
             "crates": dict(sizes), "for_types": dict(for_types), "groups": groups or {}}
    if args.compare:
        compare(saved, args.compare, args.top)
    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(saved, f, indent=1, sort_keys=True)
        print(f"\nSaved to {args.json}")


def compare(now, path, top):
    with open(path, encoding="utf-8") as f:
        before = json.load(f)

    def change(a, b):
        return f"{mb(a):.2f} -> {mb(b):.2f} MB ({mb(b - a):+.2f})"

    print(f"\nCompared with {path}:")
    print(f"  the exe: {change(before['bytes'], now['bytes'])}")
    for name in sorted(set(before["sections"]) | set(now["sections"])):
        a, b = before["sections"].get(name, 0), now["sections"].get(name, 0)
        if a != b:
            print(f"  {name}: {change(a, b)}")
    crates = set(before["crates"]) | set(now["crates"])
    moved = sorted(crates, key=lambda c: -abs(now["crates"].get(c, 0) - before["crates"].get(c, 0)))
    print("  crates that changed most:")
    for crate in moved[:top]:
        a, b = before["crates"].get(crate, 0), now["crates"].get(crate, 0)
        if a == b:
            break
        print(f"    {crate:<28} {change(a, b)}{'  (gone)' if not b else '  (new)' if not a else ''}")


if __name__ == "__main__":
    main()
