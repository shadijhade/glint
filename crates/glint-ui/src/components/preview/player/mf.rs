//! The real engine: Media Foundation's media engine (`IMFMediaEngine`) in frame-server mode, on a thread of its
//! own (previews-native.md section 10, approach A).
//!
//! **Why a thread.** Media Foundation wants the multi-threaded apartment, its callbacks arrive on its own
//! threads, and `TransferVideoFrame` waits for the GPU. So the engine and every COM object around it live on one
//! thread started here; the view only holds a [`MfEngine`], which sends it messages and reads what it leaves in
//! a slot. Dropping the handle tells the thread to stop, and it pauses and shuts the engine down first, so a
//! closed preview never goes on playing.
//!
//! **Pictures.** The engine decodes with the GPU's video hardware where Windows has it. Each time it says a new
//! frame is ready (`OnVideoStreamTick`), the frame is scaled by `TransferVideoFrame` into a Direct3D 11 texture
//! the size it is drawn at, copied to a staging texture, read back and wrapped in a `RenderImage`: what is read
//! back grows with the preview, not with the video. Only the newest picture is kept; the view takes it when it
//! has drawn the last.
//!
//! **Sound alone** (a recording) creates the engine as audio-only: no Direct3D device, no frames.
//!
//! The events the engine raises are turned into this module's [`Event`]s, and the time is read about twenty
//! times a second while it plays. Nothing here is called from a GPUI callback with a GPUI window in hand.

use super::engine::{error_text, file_url, frame_size, Command, Engine, Event, Facts, Frame, Kind, Opened};
use gpui_kit::RenderImage;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{implement, Interface, BOOL, BSTR, PCWSTR};
use windows::Win32::Foundation::{HMODULE, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIAdapter;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// How often the time is told to the view while playing.
const TIME_EVERY: Duration = Duration::from_millis(50);
/// After a seek, a resize or the first frame, the engine is asked for a fresh picture for this long.
const SETTLE: Duration = Duration::from_millis(1500);

/// What the engine's thread is sent: a command from the view, or a callback from Media Foundation.
enum Msg {
    Command(Command),
    /// `IMFMediaEngineNotify::EventNotify`: the event, and its first parameter (the error code of an error).
    Notify(u32, usize),
    Close,
}

/// The newest picture, and whether the view has been told about it.
#[derive(Default)]
struct Slot {
    frame: Mutex<Option<Frame>>,
}

/// The handle the view holds. Dropping it stops the thread, and with it the sound.
pub struct MfEngine {
    commands: Sender<Msg>,
    slot: Arc<Slot>,
}

impl Engine for MfEngine {
    fn send(&self, command: Command) {
        let _ = self.commands.send(Msg::Command(command));
    }

    fn take_frame(&self) -> Option<Frame> {
        self.slot.frame.lock().ok().and_then(|mut frame| frame.take())
    }
}

impl Drop for MfEngine {
    fn drop(&mut self) {
        let _ = self.commands.send(Msg::Close);
    }
}

/// Opens `path` on a thread of its own. The result is at once; whether Windows can play the file comes as an
/// event ([`Event::Loaded`] or [`Event::Failed`]).
pub fn open(path: &str, kind: Kind) -> Opened {
    let (commands, inbox) = mpsc::channel();
    let (events, heard) = async_channel::unbounded();
    let slot = Arc::new(Slot::default());
    let link = Link {
        events: events.clone(),
        slot: slot.clone(),
    };
    let (path, callbacks) = (path.to_string(), commands.clone());
    let started = std::thread::Builder::new()
        .name("glint-player".into())
        .spawn(move || run(&path, kind, inbox, callbacks, link));
    if started.is_err() {
        let _ = events.try_send(Event::Failed("Glint couldn't start the player".into()));
    }
    Opened {
        engine: Box::new(MfEngine { commands, slot }),
        events: heard,
    }
}

/// Where the thread puts what it has to say.
struct Link {
    events: async_channel::Sender<Event>,
    slot: Arc<Slot>,
}

impl Link {
    fn emit(&self, event: Event) {
        let _ = self.events.try_send(event);
    }

    /// Leaves a picture for the view, and says so unless it hasn't taken the last one yet.
    fn show(&self, frame: Frame) {
        let fresh = match self.slot.frame.lock() {
            Ok(mut slot) => slot.replace(frame).is_none(),
            Err(_) => false,
        };
        if fresh {
            self.emit(Event::Frame);
        }
    }
}

/// The thread: apartment, Media Foundation, the session, and all of it taken down again.
fn run(path: &str, kind: Kind, inbox: Receiver<Msg>, callbacks: Sender<Msg>, link: Link) {
    // SAFETY: COM and Media Foundation are started and stopped on this thread alone, and every object made in
    // between is dropped (the session's `Drop` shuts the engine down) before they are stopped.
    unsafe {
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        match MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            Ok(()) => {
                match Session::open(path, kind, callbacks, &link) {
                    Ok(mut session) => session.run(&inbox),
                    Err(why) => link.emit(Event::Failed(why)),
                }
                let _ = MFShutdown();
            }
            Err(e) => link.emit(Event::Failed(format!("Media Foundation didn't start: {}", e.message()))),
        }
        if com {
            CoUninitialize();
        }
    }
}

