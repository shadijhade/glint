//! Media Foundation's functions, looked up when first needed instead of linked. Windows' N editions have no
//! Media Foundation until the Media Feature Pack is added, and a Glint.exe that linked mfplat.dll or
//! mfreadwrite.dll would not start there at all: Windows refuses a program whose DLLs it can't find, before
//! any of its code runs (as Windows 10 did with the I/O ring's DLL, issue #6). Looked up, each function
//! fails like any other Media Foundation error, so a video's frames, the player and a recording's waveform
//! say Windows can't play it, and everything else works.
//!
//! Same names and signatures as the windows crate's own, so a file that has
//! `use windows::Win32::Media::MediaFoundation::*;` takes these instead with
//! `use glint_core::mfapi::{MFStartup, ...};` (a named import wins over a glob). Any other function of these
//! DLLs that gets used has to come here too: the installer and MSIX scripts refuse to pack a Glint.exe that
//! imports them (scripts/lib/windows-imports.mjs).
#![allow(non_snake_case, clippy::missing_safety_doc)]

use std::ffi::c_void;
use std::sync::LazyLock;
use windows::core::{Error, Param, Result, Type, HRESULT, PCWSTR};
use windows::Win32::Media::MediaFoundation::{IMFAttributes, IMFByteStream, IMFDXGIDeviceManager, IMFMediaType, IMFSourceReader, MF_FILE_ACCESSMODE, MF_FILE_FLAGS, MF_FILE_OPENMODE};

type Raw = *mut c_void;

// The functions, as mfapi.h and mfreadwrite.h declare them.
type Startup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type Shutdown = unsafe extern "system" fn() -> HRESULT;
type CreateAttributes = unsafe extern "system" fn(*mut Raw, u32) -> HRESULT;
type CreateMediaType = unsafe extern "system" fn(*mut Raw) -> HRESULT;
type CreateFile = unsafe extern "system" fn(MF_FILE_ACCESSMODE, MF_FILE_OPENMODE, MF_FILE_FLAGS, PCWSTR, *mut Raw) -> HRESULT;
type CreateDxgiDeviceManager = unsafe extern "system" fn(*mut u32, *mut Raw) -> HRESULT;
type CreateSourceReaderFromUrl = unsafe extern "system" fn(PCWSTR, Raw, *mut Raw) -> HRESULT;
type Found = unsafe extern "system" fn() -> isize;

struct Api {
    startup: Startup,
    shutdown: Shutdown,
    create_attributes: CreateAttributes,
    create_media_type: CreateMediaType,
    create_file: CreateFile,
    create_dxgi_device_manager: CreateDxgiDeviceManager,
    create_source_reader_from_url: CreateSourceReaderFromUrl,
}

static API: LazyLock<Option<Api>> = LazyLock::new(|| unsafe {
    use std::mem::transmute;
    use windows::core::{s, w};
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    let plat = LoadLibraryExW(w!("mfplat.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
    let readwrite = LoadLibraryExW(w!("mfreadwrite.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
    Some(Api {
        startup: transmute::<Found, Startup>(GetProcAddress(plat, s!("MFStartup"))?),
        shutdown: transmute::<Found, Shutdown>(GetProcAddress(plat, s!("MFShutdown"))?),
        create_attributes: transmute::<Found, CreateAttributes>(GetProcAddress(plat, s!("MFCreateAttributes"))?),
        create_media_type: transmute::<Found, CreateMediaType>(GetProcAddress(plat, s!("MFCreateMediaType"))?),
        create_file: transmute::<Found, CreateFile>(GetProcAddress(plat, s!("MFCreateFile"))?),
        create_dxgi_device_manager: transmute::<Found, CreateDxgiDeviceManager>(GetProcAddress(plat, s!("MFCreateDXGIDeviceManager"))?),
        create_source_reader_from_url: transmute::<Found, CreateSourceReaderFromUrl>(GetProcAddress(readwrite, s!("MFCreateSourceReaderFromURL"))?),
    })
});

/// Whether this Windows has Media Foundation (an N edition without the Media Feature Pack hasn't).
pub fn available() -> bool {
    API.is_some()
}

fn api() -> Result<&'static Api> {
    API.as_ref().ok_or_else(|| {
        // ERROR_MOD_NOT_FOUND, as Windows itself would say.
        Error::new(HRESULT::from_win32(126), "Windows has no Media Foundation here (on an N edition of Windows, the Media Feature Pack adds it)")
    })
}

pub unsafe fn MFStartup(version: u32, flags: u32) -> Result<()> {
    unsafe { (api()?.startup)(version, flags).ok() }
}

pub unsafe fn MFShutdown() -> Result<()> {
    unsafe { (api()?.shutdown)().ok() }
}

pub unsafe fn MFCreateAttributes(attributes: *mut Option<IMFAttributes>, initial_size: u32) -> Result<()> {
    // Option<IMFAttributes> is one nullable interface pointer, as the windows crate's own version relies on.
    unsafe { (api()?.create_attributes)(attributes.cast(), initial_size).ok() }
}

pub unsafe fn MFCreateMediaType() -> Result<IMFMediaType> {
    let api = api()?;
    unsafe {
        let mut raw: Raw = std::ptr::null_mut();
        (api.create_media_type)(&mut raw).and_then(|| Type::from_abi(raw))
    }
}

pub unsafe fn MFCreateFile<P3>(access: MF_FILE_ACCESSMODE, open: MF_FILE_OPENMODE, flags: MF_FILE_FLAGS, url: P3) -> Result<IMFByteStream>
where
    P3: Param<PCWSTR>,
{
    let api = api()?;
    unsafe {
        let mut raw: Raw = std::ptr::null_mut();
        (api.create_file)(access, open, flags, url.param().abi(), &mut raw).and_then(|| Type::from_abi(raw))
    }
}

pub unsafe fn MFCreateDXGIDeviceManager(reset_token: *mut u32, manager: *mut Option<IMFDXGIDeviceManager>) -> Result<()> {
    unsafe { (api()?.create_dxgi_device_manager)(reset_token, manager.cast()).ok() }
}

pub unsafe fn MFCreateSourceReaderFromURL<P0, P1>(url: P0, attributes: P1) -> Result<IMFSourceReader>
where
    P0: Param<PCWSTR>,
    P1: Param<IMFAttributes>,
{
    let api = api()?;
    unsafe {
        let mut raw: Raw = std::ptr::null_mut();
        (api.create_source_reader_from_url)(url.param().abi(), attributes.param().abi(), &mut raw).and_then(|| Type::from_abi(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Media::MediaFoundation::{IMFAttributes, MFSTARTUP_LITE, MF_MT_MAJOR_TYPE, MF_VERSION, MFMediaType_Video};

    #[test]
    fn finds_media_foundation_and_makes_its_objects() {
        // Every Windows this is built and tested on has Media Foundation (it isn't an N edition).
        assert!(available(), "mfplat.dll or mfreadwrite.dll, or one of their functions, wasn't found");
        unsafe {
            MFStartup(MF_VERSION, MFSTARTUP_LITE).unwrap();
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 1).unwrap();
            assert!(attributes.is_some());
            let kind = MFCreateMediaType().unwrap();
            kind.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).unwrap();
            assert_eq!(kind.GetGUID(&MF_MT_MAJOR_TYPE).unwrap(), MFMediaType_Video);
            let missing: Vec<u16> = r"C:\nowhere\at\all.mp4".encode_utf16().chain([0]).collect();
            assert!(MFCreateSourceReaderFromURL(PCWSTR(missing.as_ptr()), None).is_err());
            MFShutdown().unwrap();
        }
    }
}
