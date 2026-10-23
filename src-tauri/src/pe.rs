//! Zero-copy Windows Portable Executable (PE32 / PE32+) inspector for `.exe`, `.dll`, `.sys`, `.ocx`,
//! `.scr`, `.cpl`, `.efi` and `.node` binaries. Parses headers, mitigation flags, sections, imports and
//! exports in sub-millisecond time without external crates or reading the entire file.

use crate::fsx::blocking;
use serde::Serialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::Instant;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeSection {
    pub name: String,
    pub virtual_size: u32,
    pub virtual_address: u32,
    pub raw_size: u32,
    pub raw_offset: u32,
    pub flags: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeImportModule {
    pub module: String,
    pub symbols: Vec<String>,
    pub total_count: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeExport {
    pub ordinal: u32,
    pub name: String,
    pub rva: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PeInfo {
    pub format: String,
    pub machine: String,
    pub subsystem: String,
    pub is_dll: bool,
    pub is_dotnet: bool,
    pub is_signed: bool,
    pub timestamp_ms: Option<i64>,
    pub linker_version: String,
    pub os_version: String,
    pub entry_point: u32,
    pub image_base: u64,
    pub checksum: u32,
    pub features: Vec<String>,
    pub sections: Vec<PeSection>,
    pub imports: Vec<PeImportModule>,
    pub export_count: usize,
    pub exports: Vec<PeExport>,
    pub elapsed_ms: f64,
}

#[tauri::command]
pub async fn inspect_pe(path: String) -> Result<PeInfo, String> {
    blocking(move || parse_pe_file(&path)).await
}

fn u16_le(buf: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes(buf.get(off..off + 2)?.try_into().ok()?))
}

fn u32_le(buf: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(off..off + 4)?.try_into().ok()?))
}

fn u64_le(buf: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(buf.get(off..off + 8)?.try_into().ok()?))
}

fn read_cstr(buf: &[u8], off: usize, max_len: usize) -> Option<String> {
    let slice = buf.get(off..)?;
    let limit = slice.len().min(max_len);
    let end = slice[..limit].iter().position(|&b| b == 0)?;
    if end == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&slice[..end]).into_owned())
}

fn rva_to_offset(rva: u32, sections: &[PeSection]) -> Option<u32> {
    if rva == 0 {
        return None;
    }
    for s in sections {
        let span = s.virtual_size.max(s.raw_size);
        if rva >= s.virtual_address && rva < s.virtual_address.saturating_add(span) {
            let delta = rva - s.virtual_address;
            if delta < s.raw_size {
                return s.raw_offset.checked_add(delta);
            }
        }
    }
    None
}

fn read_at(file: &mut File, offset: u32, buf: &mut [u8]) -> Option<usize> {
    file.seek(SeekFrom::Start(offset as u64)).ok()?;
    file.read(buf).ok()
}