// ---- Direct3D --------------------------------------------------------------------------------------------

/// `ID3D10Multithread`, which the media engine needs the device to have switched on (it uses the device from its
/// own threads while this one reads frames back). The `windows` features this crate has don't include
/// Direct3D 10, so the four methods are declared here (only their order matters to COM, so the names are Rust's).
#[windows::core::interface("9b7e4e00-342c-4106-a19f-4f2704f689f0")]
unsafe trait ID3D10Multithread: windows::core::IUnknown {
    fn enter(&self);
    fn leave(&self);
    fn set_multithread_protected(&self, protect: BOOL) -> BOOL;
    fn get_multithread_protected(&self) -> BOOL;
}

/// The textures a frame is scaled into and read back from, at one size.
struct Target {
    size: (u32, u32),
    /// The engine draws into this (a render target the GPU owns).
    texture: ID3D11Texture2D,
    /// And it is copied here to be read by the CPU.
    staging: ID3D11Texture2D,
}

/// The device the engine decodes on, and the textures for frames.
struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    manager: IMFDXGIDeviceManager,
    target: Option<Target>,
}

impl Gpu {
    /// A device (the GPU's, else the software one), shared with the engine through a DXGI device manager.
    ///
    /// # Safety
    /// Media Foundation is started on this thread.
    unsafe fn new() -> Result<Gpu, String> {
        let failed = |what: &'static str| move |e: windows::core::Error| format!("{what}: {}", e.message());
        let (mut device, mut context) = (None, None);
        let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
        let mut made = Err("no device".to_string());
        for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
            made = make_device(driver, flags, &mut device, &mut context);
            if made.is_ok() {
                break;
            }
        }
        made?;
        let (device, context) = device.zip(context).ok_or("Direct3D gave no device")?;
        // The engine uses the device from its own threads.
        let multithread: ID3D10Multithread = device.cast().map_err(failed("Direct3D"))?;
        let _ = multithread.set_multithread_protected(BOOL(1));
        let (mut token, mut manager) = (0u32, None);
        MFCreateDXGIDeviceManager(&mut token, &mut manager).map_err(failed("Media Foundation"))?;
        let manager = manager.ok_or("Media Foundation gave no device manager")?;
        manager
            .ResetDevice(&device, token)
            .map_err(failed("Media Foundation"))?;
        Ok(Gpu {
            device,
            context,
            manager,
            target: None,
        })
    }

    /// Textures of `size`, unless they are that size already.
    ///
    /// # Safety
    /// The device is the one made in [`Gpu::new`].
    unsafe fn retarget(&mut self, size: (u32, u32)) -> bool {
        if self.target.as_ref().is_some_and(|target| target.size == size) {
            return true;
        }
        self.target = None;
        let mut description = D3D11_TEXTURE2D_DESC {
            Width: size.0,
            Height: size.1,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        if self
            .device
            .CreateTexture2D(&description, None, Some(&mut texture))
            .is_err()
        {
            return false;
        }
        description.Usage = D3D11_USAGE_STAGING;
        description.BindFlags = 0;
        description.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        if self
            .device
            .CreateTexture2D(&description, None, Some(&mut staging))
            .is_err()
        {
            return false;
        }
        if let (Some(texture), Some(staging)) = (texture, staging) {
            self.target = Some(Target { size, texture, staging });
            true
        } else {
            false
        }
    }
}

