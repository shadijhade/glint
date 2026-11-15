// Reads what a Windows program (a PE32+ .exe or .dll) imports: each DLL and the functions taken from it,
// both the ones Windows loads before the program starts and the delay-loaded ones. Node's own modules only.

import { readFileSync } from "node:fs";

/** @returns {{ dll: string, delayed: boolean, functions: string[] }[]} */
export function peImports(file) {
    const b = readFileSync(file);
    if (b.readUInt16LE(0) !== 0x5a4d) throw new Error(`${file} isn't a Windows program (no MZ header)`);
    const pe = b.readUInt32LE(0x3c);
    if (b.readUInt32LE(pe) !== 0x4550) throw new Error(`${file} has no PE header`);
    const sections = b.readUInt16LE(pe + 6);
    const optSize = b.readUInt16LE(pe + 20);
    const opt = pe + 24;
    const magic = b.readUInt16LE(opt);
    if (magic !== 0x20b) throw new Error(`${file} isn't a 64-bit program (optional header magic ${magic.toString(16)})`);
    const dir = (i) => ({ rva: b.readUInt32LE(opt + 112 + i * 8), size: b.readUInt32LE(opt + 112 + i * 8 + 4) });
    const table = opt + optSize;
    const at = (rva) => {
        for (let s = 0; s < sections; s++) {
            const h = table + s * 40;
            const va = b.readUInt32LE(h + 12);
            const size = Math.max(b.readUInt32LE(h + 8), b.readUInt32LE(h + 16));
            if (rva >= va && rva < va + size) return rva - va + b.readUInt32LE(h + 20);
        }
        throw new Error(`${file}: address ${rva.toString(16)} is in no section`);
    };
    const cstr = (rva) => {
        const start = at(rva);
        return b.toString("latin1", start, b.indexOf(0, start));
    };
    const names = (thunkRva) => {
        const out = [];
        for (let p = at(thunkRva); ; p += 8) {
            const lo = b.readUInt32LE(p);
            const hi = b.readUInt32LE(p + 4);
            if (lo === 0 && hi === 0) break;
            out.push(hi & 0x80000000 ? `#${lo & 0xffff}` : cstr(lo + 2));
        }
        return out;
    };
    const out = [];
    const imports = dir(1);
    if (imports.rva) {
        for (let p = at(imports.rva); ; p += 20) {
            const lookup = b.readUInt32LE(p);
            const name = b.readUInt32LE(p + 12);
            const iat = b.readUInt32LE(p + 16);
            if (!name) break;
            out.push({ dll: cstr(name), delayed: false, functions: names(lookup || iat) });
        }
    }
    const delay = dir(13);
    if (delay.rva) {
        for (let p = at(delay.rva); ; p += 32) {
            const name = b.readUInt32LE(p + 4);
            const nameTable = b.readUInt32LE(p + 16);
            if (!name) break;
            out.push({ dll: cstr(name), delayed: true, functions: names(nameTable) });
        }
    }
    return out;
}