pub fn parse_pe_file(path: &str) -> Result<PeInfo, String> {
    let start = Instant::now();
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut header = vec![0u8; 64 * 1024];
    let n = file.read(&mut header).map_err(|e| e.to_string())?;
    header.truncate(n);

    if header.len() < 0x40 || &header[0..2] != b"MZ" {
        return Err("Not a valid Portable Executable (MZ) file".into());
    }

    let pe_offset = u32_le(&header, 0x3c).ok_or("Corrupt DOS header")? as usize;
    if header.get(pe_offset..pe_offset + 4) != Some(b"PE\0\0") {
        return Err("Missing PE signature".into());
    }

    let coff = pe_offset + 4;
    let machine_code = u16_le(&header, coff).ok_or("Truncated COFF header")?;
    let num_sections = u16_le(&header, coff + 2).ok_or("Truncated COFF header")? as usize;
    let timestamp = u32_le(&header, coff + 4).unwrap_or(0);
    let opt_size = u16_le(&header, coff + 16).ok_or("Truncated COFF header")? as usize;
    let characteristics = u16_le(&header, coff + 18).unwrap_or(0);

    let machine = match machine_code {
        0x8664 => "x64 (AMD64)",
        0xAA64 => "ARM64",
        0x014c => "x86 (I386)",
        0x01c4 => "ARMv7",
        0x0200 => "Intel IA-64",
        0x5064 => "RISC-V 64",
        _ => "Unknown",
    }
    .to_string();

    let is_dll = (characteristics & 0x2000) != 0;
    // Reproducible builds (Windows' own binaries among them) put a hash where the build time would be,
    // which often reads as a date in the future: only a time that has passed is a real one.
    let now_s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let timestamp_ms = if timestamp > 0 && u64::from(timestamp) <= now_s {
        Some(timestamp as i64 * 1000)
    } else {
        None
    };

    let opt = coff + 20;
    if opt + opt_size > header.len() || opt_size < 24 {
        return Err("Truncated PE Optional Header".into());
    }

    let magic = u16_le(&header, opt).unwrap_or(0);
    let is_64 = match magic {
        0x20b => true,
        0x10b => false,
        _ => return Err("Unsupported PE optional header magic".into()),
    };
    let format = if is_64 { "PE32+ (64-bit)" } else { "PE32 (32-bit)" }.to_string();

    let linker_major = header.get(opt + 2).copied().unwrap_or(0);
    let linker_minor = header.get(opt + 3).copied().unwrap_or(0);
    let entry_point = u32_le(&header, opt + 16).unwrap_or(0);

    let (image_base, os_major, os_minor, subsystem_code, dll_chars, checksum, num_rva, data_dirs_off) = if is_64 {
        (
            u64_le(&header, opt + 24).unwrap_or(0),
            u16_le(&header, opt + 40).unwrap_or(0),
            u16_le(&header, opt + 42).unwrap_or(0),
            u16_le(&header, opt + 68).unwrap_or(0),
            u16_le(&header, opt + 70).unwrap_or(0),
            u32_le(&header, opt + 64).unwrap_or(0),
            u32_le(&header, opt + 108).unwrap_or(0) as usize,
            opt + 112,
        )
    } else {
        (
            u32_le(&header, opt + 28).unwrap_or(0) as u64,
            u16_le(&header, opt + 40).unwrap_or(0),
            u16_le(&header, opt + 42).unwrap_or(0),
            u16_le(&header, opt + 68).unwrap_or(0),
            u16_le(&header, opt + 70).unwrap_or(0),
            u32_le(&header, opt + 64).unwrap_or(0),
            u32_le(&header, opt + 92).unwrap_or(0) as usize,
            opt + 96,
        )
    };

    let subsystem = match subsystem_code {
        1 => "Native / Kernel Driver",
        2 => "Windows GUI",
        3 => "Windows Console (CLI)",
        5 => "OS/2 Console",
        7 => "POSIX Console",
        9 => "Windows CE",
        10 => "UEFI Application",
        11 => "UEFI Boot Service Driver",
        12 => "UEFI Runtime Driver",
        16 => "Windows Boot Application",
        _ => "Unknown",
    }
    .to_string();

    let mut features = Vec::new();
    if (characteristics & 0x0020) != 0 {
        features.push("Large Address Aware".into());
    }
    if (dll_chars & 0x0020) != 0 {
        features.push("High-Entropy ASLR".into());
    } else if (dll_chars & 0x0040) != 0 {
        features.push("ASLR".into());
    }
    if (dll_chars & 0x0100) != 0 {
        features.push("DEP / NX".into());
    }
    if (dll_chars & 0x4000) != 0 {
        features.push("Control Flow Guard (CFG)".into());
    }
    if (dll_chars & 0x0080) != 0 {
        features.push("Code Integrity Enforced".into());
    }
    if (dll_chars & 0x1000) != 0 {
        features.push("AppContainer".into());
    }

    let dir_entry = |idx: usize| -> (u32, u32) {
        if idx >= num_rva.min(16) {
            return (0, 0);
        }
        let at = data_dirs_off + idx * 8;
        (u32_le(&header, at).unwrap_or(0), u32_le(&header, at + 4).unwrap_or(0))
    };

    let (export_rva, export_size) = dir_entry(0);
    let (import_rva, _) = dir_entry(1);
    let (security_off, security_size) = dir_entry(4);
    let (clr_rva, clr_size) = dir_entry(14);

    // A signature in the file (sign.rs says whether it holds, and finds one in a catalog).
    let is_signed = security_off > 0 && security_size > 0;
    let is_dotnet = clr_rva > 0 && clr_size > 0;
    if is_dotnet {
        features.insert(0, ".NET / CLR Managed".into());
    }

    let sec_table = opt + opt_size;
    let mut sections = Vec::with_capacity(num_sections.min(96));
    for i in 0..num_sections.min(96) {
        let at = sec_table + i * 40;
        let Some(raw_name) = header.get(at..at + 8) else { break };
        let end = raw_name.iter().position(|&b| b == 0).unwrap_or(8);
        let name = String::from_utf8_lossy(&raw_name[..end]).into_owned();
        let virtual_size = u32_le(&header, at + 8).unwrap_or(0);
        let virtual_address = u32_le(&header, at + 12).unwrap_or(0);
        let raw_size = u32_le(&header, at + 16).unwrap_or(0);
        let raw_offset = u32_le(&header, at + 20).unwrap_or(0);
        let chars = u32_le(&header, at + 36).unwrap_or(0);

        let mut flags = Vec::new();
        if (chars & 0x4000_0000) != 0 {
            flags.push("R".into());
        }
        if (chars & 0x8000_0000) != 0 {
            flags.push("W".into());
        }
        if (chars & 0x2000_0000) != 0 {
            flags.push("X".into());
        }
        if (chars & 0x0000_0020) != 0 {
            flags.push("Code".into());
        }
        if (chars & 0x0000_0040) != 0 {
            flags.push("Data".into());
        }

        sections.push(PeSection {
            name,
            virtual_size,
            virtual_address,
            raw_size,
            raw_offset,
            flags,
        });
    }

    let imports = parse_imports(&mut file, import_rva, is_64, &sections);
    let (export_count, exports) = parse_exports(&mut file, export_rva, export_size, &sections);

    Ok(PeInfo {
        format,
        machine,
        subsystem,
        is_dll,
        is_dotnet,
        is_signed,
        timestamp_ms,
        linker_version: format!("{linker_major}.{linker_minor}"),
        os_version: format!("{os_major}.{os_minor}"),
        entry_point,
        image_base,
        checksum,
        features,
        sections,
        imports,
        export_count,
        exports,
        elapsed_ms: start.elapsed().as_secs_f64() * 1000.0,
    })
}