/// `D3D11CreateDevice` for one kind of driver.
///
/// # Safety
/// Plain FFI; the out parameters are the callers'.
unsafe fn make_device(
    driver: D3D_DRIVER_TYPE,
    flags: windows::Win32::Graphics::Direct3D11::D3D11_CREATE_DEVICE_FLAG,
    device: &mut Option<ID3D11Device>,
    context: &mut Option<ID3D11DeviceContext>,
) -> Result<(), String> {
    D3D11CreateDevice(
        None::<&IDXGIAdapter>,
        driver,
        HMODULE::default(),
        flags,
        None,
        D3D11_SDK_VERSION,
        Some(device),
        None,
        Some(context),
    )
    .map_err(|e| e.message())
}

// ---- Media Foundation's callback -------------------------------------------------------------------------

/// Where Media Foundation tells the engine's events: on its own threads, so each is only posted to ours.
#[implement(IMFMediaEngineNotify)]
struct Notify {
    inbox: Mutex<Sender<Msg>>,
}

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, _param2: u32) -> windows::core::Result<()> {
        if let Ok(inbox) = self.inbox.lock() {
            let _ = inbox.send(Msg::Notify(event, param1));
        }
        Ok(())
    }
}

// ---- The session -----------------------------------------------------------------------------------------

/// One file being played.
struct Session<'a> {
    engine: IMFMediaEngine,
    link: &'a Link,
    gpu: Option<Gpu>,
    video: bool,
    /// The video's size in pixels, once known.
    native: (u32, u32),
    /// The box a picture is wanted in, in device pixels.
    want: (u32, u32),
    playing: bool,
    stop_at: Option<f64>,
    last_time: Instant,
    /// Until when fresh pictures are looked for even though nothing plays.
    settle_until: Option<Instant>,
    closing: bool,
}

impl<'a> Session<'a> {
    /// Creates the engine on `path` and starts it opening.
    ///
    /// # Safety
    /// Media Foundation is started on this thread.
    unsafe fn open(path: &str, kind: Kind, callbacks: Sender<Msg>, link: &'a Link) -> Result<Session<'a>, String> {
        let failed = |what: &'static str| move |e: windows::core::Error| format!("{what}: {}", e.message());
        let video = kind == Kind::Video;
        let gpu = if video { Gpu::new().ok() } else { None };
        let notify: IMFMediaEngineNotify = Notify {
            inbox: Mutex::new(callbacks),
        }
        .into();
        let mut attributes = None;
        MFCreateAttributes(&mut attributes, 3).map_err(failed("Media Foundation"))?;
        let attributes = attributes.ok_or("Media Foundation gave no attributes")?;
        attributes
            .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)
            .map_err(failed("Media Foundation"))?;
        if let Some(gpu) = &gpu {
            attributes
                .SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, &gpu.manager)
                .map_err(failed("Media Foundation"))?;
            attributes
                .SetUINT32(
                    &MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT,
                    DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32,
                )
                .map_err(failed("Media Foundation"))?;
        }
        let factory: IMFMediaEngineClassFactory =
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
                .map_err(failed("Windows has no media engine"))?;
        let flags = if video { 0 } else { MF_MEDIA_ENGINE_AUDIOONLY.0 as u32 };
        let engine = factory
            .CreateInstance(flags, &attributes)
            .map_err(failed("Windows' media engine didn't start"))?;
        let extended: IMFMediaEngineEx = engine.cast().map_err(failed("Windows' media engine"))?;
        engine
            .SetPreload(MF_MEDIA_ENGINE_PRELOAD_AUTOMATIC)
            .map_err(failed("Windows' media engine"))?;
        // A byte stream rather than an address: any path opens, whatever it holds (spaces, Arabic, a share).
        let wide: Vec<u16> = path.encode_utf16().chain([0]).collect();
        let stream = MFCreateFile(
            MF_ACCESSMODE_READ,
            MF_OPENMODE_FAIL_IF_NOT_EXIST,
            MF_FILEFLAGS_NONE,
            PCWSTR(wide.as_ptr()),
        )
        .map_err(failed("The file couldn't be opened"))?;
        extended
            .SetSourceFromByteStream(&stream, &BSTR::from(file_url(path)))
            .map_err(failed("Windows couldn't read the file"))?;
        Ok(Session {
            engine,
            link,
            gpu,
            video,
            native: (0, 0),
            want: (0, 0),
            playing: false,
            stop_at: None,
            last_time: Instant::now(),
            settle_until: None,
            closing: false,
        })
    }

    /// Serves messages until the view lets go, polling for pictures and the time as it goes.
    fn run(&mut self, inbox: &Receiver<Msg>) {
        loop {
            let wait = if self.playing && self.video {
                Duration::from_millis(4)
            } else if self.playing {
                Duration::from_millis(25)
            } else if self.settling() {
                Duration::from_millis(30)
            } else {
                Duration::from_millis(500)
            };
            match inbox.recv_timeout(wait) {
                Ok(first) => {
                    let mut batch = vec![first];
                    while let Ok(more) = inbox.try_recv() {
                        batch.push(more);
                    }
                    for msg in coalesce(batch) {
                        self.handle(msg);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            if self.closing || self.link.events.is_closed() {
                break;
            }
            self.tick();
        }
    }

    fn settling(&self) -> bool {
        self.settle_until.is_some_and(|until| Instant::now() < until)
    }

    fn settle(&mut self) {
        self.settle_until = Some(Instant::now() + SETTLE);
    }

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Close => self.closing = true,
            Msg::Notify(event, param1) => self.notified(event, param1),
            Msg::Command(command) => self.command(command),
        }
    }

    fn command(&mut self, command: Command) {
        // SAFETY: the engine is alive (`self` owns it) and used on this thread alone.
        unsafe {
            match command {
                Command::Play => {
                    let _ = self.engine.Play();
                }
                Command::Pause => {
                    let _ = self.engine.Pause();
                }
                Command::Seek(seconds) => {
                    let _ = self.engine.SetCurrentTime(seconds.max(0.));
                    self.settle();
                }
                Command::Volume(volume) => {
                    let _ = self.engine.SetVolume(volume.clamp(0., 1.));
                }
                Command::Muted(muted) => {
                    let _ = self.engine.SetMuted(muted);
                }
                Command::Rate(rate) => {
                    let _ = self.engine.SetDefaultPlaybackRate(rate);
                    let _ = self.engine.SetPlaybackRate(rate);
                }
                Command::StopAt(at) => self.stop_at = at,
                Command::Frame { width, height } => {
                    if self.want != (width, height) {
                        self.want = (width, height);
                        self.retarget_and_show();
                    }
                }
            }
        }
    }

    fn notified(&mut self, event: u32, param1: usize) {
        match MF_MEDIA_ENGINE_EVENT(event as i32) {
            MF_MEDIA_ENGINE_EVENT_LOADEDMETADATA
            | MF_MEDIA_ENGINE_EVENT_DURATIONCHANGE
            | MF_MEDIA_ENGINE_EVENT_FORMATCHANGE => self.loaded(),
            MF_MEDIA_ENGINE_EVENT_LOADEDDATA
            | MF_MEDIA_ENGINE_EVENT_CANPLAY
            | MF_MEDIA_ENGINE_EVENT_FIRSTFRAMEREADY => {
                self.loaded();
                self.settle();
                self.retarget_and_show();
            }
            MF_MEDIA_ENGINE_EVENT_SEEKED => {
                self.tell_time();
                self.settle();
                self.show(true);
            }
            MF_MEDIA_ENGINE_EVENT_PLAYING => {
                self.playing = true;
                self.link.emit(Event::Waiting(false));
                self.link.emit(Event::Playing(true));
            }
            MF_MEDIA_ENGINE_EVENT_PAUSE => {
                self.playing = false;
                self.link.emit(Event::Playing(false));
                self.tell_time();
            }
            MF_MEDIA_ENGINE_EVENT_ENDED => {
                self.playing = false;
                self.link.emit(Event::Playing(false));
                self.tell_time();
                self.link.emit(Event::Ended);
            }
            MF_MEDIA_ENGINE_EVENT_WAITING | MF_MEDIA_ENGINE_EVENT_STALLED => self.link.emit(Event::Waiting(true)),
            MF_MEDIA_ENGINE_EVENT_ERROR => self.link.emit(Event::Failed(error_text(param1).to_string())),
            MF_MEDIA_ENGINE_EVENT_RESOURCELOST => self.link.emit(Event::Failed(error_text(0).to_string())),
            _ => {}
        }
    }

    /// Tells the view what the file turned out to be.
    fn loaded(&mut self) {
        // SAFETY: the engine is alive and used on this thread alone; the out parameters are locals.
        let facts = unsafe {
            let duration = self.engine.GetDuration();
            let (mut width, mut height) = (0u32, 0u32);
            // Without a Direct3D device there are no pictures to show, and the file plays as sound alone.
            let has_video = self.engine.HasVideo().as_bool() && self.gpu.is_some();
            if has_video {
                let _ = self.engine.GetNativeVideoSize(Some(&mut width), Some(&mut height));
            }
            Facts {
                // A stream says infinity, and it says nothing before it has read the start.
                duration: if duration.is_finite() && duration > 0. {
                    duration
                } else {
                    0.
                },
                width,
                height,
                has_video,
                has_audio: self.engine.HasAudio().as_bool(),
            }
        };
        self.native = (facts.width, facts.height);
        self.link.emit(Event::Loaded(facts));
        if facts.has_video && self.gpu.is_some() {
            self.retarget_and_show();
        }
    }

    fn tell_time(&mut self) {
        // SAFETY: the engine is alive and used on this thread alone.
        let now = unsafe { self.engine.GetCurrentTime() };
        self.last_time = Instant::now();
        self.link
            .emit(Event::Time(if now.is_finite() { now.max(0.) } else { 0. }));
    }

    /// Once a poll: the time, the Trim bar's stop, the next picture.
    fn tick(&mut self) {
        if self.playing {
            if let Some(end) = self.stop_at {
                // SAFETY: as above.
                let now = unsafe { self.engine.GetCurrentTime() };
                if now >= end {
                    // SAFETY: as above.
                    let _ = unsafe { self.engine.Pause() };
                }
            }
            if self.last_time.elapsed() >= TIME_EVERY {
                self.tell_time();
            }
        }
        if self.video && (self.playing || self.settling()) {
            self.show(false);
        }
    }

    // ---- Pictures ------------------------------------------------------------------------------------

    /// Makes the textures the size a picture is wanted at, and shows the current frame in them.
    fn retarget_and_show(&mut self) {
        if !self.video || self.native.0 == 0 {
            return;
        }
        let size = frame_size(self.native, self.want);
        if size.0 == 0 {
            return;
        }
        let Some(gpu) = self.gpu.as_mut() else {
            return;
        };
        // SAFETY: the device is the one `Gpu::new` made, used on this thread.
        if unsafe { gpu.retarget(size) } {
            self.settle();
            self.show(true);
        }
    }

    /// Reads the engine's current frame into a picture and hands it to the view: when the engine says there is a
    /// new one, or always when `force`.
    fn show(&mut self, force: bool) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let Some(target) = gpu.target.as_ref() else {
            return;
        };
        // SAFETY: the engine and the device are alive and used on this thread alone; `pts` outlives the call.
        let fresh = unsafe {
            let mut pts = 0i64;
            // `OnVideoStreamTick` answers S_OK for a new frame and S_FALSE for none, which the wrapper's
            // `Result` would not tell apart.
            let answer = (Interface::vtable(&self.engine).OnVideoStreamTick)(Interface::as_raw(&self.engine), &mut pts);
            answer.0 == 0
        };
        if !fresh && !force {
            return;
        }
        // SAFETY: as above; the mapped memory is read only between `Map` and `Unmap`.
        if let Some(frame) = unsafe { self.read_frame(gpu, target) } {
            if fresh {
                self.settle_until = None;
            }
            self.link.show(frame);
        }
    }

    /// The engine's frame, scaled into the target, copied out as BGRA.
    ///
    /// # Safety
    /// `gpu` and `target` are this session's, used on this thread.
    unsafe fn read_frame(&self, gpu: &Gpu, target: &Target) -> Option<Frame> {
        let (width, height) = target.size;
        let area = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        self.engine
            .TransferVideoFrame(&target.texture, None, &area, None)
            .ok()?;
        gpu.context.CopyResource(&target.staging, &target.texture);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        gpu.context
            .Map(&target.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .ok()?;
        let (row, pitch) = (width as usize * 4, mapped.RowPitch as usize);
        let mut bytes = Vec::with_capacity(row * height as usize);
        if !mapped.pData.is_null() && pitch >= row {
            let source = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * height as usize);
            for line in source.chunks_exact(pitch) {
                bytes.extend_from_slice(&line[..row]);
            }
        }
        gpu.context.Unmap(&target.staging, 0);
        if bytes.len() != row * height as usize {
            return None;
        }
        // The video processor leaves alpha at 0; GPUI draws premultiplied pixels, so make them opaque.
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel[3] = 255;
        }
        let image = bgra_image(width, height, bytes)?;
        Some(Frame { image, width, height })
    }
}