fn parse_imports(file: &mut File, import_rva: u32, is_64: bool, sections: &[PeSection]) -> Vec<PeImportModule> {
    let Some(table_off) = rva_to_offset(import_rva, sections) else {
        return Vec::new();
    };
    let mut desc_buf = vec![0u8; 20 * 128];
    let Some(read_bytes) = read_at(file, table_off, &mut desc_buf) else {
        return Vec::new();
    };
    desc_buf.truncate(read_bytes);

    let mut modules = Vec::new();
    let mut scratch = vec![0u8; 4096];

    for chunk in desc_buf.as_chunks::<20>().0 {
        let ilt_rva = u32_le(chunk, 0).unwrap_or(0);
        let name_rva = u32_le(chunk, 12).unwrap_or(0);
        let iat_rva = u32_le(chunk, 16).unwrap_or(0);
        if ilt_rva == 0 && name_rva == 0 && iat_rva == 0 {
            break;
        }
        let Some(name_off) = rva_to_offset(name_rva, sections) else { continue };
        let Some(n) = read_at(file, name_off, &mut scratch[..256]) else { continue };
        let Some(module) = read_cstr(&scratch[..n], 0, 256) else { continue };

        let thunk_rva = if ilt_rva != 0 { ilt_rva } else { iat_rva };
        let mut symbols = Vec::new();
        let mut total_count = 0usize;

        if let Some(thunk_off) = rva_to_offset(thunk_rva, sections) {
            let stride = if is_64 { 8 } else { 4 };
            if let Some(tn) = read_at(file, thunk_off, &mut scratch) {
                for entry in scratch[..tn].chunks_exact(stride) {
                    let val = if is_64 {
                        u64_le(entry, 0).unwrap_or(0)
                    } else {
                        u32_le(entry, 0).unwrap_or(0) as u64
                    };
                    if val == 0 {
                        break;
                    }
                    total_count += 1;
                    if symbols.len() >= 64 {
                        continue;
                    }
                    let by_ordinal = if is_64 { (val & (1u64 << 63)) != 0 } else { (val & (1u64 << 31)) != 0 };
                    if by_ordinal {
                        symbols.push(format!("#{}", val & 0xffff));
                    } else if let Some(hint_off) = rva_to_offset((val & 0x7fff_ffff) as u32, sections) {
                        let mut name_buf = [0u8; 128];
                        if let Some(hn) = hint_off.checked_add(2).and_then(|off| read_at(file, off, &mut name_buf)) {
                            if let Some(sym) = read_cstr(&name_buf[..hn], 0, 128) {
                                symbols.push(sym);
                            }
                        }
                    }
                }
            }
        }

        modules.push(PeImportModule {
            module,
            symbols,
            total_count,
        });
    }

    modules
}