impl Drop for Session<'_> {
    fn drop(&mut self) {
        // SAFETY: the engine is alive; pausing and shutting it down stops the sound before it is released.
        unsafe {
            let _ = self.engine.Pause();
            let _ = self.engine.Shutdown();
        }
    }
}

/// Pixels in GPUI's order (BGRA) as an image to draw.
pub fn bgra_image(width: u32, height: u32, bytes: Vec<u8>) -> Option<Arc<RenderImage>> {
    // GPUI keeps its pictures as BGRA in a buffer typed RGBA: the bytes are not swapped.
    let buffer = image::RgbaImage::from_raw(width, height, bytes)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
}

/// A batch of messages with what a later message makes pointless dropped: of several seeks only the last matters
/// (a drag on the seek bar sends many), likewise the box, the volume, the speed and the stop. The order of the
/// rest is kept.
fn coalesce(batch: Vec<Msg>) -> Vec<Msg> {
    let same = |a: &Msg, b: &Msg| match (a, b) {
        (Msg::Command(a), Msg::Command(b)) => {
            std::mem::discriminant(a) == std::mem::discriminant(b)
                && matches!(
                    a,
                    Command::Seek(_)
                        | Command::Frame { .. }
                        | Command::Volume(_)
                        | Command::Rate(_)
                        | Command::StopAt(_)
                )
        }
        _ => false,
    };
    let keep: Vec<bool> = (0..batch.len())
        .map(|i| !batch[i + 1..].iter().any(|later| same(&batch[i], later)))
        .collect();
    batch
        .into_iter()
        .zip(keep)
        .filter_map(|(msg, keep)| keep.then_some(msg))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(batch: Vec<Msg>) -> Vec<Command> {
        coalesce(batch)
            .into_iter()
            .filter_map(|msg| match msg {
                Msg::Command(command) => Some(command),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_drag_of_seeks_is_one_seek_and_the_order_of_the_rest_is_kept() {
        let batch = vec![
            Msg::Command(Command::Seek(1.)),
            Msg::Command(Command::Play),
            Msg::Command(Command::Seek(2.)),
            Msg::Command(Command::Volume(0.5)),
            Msg::Command(Command::Seek(3.)),
            Msg::Command(Command::Volume(0.2)),
            Msg::Command(Command::Pause),
        ];
        assert_eq!(
            commands(batch),
            [Command::Play, Command::Seek(3.), Command::Volume(0.2), Command::Pause]
        );
    }

    #[test]
    fn notifications_and_closing_are_never_dropped() {
        let batch = vec![Msg::Notify(1, 0), Msg::Notify(1, 0), Msg::Close];
        assert_eq!(coalesce(batch).len(), 3);
    }

    #[test]
    fn pixels_become_an_image_of_the_right_size() {
        let image = bgra_image(2, 1, vec![0, 0, 255, 255, 255, 0, 0, 255]).expect("an image");
        assert_eq!(image.size(0).width.0, 2);
        assert_eq!(image.as_bytes(0).map(<[u8]>::len), Some(8));
        assert!(bgra_image(2, 2, vec![0; 4]).is_none(), "too few pixels");
    }

    // ---- The real engine ---------------------------------------------------------------------------------

    /// Waits for an event that `pick` takes, up to `seconds`; the events before it are given to `seen`.
    fn wait_for<T>(
        events: &async_channel::Receiver<Event>,
        seconds: u64,
        mut pick: impl FnMut(&Event) -> Option<T>,
    ) -> Option<T> {
        let until = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < until {
            while let Ok(event) = events.try_recv() {
                if let Some(found) = pick(&event) {
                    return Some(found);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }

    /// Windows' own sample sound plays for a second through the real engine, and its time moves.
    #[test]
    #[ignore = "plays a sound through the real media engine (muted); needs Windows' Media folder"]
    fn a_real_wav_plays_and_its_time_moves() {
        let Some(path) = std::fs::read_dir(r"C:\Windows\Media")
            .ok()
            .and_then(|dir| {
                dir.flatten()
                    .map(|e| e.path())
                    .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")))
            })
            .map(|p| p.to_string_lossy().into_owned())
        else {
            return;
        };
        let Opened { engine, events } = open(&path, Kind::Audio);
        let facts = wait_for(&events, 10, |e| match e {
            Event::Loaded(facts) if facts.duration > 0. => Some(*facts),
            Event::Failed(why) => panic!("the engine failed: {why}"),
            _ => None,
        })
        .expect("the sound loads");
        assert!(facts.has_audio && !facts.has_video, "{facts:?}");
        // Muted: the test must not be heard.
        engine.send(Command::Muted(true));
        engine.send(Command::Play);
        assert!(wait_for(&events, 5, |e| matches!(e, Event::Playing(true)).then_some(())).is_some());
        std::thread::sleep(Duration::from_millis(1000));
        let mut last = 0.;
        while let Ok(event) = events.try_recv() {
            if let Event::Time(at) = event {
                last = at;
            }
        }
        assert!(last > 0.3, "the time moved to {last}");
        // A seek is told as the time it went to.
        engine.send(Command::Pause);
        engine.send(Command::Seek(0.));
        let back = wait_for(&events, 5, |e| match e {
            Event::Time(at) if *at < 0.2 => Some(*at),
            _ => None,
        });
        assert!(back.is_some(), "the seek came back");
        drop(engine);
        // The thread goes with the handle: the channel closes.
        let until = Instant::now() + Duration::from_secs(5);
        while !events.is_closed() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(events.is_closed(), "the engine's thread ended");
    }

    /// A two-second video made by the user's FFmpeg (if it is there) plays through the real engine and gives
    /// pictures at the size asked for.
    #[test]
    #[ignore = "needs FFmpeg on the PATH to make a test video, and a GPU or WARP"]
    fn a_real_video_gives_pictures_of_the_size_asked_for() {
        let dir = std::env::temp_dir().join(format!("glint-player-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp folder");
        let video = dir.join("test clip.mp4");
        let made = std::process::Command::new("ffmpeg")
            .args([
                "-y",
                "-v",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=320x240:rate=30",
            ])
            .args([
                "-f",
                "lavfi",
                "-i",
                "sine=duration=2",
                "-pix_fmt",
                "yuv420p",
                "-c:v",
                "libx264",
                "-c:a",
                "aac",
            ])
            .arg(&video)
            .status();
        if !made.is_ok_and(|status| status.success()) {
            eprintln!("no FFmpeg to make a test video: skipped");
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
        let Opened { engine, events } = open(&video.to_string_lossy(), Kind::Video);
        let facts = wait_for(&events, 10, |e| match e {
            Event::Loaded(facts) if facts.has_video => Some(*facts),
            Event::Failed(why) => panic!("the engine failed: {why}"),
            _ => None,
        })
        .expect("the video loads");
        assert_eq!((facts.width, facts.height), (320, 240), "{facts:?}");
        assert!(facts.duration > 1.5);
        engine.send(Command::Muted(true));
        engine.send(Command::Frame {
            width: 160,
            height: 160,
        });
        engine.send(Command::Play);
        // Pictures come as it plays; one of them (not a blank one before the first is decoded) has the test
        // pattern's colours, at the size of the box asked for.
        let colourful = wait_for(&events, 10, |e| {
            if !matches!(e, Event::Frame) {
                return None;
            }
            let frame = engine.take_frame()?;
            assert_eq!((frame.width, frame.height), (160, 120), "scaled to the box asked for");
            let bytes = frame.image.as_bytes(0)?;
            assert_eq!(bytes.len(), 160 * 120 * 4);
            assert!(bytes.as_chunks::<4>().0.iter().all(|p| p[3] == 255), "opaque");
            let distinct: std::collections::HashSet<[u8; 4]> = bytes.as_chunks::<4>().0.iter().copied().collect();
            (distinct.len() > 8).then_some(distinct.len())
        });
        assert!(colourful.is_some(), "no picture had the test pattern's colours");
        engine.send(Command::Pause);
        drop(engine);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