fn parse_exports(file: &mut File, export_rva: u32, export_size: u32, sections: &[PeSection]) -> (usize, Vec<PeExport>) {
    if export_rva == 0 || export_size < 40 {
        return (0, Vec::new());
    }
    let Some(dir_off) = rva_to_offset(export_rva, sections) else {
        return (0, Vec::new());
    };
    let mut dir = [0u8; 40];
    if read_at(file, dir_off, &mut dir) != Some(40) {
        return (0, Vec::new());
    }

    let ordinal_base = u32_le(&dir, 16).unwrap_or(1);
    let num_functions = u32_le(&dir, 20).unwrap_or(0) as usize;
    let num_names = u32_le(&dir, 24).unwrap_or(0) as usize;
    let funcs_rva = u32_le(&dir, 28).unwrap_or(0);
    let names_rva = u32_le(&dir, 32).unwrap_or(0);
    let ords_rva = u32_le(&dir, 36).unwrap_or(0);

    let total = num_names.max(num_functions);
    let cap = num_names.min(400);
    if cap == 0 {
        return (total, Vec::new());
    }

    let (Some(names_off), Some(ords_off)) = (rva_to_offset(names_rva, sections), rva_to_offset(ords_rva, sections)) else {
        return (total, Vec::new());
    };
    let funcs_off = rva_to_offset(funcs_rva, sections);

    let mut name_rvas = vec![0u8; cap * 4];
    let mut ord_indices = vec![0u8; cap * 2];
    if read_at(file, names_off, &mut name_rvas).is_none() || read_at(file, ords_off, &mut ord_indices).is_none() {
        return (total, Vec::new());
    }

    let mut func_rvas = vec![0u8; num_functions.min(4096) * 4];
    let has_funcs = funcs_off.and_then(|off| read_at(file, off, &mut func_rvas)).is_some();

    let mut exports = Vec::with_capacity(cap);
    let mut name_buf = [0u8; 160];
    for i in 0..cap {
        let Some(nrva) = u32_le(&name_rvas, i * 4) else { break };
        let ord_idx = u16_le(&ord_indices, i * 2).unwrap_or(i as u16) as usize;
        let rva = if has_funcs { u32_le(&func_rvas, ord_idx * 4).unwrap_or(0) } else { 0 };
        let Some(noff) = rva_to_offset(nrva, sections) else { continue };
        let Some(nn) = read_at(file, noff, &mut name_buf) else { continue };
        let Some(name) = read_cstr(&name_buf[..nn], 0, 160) else { continue };
        exports.push(PeExport {
            ordinal: ordinal_base.wrapping_add(ord_idx as u32),
            name,
            rva,
        });
    }

    (total, exports)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(windows)]
    fn parses_system32_kernel32_dll() {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let path = format!(r"{windir}\System32\kernel32.dll");
        let info = parse_pe_file(&path).expect("should parse kernel32.dll");
        assert!(info.is_dll);
        assert!(!info.sections.is_empty());
        assert!(info.export_count > 500, "kernel32.dll should have >500 exports, got {}", info.export_count);
        assert!(info.exports.iter().any(|e| e.name == "CreateFileW"));
    }
}
