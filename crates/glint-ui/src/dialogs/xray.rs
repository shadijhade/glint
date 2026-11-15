//! A file's X-Ray: checksums, entropy, size on disk, hidden streams and signature. Ported from src/components/explorer/XRayDialog.tsx and the parts of SignatureChip.tsx it uses
//! (`describeSignature`), built the way [`super::drive_health`] is.
//!
//! | TypeScript                                    | Here                                                          |
//! |-----------------------------------------------|---------------------------------------------------------------|
//! | `useXRayStore` + `inspectXRay(path, full)`    | [`XRay`], a model entity; [`open`]                            |
//! | `inspectFileXRay`, `cancelXRay`               | [`Backend::inspect`], [`Backend::cancel`]                     |
//! | `verifySignature`, `describeSignature`        | [`Backend::signature`], [`describe_signature`]                |
//! | `removeNtfsStream`, `confirm(...)`            | [`Backend::remove_stream`], [`super::confirm`]                |
//! | `scanWithDefender([path])`                    | the check dialog's Defender scan (`check::open`)              |
//! | `XRayDialog`                                  | [`XRayDialog`]                                                |
//!
//! **How the coordinator wires it.** `MenuEvent::XRay { path }` is `dialogs::xray::open(window, cx, path)`.
//! The "Scan with Defender" button needs the `dialog-check` feature; without it the button is left out.
//!
//! The toasts of the TypeScript are notifications raised from [`XRayEvent`]s.

use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_core::sign::Signature;
use glint_core::xray::{FileXRayReport, NtfsStream};
use glint_model::format::{format_count, format_full_date, format_size};
use glint_model::paths::base_name;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// A boxed core call.
pub type Fut<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// The calls an X-Ray makes. The app uses [`CoreBackend`]; tests a fake.
pub trait Backend: Send + Sync {
    /// `inspect_file_xray`: hashes the first 256 MB, or the whole file when `full`.
    fn inspect(&self, path: String, full: bool) -> Fut<FileXRayReport>;
    /// `cancel_xray`.
    fn cancel(&self);
    /// `verify_signature`.
    fn signature(&self, path: String) -> Fut<Signature>;
    /// `remove_ntfs_stream`.
    fn remove_stream(&self, path: String, stream: String) -> Fut<()>;
}

/// glint-core.
pub struct CoreBackend;

impl Backend for CoreBackend {
    fn inspect(&self, path: String, full: bool) -> Fut<FileXRayReport> {
        Box::pin(glint_core::xray::inspect_file_xray(path, Some(full)))
    }
    fn cancel(&self) {
        glint_core::xray::cancel_xray();
    }
    fn signature(&self, path: String) -> Fut<Signature> {
        Box::pin(glint_core::sign::verify_signature(path))
    }
    fn remove_stream(&self, path: String, stream: String) -> Fut<()> {
        Box::pin(glint_core::xray::remove_ntfs_stream(path, stream))
    }
}

// ---- What the words mean ----------------------------------------------------------------------------

/// Extensions Windows can check a signature of (`SIGNABLE`).
const SIGNABLE: &[&str] = &[
    "exe",
    "dll",
    "sys",
    "ocx",
    "scr",
    "cpl",
    "efi",
    "drv",
    "mui",
    "node",
    "msi",
    "msp",
    "msm",
    "msix",
    "msixbundle",
    "appx",
    "appxbundle",
    "cab",
    "cat",
    "ps1",
    "psm1",
    "psd1",
    "ps1xml",
    "vbs",
    "js",
    "wsf",
];

/// The extension after the last dot of the file name (lowercase), if the name has one that isn't its start.
fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('\\').next().unwrap_or(path);
    let dot = name.rfind('.')?;
    (dot > 0).then(|| name[dot + 1..].to_lowercase())
}

/// Whether a signature can be checked (`signable`).
pub fn signable(path: &str) -> bool {
    extension(path).is_some_and(|e| SIGNABLE.contains(&e.as_str()))
}

/// How a signature reads: good, bad, a caution or neutral.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Warn,
    Plain,
}

/// What a signature check found, in words: a short line, and the rest for a list (`describeSignature`).
#[derive(Clone, Debug, PartialEq)]
pub struct Described {
    pub text: String,
    pub tone: Tone,
    pub details: Vec<String>,
}

/// `describeSignature`.
pub fn describe_signature(s: &Signature) -> Described {
    let by = s
        .signer
        .as_ref()
        .map(|signer| t!(" by {signer}", signer = signer).to_string())
        .unwrap_or_default();
    let mut details: Vec<String> = Vec::new();
    if let Some(issuer) = s.issuer.as_ref().filter(|i| !i.is_empty()) {
        details.push(t!("Its certificate was issued by {issuer}", issuer = issuer).to_string());
    }
    if s.catalog {
        details.push(t!("Signed in a Windows catalog, as Windows' own files are").to_string());
    }
    if let Some(at) = s.signed_at.filter(|&at| at != 0) {
        details.push(t!("Signed {date}", date = format_full_date(at)).to_string());
    }
    if s.state != "valid" {
        details.extend(s.detail.clone().filter(|d| !d.is_empty()));
    }
    details.push(t!("Checked offline: certificates aren't looked up as revoked").to_string());
    match s.state {
        "valid" => Described {
            text: t!("Signed{by}", by = by).to_string(),
            tone: Tone::Good,
            details,
        },
        "unsigned" => Described {
            text: t!("Not signed").to_string(),
            tone: Tone::Plain,
            details: vec![t!("No digital signature, in the file or in a Windows catalog").to_string()],
        },
        "invalid" => Described {
            text: t!("Signature doesn't hold: changed after it was signed{by}", by = by).to_string(),
            tone: Tone::Bad,
            details,
        },
        "untrusted" => Described {
            text: t!("Signed{by}, but not with a certificate Windows trusts", by = by).to_string(),
            tone: Tone::Warn,
            details,
        },
        "expired" => Described {
            text: t!(
                "Signed{by}, with a certificate that expired (and no timestamp)",
                by = by
            )
            .to_string(),
            tone: Tone::Warn,
            details,
        },
        "revoked" => Described {
            text: t!("Signed{by}, with a certificate that was revoked", by = by).to_string(),
            tone: Tone::Bad,
            details,
        },
        _ => Described {
            text: t!("Couldn't check the signature").to_string(),
            tone: Tone::Plain,
            details,
        },
    }
}

/// Which checksum a pasted one is: SHA-256, BLAKE3 or CRC32, compared without case or surrounding space.
pub fn matched_algo(report: &FileXRayReport, pasted: &str) -> Option<&'static str> {
    let wanted = pasted.trim().to_lowercase();
    if wanted.is_empty() {
        None
    } else if wanted == report.sha256.to_lowercase() {
        Some("SHA-256")
    } else if wanted == report.blake3.to_lowercase() {
        Some("BLAKE3")
    } else if wanted == report.crc32.to_lowercase() {
        Some("CRC32")
    } else {
        None
    }
}

/// "0x10–0x1F" for bucket `ix` of the byte-value histogram.
pub fn bucket_range(ix: usize) -> String {
    format!("0x{:02X}–0x{:02X}", ix * 16, ix * 16 + 15)
}

/// How tall bucket bars are, as a share of the tallest (at least 3 %), in percent.
pub fn bar_height(pct: f64, tallest: f64) -> f32 {
    (((pct / tallest.max(1.0)) * 100.0).round() as f32).max(3.0)
}

// ---- The model --------------------------------------------------------------------------------------

/// Where the X-Ray itself stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Load {
    Loading,
    Failed(SharedString),
    Ready,
}

/// What the model tells the view to say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XRayEvent {
    /// The mark was taken off ("Unblocked “name”").
    Unblocked(String),
    /// A hidden stream was deleted.
    StreamDeleted(String),
    /// Removing failed; `zone` when it was the download mark.
    RemoveFailed { zone: bool, name: String, reason: String },
}

/// The X-Ray of one file.
pub struct XRay {
    path: String,
    full: bool,
    load: Load,
    report: Option<FileXRayReport>,
    signature: Option<Signature>,
    /// The hidden stream being deleted.
    removing: Option<String>,
    backend: Arc<dyn Backend>,
    /// The number of the inspection whose answer counts.
    run: u64,
    side_run: u64,
}

impl EventEmitter<XRayEvent> for XRay {}

impl XRay {
    /// The X-Ray of `path`, started: the report, and the signature beside it.
    pub fn new(path: String, backend: Arc<dyn Backend>, cx: &mut Context<Self>) -> Self {
        let mut this = Self {
            path,
            full: false,
            load: Load::Loading,
            report: None,
            signature: None,
            removing: None,
            backend,
            run: 0,
            side_run: 0,
        };
        this.inspect(false, cx);
        this.look_aside(cx);
        this
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn load(&self) -> &Load {
        &self.load
    }

    /// Whether the whole file is (being) hashed.
    pub fn full(&self) -> bool {
        self.full
    }

    pub fn report(&self) -> Option<&FileXRayReport> {
        self.report.as_ref()
    }

    /// Reads the file again; `full` hashes all of it (`inspectXRay(path, full)`).
    pub fn inspect(&mut self, full: bool, cx: &mut Context<Self>) {
        self.run += 1;
        let run = self.run;
        self.full = full;
        self.load = Load::Loading;
        self.report = None;
        cx.notify();
        let work = self.backend.inspect(self.path.clone(), full);
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                if this.run != run {
                    return;
                }
                match result {
                    Ok(report) => {
                        this.report = Some(report);
                        this.load = Load::Ready;
                    }
                    Err(reason) => this.load = Load::Failed(reason.into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks for the signature, where the file can be signed.
    pub fn look_aside(&mut self, cx: &mut Context<Self>) {
        self.side_run += 1;
        let run = self.side_run;
        self.signature = None;
        cx.notify();
        let path = self.path.clone();
        if signable(&path) {
            let work = self.backend.signature(path.clone());
            cx.spawn(async move |this, cx| {
                let result = core_bridge::run(work).await.and_then(|inner| inner);
                let _ = this.update(cx, |this, cx| {
                    if this.side_run == run {
                        // A check that fails is "couldn't check", as the TypeScript's catch made it.
                        this.signature = Some(result.unwrap_or(Signature {
                            state: "unknown",
                            ..Default::default()
                        }));
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// Deletes the hidden stream `name` (the download mark is `Zone.Identifier`), then reads the file again.
    pub fn remove_stream(&mut self, name: String, cx: &mut Context<Self>) {
        if self.removing.is_some() {
            return;
        }
        self.removing = Some(name.clone());
        cx.notify();
        let zone = name.eq_ignore_ascii_case("zone.identifier");
        let file = base_name(&self.path).to_string();
        let work = self.backend.remove_stream(self.path.clone(), name.clone());
        cx.spawn(async move |this, cx| {
            let result = core_bridge::run(work).await.and_then(|inner| inner);
            let _ = this.update(cx, |this, cx| {
                this.removing = None;
                match result {
                    Ok(()) => {
                        cx.emit(if zone {
                            XRayEvent::Unblocked(file)
                        } else {
                            XRayEvent::StreamDeleted(name)
                        });
                        this.inspect(this.full, cx);
                    }
                    Err(reason) => cx.emit(XRayEvent::RemoveFailed {
                        zone,
                        name: if zone { file } else { name },
                        reason,
                    }),
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Drop for XRay {
    fn drop(&mut self) {
        // Closing stops the hashing (`closeXRay`).
        if self.load == Load::Loading {
            self.backend.cancel();
        }
    }
}

// ---- The dialog -------------------------------------------------------------------------------------

/// A file's X-Ray in full.
pub struct XRayDialog {
    xray: Entity<XRay>,
    expected: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
}

impl XRayDialog {
    /// The view of `xray`.
    pub fn new(xray: Entity<XRay>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // F5 is the Refresh button's key, as in the web app's dialogs.
        super::refresh_on_f5(cx, |this, _, cx| {
            this.xray.update(cx, |xray, cx| xray.inspect(false, cx));
        });
        let expected = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Paste a checksum to compare")));
        let subscriptions = vec![
            cx.observe(&xray, |_, _, cx| cx.notify()),
            cx.subscribe_in(&expected, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe_in(&xray, window, |_, _, event: &XRayEvent, window, cx| match event {
                XRayEvent::Unblocked(name) => {
                    window.push_notification(Notification::success(t!("Unblocked “{name}”", name = name)), cx)
                }
                XRayEvent::StreamDeleted(stream) => window.push_notification(
                    Notification::success(t!("Deleted the stream “:{stream}”", stream = stream)),
                    cx,
                ),
                XRayEvent::RemoveFailed { zone, name, reason } => window.push_notification(
                    Notification::error(SharedString::from(reason.clone())).title(if *zone {
                        t!("Couldn't unblock “{name}”", name = name)
                    } else {
                        t!("Couldn't delete the stream")
                    }),
                    cx,
                ),
            }),
        ];
        Self {
            xray,
            expected,
            _subscriptions: subscriptions,
        }
    }

    fn card() -> Div {
        col().gap(space(2.5))
    }

    fn card_title(name: IconName, text: SharedString, t: &theme::Tokens) -> Div {
        align_row(1.5)
            .font_weight(FontWeight::SEMIBOLD)
            .child(icon(name).size_3p5().text_color(t.primary))
            .child(text)
    }

    /// A bordered section of the body, named for a screen reader.
    fn section(id: &'static str, label: SharedString, t: &theme::Tokens) -> impl IntoElement + ParentElement + Styled {
        Self::card()
            .id(id)
            .role(Role::Group)
            .aria_label(label)
            .test_support()
            .p(space(3.5))
            .rounded(theme::size::RADIUS_LG)
            .border_1()
            .border_color(t.border)
            .bg(t.card)
    }

    fn pill(text: SharedString, fill: Hsla, ink: Hsla) -> Div {
        div()
            .px(space(2.))
            .py(space(0.5))
            .rounded(theme::size::RADIUS_SM)
            .font_weight(FontWeight::MEDIUM)
            .bg(fill)
            .text_color(ink)
            .child(text)
    }

    /// A [`Self::pill`] a screen reader reads as a status.
    fn status_pill(id: &'static str, text: SharedString, fill: Hsla, ink: Hsla) -> impl IntoElement {
        Self::pill(text.clone(), fill, ink)
            .id(id)
            .role(Role::Status)
            .aria_label(text)
            .test_support()
    }

    fn tone_colors(tone: Tone, t: &theme::Tokens) -> (Hsla, Hsla) {
        match tone {
            Tone::Good => (t.success.opacity(0.15), t.success),
            Tone::Bad => (t.destructive.opacity(0.15), t.destructive),
            Tone::Warn => (t.warning.opacity(0.15), t.warning),
            Tone::Plain => (t.muted, t.muted_foreground),
        }
    }

    fn signature_section(&self, t: &theme::Tokens, cx: &App) -> impl IntoElement {
        let xray = self.xray.read(cx);
        let described = xray.signature.as_ref().map(describe_signature);
        let label = t!("Signature");
        let headline: AnyElement = match &described {
            Some(d) => {
                let (fill, ink) = Self::tone_colors(d.tone, t);
                Self::status_pill("xray-signature-state", d.text.clone().into(), fill, ink).into_any_element()
            }
            None => align_row(1.)
                .text_color(t.muted_foreground)
                .child(Spinner::new().color(t.primary))
                .child(t!("Checking…"))
                .into_any_element(),
        };
        Self::section("xray-signature", label.clone(), t)
            .child(
                row_start()
                    .flex_wrap()
                    .items_center()
                    .gap(space(2.))
                    .child(Self::card_title(IconName::ShieldCheck, label, t))
                    .child(headline),
            )
            .children(described.filter(|d| !d.details.is_empty()).map(|d| {
                col()
                    .id("xray-signature-details")
                    .role(Role::List)
                    .aria_label(t!("Signature details"))
                    .test_support()
                    .gap(space(0.5))
                    .ps(5.)
                    .text_color(t.muted_foreground)
                    .children(d.details.into_iter().enumerate().map(|(i, line)| {
                        div()
                            .id(("xray-signature-detail", i))
                            .role(Role::ListItem)
                            .aria_label(line.clone())
                            .test_support()
                            .child(format!("• {line}"))
                    }))
            }))
    }

    fn zone_banner(&self, report: &FileXRayReport, t: &theme::Tokens, cx: &mut Context<Self>) -> Option<AnyElement> {
        let zone = report.zone_info.as_ref()?;
        let removing = self.xray.read(cx).removing.is_some();
        let headline = t!("Marked as downloaded · {zone}", zone = zone.zone_label);
        let url = |id: &'static str, text: SharedString| {
            div()
                .id(id)
                .role(Role::Group)
                .aria_label(text.clone())
                .test_support()
                .max_w(rems(32.))
                .truncate()
                .font_family(theme::mono_font(cx))
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(text)
        };
        Some(
            row_start()
                .id("xray-zone")
                .role(Role::Group)
                .aria_label(headline.clone())
                .test_support()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap(space(3.))
                .p(space(3.))
                .rounded(theme::size::RADIUS_LG)
                .border_1()
                .border_color(t.warning.opacity(0.3))
                .bg(t.warning.opacity(0.1))
                .child(
                    col()
                        .gap(space(1.))
                        .min_w_0()
                        .child(
                            align_row(1.5)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.warning)
                                .child(icon(IconName::ShieldAlert).size_4().flex_none())
                                .child(headline),
                        )
                        .children(
                            zone.host_url
                                .as_ref()
                                .filter(|u| !u.is_empty())
                                .map(|u| url("xray-zone-host", t!("Source URL: {url}", url = u))),
                        )
                        .children(
                            zone.referrer_url
                                .as_ref()
                                .filter(|u| !u.is_empty())
                                .map(|u| url("xray-zone-referrer", t!("Referrer: {url}", url = u))),
                        ),
                )
                .child(
                    Button::new("xray-unblock")
                        .outline()
                        .small()
                        .icon(IconName::ShieldCheck)
                        .label(t!("Unblock"))
                        .loading(removing)
                        .disabled(removing)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.ask_remove("Zone.Identifier".into(), window, cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Asks before taking a stream off (`handleRemoveStream`'s `confirm`).
    fn ask_remove(&mut self, stream: String, window: &mut Window, cx: &mut Context<Self>) {
        let name = {
            let path = &self.xray.read(cx).path;
            let name = base_name(path);
            if name.is_empty() {
                path.clone()
            } else {
                name.to_string()
            }
        };
        let zone = stream.eq_ignore_ascii_case("zone.identifier");
        let (title, message, ok) = if zone {
            (
                t!("Unblock file"),
                t!(
                    "Unblock “{name}”?\n\nWindows marked it as downloaded from the internet. Unblocking removes that mark, so Windows (SmartScreen, Office's Protected View) stops warning you before it opens. Only unblock files you trust.",
                    name = name
                ),
                t!("Unblock"),
            )
        } else {
            (
                t!("Delete stream"),
                t!(
                    "Delete the hidden stream “:{stream}” from “{name}”?\n\nWhat another program stored in it is gone for good.",
                    stream = stream,
                    name = name
                ),
                t!("Delete"),
            )
        };
        let xray = self.xray.clone();
        super::confirm(window, cx, title, message, ok, !zone, move |_, cx| {
            xray.update(cx, |xray, cx| xray.remove_stream(stream.clone(), cx));
        });
    }

    fn checksums(&self, report: &FileXRayReport, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let complete = report.hashes_complete;
        let pasted = self.expected.read(cx).value().to_string();
        let matched = matched_algo(report, &pasted);
        let mono = theme::mono_font(cx);
        let label = t!("Checksums");

        let verdict: Option<AnyElement> = (!pasted.trim().is_empty()).then(|| match matched {
            Some(algo) => {
                let text = t!("{algo} matches", algo = algo);
                div()
                    .id("xray-match")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .child(Self::pill(text, t.success.opacity(0.15), t.success))
                    .into_any_element()
            }
            None if complete => {
                let text = t!("Doesn't match");
                div()
                    .id("xray-mismatch")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .child(Self::pill(text, t.destructive.opacity(0.15), t.destructive))
                    .into_any_element()
            }
            None => {
                let text = t!("Not checked: hash the whole file first");
                div()
                    .id("xray-unchecked")
                    .role(Role::Status)
                    .aria_label(text.clone())
                    .test_support()
                    .child(Self::pill(text, t.warning.opacity(0.15), t.warning))
                    .into_any_element()
            }
        });

        let rows = [
            ("SHA-256", report.sha256.clone()),
            ("BLAKE3", report.blake3.clone()),
            ("CRC32", report.crc32.clone()),
        ];
        let table = col()
            .rounded(theme::size::RADIUS)
            .border_1()
            .border_color(t.border)
            .overflow_hidden()
            .children(rows.into_iter().enumerate().map(|(ix, (name, value))| {
                let is_match = matched == Some(name);
                let copied = value.clone();
                row_start()
                    .id(("xray-hash", ix))
                    .role(Role::Group)
                    .aria_label(format!("{name}: {value}"))
                    .test_support()
                    .items_center()
                    .gap(space(3.))
                    .px(space(2.5))
                    .py(space(1.5))
                    .when(ix > 0, |this| this.border_t_1().border_color(t.border))
                    .when(is_match, |this| this.bg(t.success.opacity(0.1)).text_color(t.success))
                    .child(
                        div()
                            .w(rems(5.5))
                            .flex_none()
                            .text_size(theme::size::XS)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if is_match { t.success } else { t.muted_foreground })
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .child(value),
                    )
                    .child(
                        Button::new(("xray-copy", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Copy)
                            .tooltip(t!("Copy {name}", name = name))
                            .accessibility_label(t!("Copy {name}", name = name))
                            .on_click(move |_, window, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()));
                                window.push_notification(Notification::success(t!("Copied {name}", name = name)), cx);
                            }),
                    )
            }));

        Self::section("xray-checksums", label.clone(), t)
            .child(
                row_start()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(space(2.))
                    .child(
                        align_row(1.5)
                            .child(Self::card_title(IconName::FingerprintPattern, label, t))
                            .children((!complete).then(|| {
                                align_row(1.)
                                    .px(space(1.5))
                                    .py(space(0.5))
                                    .rounded(theme::size::RADIUS_SM)
                                    .bg(t.warning.opacity(0.15))
                                    .text_color(t.warning)
                                    .text_size(theme::size::XXS)
                                    .child(t!("Of the first 256 MB ·"))
                                    .child(
                                        Button::new("xray-hash-all")
                                            .link()
                                            .xsmall()
                                            .label(t!("Hash the whole file"))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.xray.update(cx, |xray, cx| xray.inspect(true, cx))
                                            })),
                                    )
                            })),
                    )
                    .child(
                        align_row(2.)
                            .child(
                                div()
                                    .id("xray-expected")
                                    .w(rems(16.))
                                    .tooltip(|window, cx| {
                                        crate::kit::tooltip(
                                            t!("A SHA-256, BLAKE3 or CRC32 checksum, as a download page publishes it"),
                                            window,
                                            cx,
                                        )
                                    })
                                    .child(Input::new(&self.expected).aria_label(t!("Paste a checksum to compare"))),
                            )
                            .children(verdict),
                    ),
            )
            .child(table)
    }

    fn entropy(&self, report: &FileXRayReport, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let label = t!("Entropy and byte values");
        let share = (report.entropy / 8.0).clamp(0.0, 1.0);
        let tallest = report.byte_spectrum.iter().cloned().fold(1.0_f64, f64::max);
        let mono = theme::mono_font(cx);
        let bars = row_start()
            .id("xray-spectrum")
            .role(Role::List)
            .aria_label(t!("Byte values"))
            .test_support()
            .h(rems(4.))
            .items_end()
            .gap(space(1.))
            .px(space(2.))
            .pt(space(2.))
            .pb(space(1.))
            .rounded(theme::size::RADIUS_SM)
            .border_1()
            .border_color(t.border)
            .bg(t.muted.opacity(0.2))
            .children(report.byte_spectrum.iter().enumerate().map(|(ix, pct)| {
                let tip = t!(
                    "{range}: {pct}% of bytes",
                    range = bucket_range(ix),
                    pct = format!("{pct:.1}")
                );
                col()
                    .id(("xray-bar", ix))
                    .role(Role::ListItem)
                    .aria_label(tip.clone())
                    .test_support()
                    .h_full()
                    .flex_1()
                    .justify_end()
                    .items_center()
                    .tooltip({
                        let tip = tip.clone();
                        move |window, cx| crate::kit::tooltip(tip.clone(), window, cx)
                    })
                    .child(
                        div()
                            .w_full()
                            .h(relative(bar_height(*pct, tallest) / 100.0))
                            .rounded_t(px(2.))
                            .bg(t.primary.opacity(0.75)),
                    )
            }));
        Self::section("xray-entropy", label.clone(), t)
            .gap(space(3.))
            .child(
                row_start()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap(space(2.))
                    .child(Self::card_title(IconName::Sigma, label, t))
                    .child(
                        div()
                            .id("xray-entropy-value")
                            .role(Role::Status)
                            .aria_label(t!("H = {h} / 8.000 bits/B", h = format!("{:.3}", report.entropy)))
                            .test_support()
                            .px(space(2.))
                            .py(space(0.5))
                            .rounded(theme::size::RADIUS)
                            .bg(t.primary.opacity(0.1))
                            .text_color(t.primary)
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(t!("H = {h} / 8.000 bits/B", h = format!("{:.3}", report.entropy))),
                    ),
            )
            .child(
                col()
                    .gap(space(1.))
                    .child(
                        row_start()
                            .justify_between()
                            .text_size(theme::size::XXS)
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(report.entropy_label.clone()),
                            )
                            .child(
                                div()
                                    .text_color(t.muted_foreground)
                                    .child(t!("{n}% of the maximum", n = (share * 100.0).round() as i64)),
                            ),
                    )
                    .child(
                        div()
                            .h(space(2.))
                            .w_full()
                            .rounded_full()
                            .overflow_hidden()
                            .bg(t.muted)
                            .child(div().h_full().w(relative(share as f32)).bg(t.primary.opacity(0.7))),
                    ),
            )
            .child(bars)
            .child(
                row_start()
                    .justify_between()
                    .px(space(1.))
                    .font_family(mono)
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child("0x00 (NUL)")
                    .child(t!(
                        "0x20–0x7E (ASCII: {pct}%)",
                        pct = format!("{:.1}", report.ascii_pct)
                    ))
                    .child(t!(
                        "0x80–0xFF (High: {pct}%)",
                        pct = format!("{:.1}", report.high_byte_pct)
                    )),
            )
    }

    fn facts(
        id: &'static str,
        name: IconName,
        title: SharedString,
        rows: Vec<(SharedString, String)>,
        t: &theme::Tokens,
        mono: &SharedString,
    ) -> impl IntoElement {
        Self::section(id, title.clone(), t)
            .flex_1()
            .min_w(rems(14.))
            .child(Self::card_title(name, title, t))
            .children(rows.into_iter().enumerate().map(|(ix, (label, value))| {
                row_start()
                    .id((id, ix))
                    .role(Role::Group)
                    .aria_label(format!("{label}: {value}"))
                    .test_support()
                    .items_start()
                    .justify_between()
                    .gap(space(3.))
                    .text_size(theme::size::XXS)
                    .child(div().text_color(t.muted_foreground).child(label))
                    .child(div().min_w_0().font_family(mono.clone()).text_end().child(value))
            }))
    }

    fn streams(&self, report: &FileXRayReport, t: &theme::Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let removing = self.xray.read(cx).removing.clone();
        let busy = removing.is_some();
        let label = t!("Hidden streams ({n})", n = report.streams.len());
        let mono = theme::mono_font(cx);
        let body: AnyElement = if report.streams.is_empty() {
            div()
                .py(space(2.))
                .text_size(theme::size::XXS)
                .text_color(t.muted_foreground)
                .child(t!("None: the file has only its own content."))
                .into_any_element()
        } else {
            col()
                .rounded(theme::size::RADIUS)
                .border_1()
                .border_color(t.border)
                .children(report.streams.iter().enumerate().map(|(ix, stream)| {
                    self.stream_row(
                        ix,
                        stream,
                        removing.as_deref() == Some(stream.clean_name.as_str()),
                        busy,
                        t,
                        &mono,
                        cx,
                    )
                }))
                .into_any_element()
        };
        Self::section("xray-streams", label.clone(), t)
            .child(
                row_start()
                    .items_center()
                    .justify_between()
                    .child(Self::card_title(IconName::FileStack, label, t))
                    .child(
                        div()
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(t!("NTFS alternate data streams")),
                    ),
            )
            .child(body)
    }

    #[allow(clippy::too_many_arguments)]
    fn stream_row(
        &self,
        ix: usize,
        stream: &NtfsStream,
        deleting: bool,
        busy: bool,
        t: &theme::Tokens,
        mono: &SharedString,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let clean = stream.clean_name.clone();
        col()
            .id(("xray-stream", ix))
            .role(Role::Group)
            .aria_label(format!(":{} · {}", stream.clean_name, format_size(stream.size)))
            .test_support()
            .gap(space(1.5))
            .p(space(2.5))
            .when(ix > 0, |this| this.border_t_1().border_color(t.border))
            .child(
                align_row(2.)
                    .child(
                        div()
                            .font_family(mono.clone())
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!(":{}", stream.clean_name)),
                    )
                    .child(
                        div()
                            .px(space(1.5))
                            .rounded(theme::size::RADIUS_SM)
                            .bg(t.muted)
                            .font_family(mono.clone())
                            .text_size(theme::size::XXS)
                            .text_color(t.muted_foreground)
                            .child(format_size(stream.size)),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(("xray-delete-stream", ix))
                            .ghost()
                            .danger()
                            .xsmall()
                            .icon(IconName::Trash)
                            .label(t!("Delete"))
                            .tooltip(t!("Delete the stream :{stream}", stream = stream.clean_name))
                            .accessibility_label(t!("Delete the stream :{stream}", stream = stream.clean_name))
                            .loading(deleting)
                            .disabled(busy)
                            .on_click(
                                cx.listener(move |this, _, window, cx| this.ask_remove(clean.clone(), window, cx)),
                            ),
                    ),
            )
            .children(stream.preview.as_ref().filter(|p| !p.is_empty()).map(|preview| {
                div()
                    .max_h(rems(7.))
                    .overflow_hidden()
                    .p(space(2.))
                    .rounded(theme::size::RADIUS_SM)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.background.opacity(0.7))
                    .font_family(mono.clone())
                    .text_size(theme::size::XXS)
                    .text_color(t.muted_foreground)
                    .child(preview.clone())
            }))
    }
}

impl Render for XRayDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let xray = self.xray.read(cx);
        let path = xray.path.clone();
        let load = xray.load.clone();
        let full = xray.full;
        let report = xray.report.clone();
        let name = {
            let name = base_name(&path);
            if name.is_empty() {
                path.clone()
            } else {
                name.to_string()
            }
        };
        let title = t!("File X-Ray: {name}", name = name);

        let defender: Option<AnyElement> = defender_button(&path);

        let header = row_start()
            .items_center()
            .gap(space(3.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(isolate(path.clone())),
            )
            .children(defender)
            .children(report.as_ref().map(|r| {
                div()
                    .text_size(theme::size::XS)
                    .text_color(t.muted_foreground)
                    .child(t!("{ms} ms", ms = format!("{:.1}", r.elapsed_ms)))
            }))
            .child(
                Button::new("xray-refresh")
                    .ghost()
                    .xsmall()
                    .icon(IconName::RefreshCw)
                    .loading(load == Load::Loading)
                    .tooltip(t!("Refresh (F5)"))
                    .accessibility_label(t!("Refresh"))
                    .on_click(cx.listener(|this, _, _, cx| this.xray.update(cx, |xray, cx| xray.inspect(false, cx)))),
            );

        let body: AnyElement = match (&load, &report) {
            (Load::Loading, _) => {
                let words = if full {
                    t!("Hashing the whole file…")
                } else {
                    t!("Reading the file…")
                };
                col()
                    .id("xray-loading")
                    .role(Role::Status)
                    .aria_label(words.clone())
                    .test_support()
                    .items_center()
                    .gap(space(2.5))
                    .py(space(16.))
                    .text_color(t.muted_foreground)
                    .child(Spinner::new().color(t.primary))
                    .child(words)
                    .into_any_element()
            }
            (Load::Failed(reason), _) => col()
                .id("xray-error")
                .role(Role::Alert)
                .aria_label(format!("{}. {}", t!("Couldn't X-Ray this item"), reason))
                .test_support()
                .items_center()
                .gap(space(2.))
                .py(space(14.))
                .text_center()
                .child(icon(IconName::TriangleAlert).size_6().text_color(t.destructive))
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child(t!("Couldn't X-Ray this item")),
                )
                .child(
                    div()
                        .max_w(rems(28.))
                        .text_size(theme::size::XS)
                        .text_color(t.muted_foreground)
                        .child(reason.clone()),
                )
                .into_any_element(),
            (Load::Ready, Some(report)) => {
                let mono = theme::mono_font(cx);
                let size_rows = vec![
                    (
                        t!("Logical size"),
                        format!(
                            "{} ({} B)",
                            format_size(report.logical_size),
                            format_count_u64(report.logical_size)
                        ),
                    ),
                    (
                        t!("Allocated on disk"),
                        format!(
                            "{} ({} B)",
                            format_size(report.allocated_size),
                            format_count_u64(report.allocated_size)
                        ),
                    ),
                    (t!("NTFS hardlinks"), report.hardlink_count.to_string()),
                ];
                let record_rows = vec![
                    (t!("MFT File ID"), report.file_index.clone()),
                    (t!("Volume Serial"), report.volume_serial.clone()),
                    (
                        t!("Attributes"),
                        if report.attributes.is_empty() {
                            t!("Normal").to_string()
                        } else {
                            report.attributes.join(", ")
                        },
                    ),
                ];
                col()
                    .gap(space(4.))
                    .children(self.zone_banner(report, &t, cx))
                    .children(signable(&path).then(|| self.signature_section(&t, cx)))
                    .child(self.checksums(report, &t, cx))
                    .child(self.entropy(report, &t, cx))
                    .child(
                        row_start()
                            .flex_wrap()
                            .gap(space(3.))
                            .child(Self::facts(
                                "xray-size",
                                IconName::HardDrive,
                                t!("Size on disk"),
                                size_rows,
                                &t,
                                &mono,
                            ))
                            .child(Self::facts(
                                "xray-record",
                                IconName::Binary,
                                t!("File record"),
                                record_rows,
                                &t,
                                &mono,
                            )),
                    )
                    .child(self.streams(report, &t, cx))
                    .into_any_element()
            }
            _ => div().into_any_element(),
        };

        let footer_note: SharedString = match &report {
            Some(r) if r.hashes_complete => t!("SHA-256, BLAKE3 and CRC32 of the whole file"),
            Some(_) => t!("SHA-256, BLAKE3 and CRC32 of the first 256 MB"),
            None => "".into(),
        };

        col()
            .id("xray")
            .role(Role::Group)
            .aria_label(title)
            .test_support()
            .gap(space(3.))
            .text_size(theme::size::XS)
            .text_start()
            .pt(space(1.))
            .child(header)
            .child(
                div()
                    .id("xray-scroll")
                    .test_support()
                    .min_h(rems(14.))
                    .max_h(rems(34.))
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(
                row_start()
                    .items_center()
                    .justify_between()
                    .pt(space(2.))
                    .border_t_1()
                    .border_color(t.border)
                    .text_color(t.muted_foreground)
                    .child(div().child(footer_note))
                    .child(
                        Button::new("xray-close")
                            .outline()
                            .small()
                            .label(t!("Close"))
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
            )
    }
}

/// A count of bytes with the user's digit grouping (counts beyond `usize` can't occur on a file here).
fn format_count_u64(n: u64) -> String {
    format_count(usize::try_from(n).unwrap_or(usize::MAX))
}

/// "Scan with Defender": opens the check dialog's Defender scan of the file.
fn defender_button(path: &str) -> Option<AnyElement> {
    let path = path.to_string();
    Some(
        Button::new("xray-defender")
            .outline()
            .xsmall()
            .icon(IconName::ShieldCheck)
            .label(t!("Scan with Defender"))
            .tooltip(t!("A custom scan of this file by Microsoft Defender"))
            .on_click(move |_, window, cx| {
                super::check::open(window, cx, vec![path.clone()])
            })
            .into_any_element(),
    )
}

// ---- Opening it -------------------------------------------------------------------------------------

/// Opens the X-Ray of `path` in `window` (`inspectXRay`). Nothing opens for an empty path.
pub fn open(window: &mut Window, cx: &mut App, path: String) {
    if path.is_empty() {
        return;
    }
    open_with(window, cx, Arc::new(CoreBackend), path);
}

/// [`open`] with a backend of your own (tests pass a fake). Returns the view.
pub fn open_with(window: &mut Window, cx: &mut App, backend: Arc<dyn Backend>, path: String) -> Entity<XRayDialog> {
    let title = t!(
        "File X-Ray: {name}",
        name = {
            let name = base_name(&path);
            if name.is_empty() {
                path.clone()
            } else {
                name.to_string()
            }
        }
    );
    let xray = cx.new(|cx| XRay::new(path, backend, cx));
    let view = cx.new(|cx| XRayDialog::new(xray, window, cx));
    super::show(window, cx, title, px(760.), view.clone());
    view
}

// ---- Tests ------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use std::time::Duration;

    fn report(path: &str) -> FileXRayReport {
        FileXRayReport {
            path: path.into(),
            logical_size: 1_048_576,
            allocated_size: 1_052_672,
            hardlink_count: 1,
            file_index: "0x0001000000012345".into(),
            volume_serial: "ABCD-1234".into(),
            attributes: vec!["Archive".into()],
            streams: vec![],
            zone_info: None,
            entropy: 7.912,
            entropy_label: "Looks compressed or encrypted".into(),
            byte_spectrum: [6.0; 16],
            null_byte_pct: 0.4,
            ascii_pct: 37.5,
            high_byte_pct: 50.0,
            sha256: "ab".repeat(32),
            blake3: "cd".repeat(32),
            crc32: "deadbeef".into(),
            hashes_complete: true,
            elapsed_ms: 12.34,
        }
    }

    fn zone_report(path: &str) -> FileXRayReport {
        FileXRayReport {
            zone_info: Some(glint_core::xray::ZoneIdentifier {
                zone_id: 3,
                zone_label: "Internet".into(),
                host_url: Some("https://example.com/setup.exe".into()),
                referrer_url: None,
            }),
            streams: vec![
                NtfsStream {
                    name: ":Zone.Identifier:$DATA".into(),
                    clean_name: "Zone.Identifier".into(),
                    size: 120,
                    preview: Some("[ZoneTransfer]\nZoneId=3".into()),
                },
                NtfsStream {
                    name: ":notes:$DATA".into(),
                    clean_name: "notes".into(),
                    size: 4096,
                    preview: None,
                },
            ],
            ..report(path)
        }
    }

    struct Fake {
        report: Mutex<Result<FileXRayReport, String>>,
        signature: Signature,
        inspected: Mutex<Vec<bool>>,
        removed: Mutex<Vec<String>>,
        cancelled: AtomicUsize,
        /// The inspection waits for this (the cancel test).
        gate: Mutex<Option<async_channel::Receiver<()>>>,
    }

    impl Fake {
        fn new(report: FileXRayReport) -> Self {
            Self {
                report: Mutex::new(Ok(report)),
                signature: Signature {
                    state: "valid",
                    signer: Some("Contoso Ltd".into()),
                    issuer: Some("Contoso CA".into()),
                    ..Default::default()
                },
                inspected: Mutex::new(Vec::new()),
                removed: Mutex::new(Vec::new()),
                cancelled: AtomicUsize::new(0),
                gate: Mutex::new(None),
            }
        }
    }

    impl Backend for Fake {
        fn inspect(&self, _: String, full: bool) -> Fut<FileXRayReport> {
            self.inspected.lock().unwrap().push(full);
            let answer = self.report.lock().unwrap().clone();
            let gate = self.gate.lock().unwrap().clone();
            Box::pin(async move {
                if let Some(gate) = gate {
                    let _ = gate.recv().await;
                }
                answer
            })
        }
        fn cancel(&self) {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
        }
        fn signature(&self, _: String) -> Fut<Signature> {
            let s = self.signature.clone();
            Box::pin(async move { Ok(s) })
        }
        fn remove_stream(&self, _: String, stream: String) -> Fut<()> {
            self.removed.lock().unwrap().push(stream);
            Box::pin(async { Ok(()) })
        }
    }

    struct Host;

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full()
        }
    }

    fn window(cx: &mut TestAppContext) -> VisualTestContext {
        cx.update(testing::init);
        testing::open_window(cx, |_, cx| cx.new(|_| Host)).0
    }

    fn has(cx: &mut VisualTestContext, label: &str) -> bool {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn role_of(cx: &mut VisualTestContext, label: &str) -> Option<Role> {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).and_then(|element| element.role())
        })
    }

    fn settle(cx: &mut VisualTestContext, what: &str, mut done: impl FnMut(&mut VisualTestContext) -> bool) {
        for _ in 0..400 {
            cx.run_until_parked();
            if done(cx) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("never happened: {what}");
    }

    fn open_fake(cx: &mut VisualTestContext, fake: &Arc<Fake>, path: &str) -> Entity<XRayDialog> {
        let backend: Arc<dyn Backend> = fake.clone();
        let path = path.to_string();
        cx.update(|window, cx| open_with(window, cx, backend, path))
    }

    #[test]
    fn extensions_decide_what_is_checked() {
        assert!(signable("C:\\x\\setup.EXE"));
        assert!(signable("C:\\x\\run.ps1"));
        assert!(!signable("C:\\x\\notes.txt"));
        assert!(!signable("C:\\x.dir\\noext"));
        assert!(!signable("C:\\x\\.exe"), "a name that is only an extension isn't one");
    }

    #[test]
    fn a_pasted_checksum_is_compared_without_case_or_spaces() {
        let r = report("C:\\f.bin");
        assert_eq!(matched_algo(&r, &format!("  {}  ", "AB".repeat(32))), Some("SHA-256"));
        assert_eq!(matched_algo(&r, &"cd".repeat(32)), Some("BLAKE3"));
        assert_eq!(matched_algo(&r, "DEADBEEF"), Some("CRC32"));
        assert_eq!(matched_algo(&r, "nope"), None);
        assert_eq!(matched_algo(&r, "  "), None);
    }

    #[test]
    fn the_histogram_is_labelled_and_scaled_to_its_tallest_bar() {
        assert_eq!(bucket_range(0), "0x00–0x0F");
        assert_eq!(bucket_range(15), "0xF0–0xFF");
        assert_eq!(bar_height(50.0, 50.0), 100.0);
        assert_eq!(bar_height(25.0, 50.0), 50.0);
        assert_eq!(bar_height(0.0, 50.0), 3.0, "an empty bucket still shows");
        // Under 1 %, the scale doesn't blow tiny readings up.
        assert_eq!(bar_height(0.5, 0.5), 50.0);
    }

    #[test]
    fn signatures_are_described_as_the_typescript_did() {
        let _guard = testing::lock();
        let valid = describe_signature(&Signature {
            state: "valid",
            signer: Some("Contoso Ltd".into()),
            issuer: Some("Contoso CA".into()),
            catalog: true,
            ..Default::default()
        });
        assert_eq!(valid.text, "Signed by Contoso Ltd");
        assert_eq!(valid.tone, Tone::Good);
        assert_eq!(
            valid.details,
            [
                "Its certificate was issued by Contoso CA",
                "Signed in a Windows catalog, as Windows' own files are",
                "Checked offline: certificates aren't looked up as revoked"
            ]
        );
        let unsigned = describe_signature(&Signature {
            state: "unsigned",
            ..Default::default()
        });
        assert_eq!(unsigned.text, "Not signed");
        assert_eq!(unsigned.tone, Tone::Plain);
        assert_eq!(
            unsigned.details,
            ["No digital signature, in the file or in a Windows catalog"]
        );
        let invalid = describe_signature(&Signature {
            state: "invalid",
            signer: Some("Evil Corp".into()),
            detail: Some("The digest doesn't match".into()),
            ..Default::default()
        });
        assert_eq!(
            invalid.text,
            "Signature doesn't hold: changed after it was signed by Evil Corp"
        );
        assert_eq!(invalid.tone, Tone::Bad);
        assert!(invalid.details.contains(&"The digest doesn't match".to_string()));
        assert_eq!(
            describe_signature(&Signature {
                state: "expired",
                ..Default::default()
            })
            .text,
            "Signed, with a certificate that expired (and no timestamp)"
        );
        assert_eq!(
            describe_signature(&Signature {
                state: "unknown",
                ..Default::default()
            })
            .text,
            "Couldn't check the signature"
        );
    }

    #[gpui_kit::test]
    fn the_x_ray_shows_loading_then_every_section_labelled(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report("C:\\dl\\setup.exe")));
        open_fake(&mut visual, &fake, "C:\\dl\\setup.exe");
        visual.update(|window, cx| assert!(window.has_active_dialog(cx)));
        assert_eq!(role_of(&mut visual, "Reading the file…"), Some(Role::Status));
        settle(&mut visual, "the report", |cx| has(cx, "Checksums"));
        assert_eq!(role_of(&mut visual, "File X-Ray: setup.exe"), Some(Role::Group));
        let sha = "ab".repeat(32);
        assert_eq!(role_of(&mut visual, &format!("SHA-256: {sha}")), Some(Role::Group));
        assert_eq!(
            role_of(&mut visual, &format!("BLAKE3: {}", "cd".repeat(32))),
            Some(Role::Group)
        );
        assert_eq!(role_of(&mut visual, "CRC32: deadbeef"), Some(Role::Group));
        assert!(has(&mut visual, "H = 7.912 / 8.000 bits/B"));
        assert!(has(&mut visual, "0x00–0x0F: 6.0% of bytes"));
        assert!(has(&mut visual, "Logical size: 1.0 MB (1,048,576 B)"));
        assert!(has(&mut visual, "NTFS hardlinks: 1"));
        assert!(has(&mut visual, "MFT File ID: 0x0001000000012345"));
        assert!(has(&mut visual, "Attributes: Archive"));
        assert!(has(&mut visual, "Hidden streams (0)"));
        assert!(!has(&mut visual, "Reading the file…"));
        // The signature came beside it.
        settle(&mut visual, "the signature", |cx| has(cx, "Signed by Contoso Ltd"));
        assert!(!has(&mut visual, "Marked as downloaded · Internet"));
        assert_eq!(*fake.inspected.lock().unwrap(), [false]);
    }

    #[gpui_kit::test]
    fn a_pasted_checksum_says_whether_it_matches(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report("C:\\f.txt")));
        let view = open_fake(&mut visual, &fake, "C:\\f.txt");
        settle(&mut visual, "the report", |cx| has(cx, "Checksums"));
        let input = view.read_with(&visual, |v, _| v.expected.clone());
        visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("AB".repeat(32), window, cx)));
        settle(&mut visual, "the match", |cx| has(cx, "SHA-256 matches"));
        visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("1234", window, cx)));
        settle(&mut visual, "the mismatch", |cx| has(cx, "Doesn't match"));
        visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("", window, cx)));
        settle(&mut visual, "no verdict", |cx| !has(cx, "Doesn't match"));
    }

    #[gpui_kit::test]
    fn a_partial_hash_offers_the_whole_file_and_will_not_say_it_mismatches(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let mut partial = report("C:\\big.iso");
        partial.hashes_complete = false;
        let fake = Arc::new(Fake::new(partial));
        let view = open_fake(&mut visual, &fake, "C:\\big.iso");
        settle(&mut visual, "the report", |cx| has(cx, "Checksums"));
        let input = view.read_with(&visual, |v, _| v.expected.clone());
        visual.update(|window, cx| input.update(cx, |input, cx| input.set_value("1234", window, cx)));
        settle(&mut visual, "not checked", |cx| {
            has(cx, "Not checked: hash the whole file first")
        });
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("xray-hash-all", cx);
        });
        settle(&mut visual, "the second inspection", |_| {
            fake.inspected.lock().unwrap().len() == 2
        });
        assert_eq!(*fake.inspected.lock().unwrap(), [false, true]);
        assert_eq!(role_of(&mut visual, "Hashing the whole file…"), Some(Role::Status));
    }

    #[gpui_kit::test]
    fn a_failure_is_an_alert_and_refresh_reads_again(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report("C:\\x")));
        *fake.report.lock().unwrap() = Err("Access is denied.".into());
        open_fake(&mut visual, &fake, "C:\\x");
        settle(&mut visual, "the error", |cx| {
            has(cx, "Couldn't X-Ray this item. Access is denied.")
        });
        assert_eq!(
            role_of(&mut visual, "Couldn't X-Ray this item. Access is denied."),
            Some(Role::Alert)
        );
        *fake.report.lock().unwrap() = Ok(report("C:\\x"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("xray-refresh", cx);
        });
        settle(&mut visual, "the retry", |cx| has(cx, "Checksums"));
        assert_eq!(fake.inspected.lock().unwrap().len(), 2);
    }

    #[gpui_kit::test]
    fn the_download_mark_is_shown_and_unblock_asks_first(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(zone_report("C:\\dl\\setup.exe")));
        open_fake(&mut visual, &fake, "C:\\dl\\setup.exe");
        settle(&mut visual, "the banner", |cx| {
            has(cx, "Marked as downloaded · Internet")
        });
        assert_eq!(
            role_of(&mut visual, "Marked as downloaded · Internet"),
            Some(Role::Group)
        );
        assert!(has(&mut visual, "Source URL: https://example.com/setup.exe"));
        assert!(has(&mut visual, "Hidden streams (2)"));
        assert!(has(&mut visual, ":Zone.Identifier · 120 B"));
        assert!(has(&mut visual, ":notes · 4.0 KB"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("xray-unblock", cx);
        });
        visual.run_until_parked();
        assert!(fake.removed.lock().unwrap().is_empty(), "asked first");
        settle(&mut visual, "the question", |cx| has(cx, "Unblock file"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("confirm-ok", cx);
        });
        settle(&mut visual, "the removal", |_| !fake.removed.lock().unwrap().is_empty());
        assert_eq!(*fake.removed.lock().unwrap(), ["Zone.Identifier"]);
        // The file is read again after it.
        settle(&mut visual, "the re-read", |_| {
            fake.inspected.lock().unwrap().len() == 2
        });
    }

    #[gpui_kit::test]
    fn deleting_a_stream_asks_with_a_danger_button(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(zone_report("C:\\dl\\setup.exe")));
        let view = open_fake(&mut visual, &fake, "C:\\dl\\setup.exe");
        settle(&mut visual, "the streams", |cx| has(cx, ":notes · 4.0 KB"));
        assert!(has(&mut visual, "Delete the stream :notes"));
        // The button sits below the dialog's fold in a 700 px window, so the question is asked as its click does.
        visual.update(|window, cx| {
            view.update(cx, |this, cx| this.ask_remove("notes".into(), window, cx));
        });
        settle(&mut visual, "the question", |cx| has(cx, "Delete stream"));
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.click("confirm-cancel", cx);
        });
        visual.run_until_parked();
        assert!(fake.removed.lock().unwrap().is_empty(), "Cancel deletes nothing");
    }

    #[gpui_kit::test]
    fn closing_while_it_hashes_cancels(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(r"C:\big.iso")));
        let (_tx, rx) = async_channel::bounded::<()>(1);
        *fake.gate.lock().unwrap() = Some(rx);
        let view = open_fake(&mut visual, &fake, r"C:\big.iso");
        visual.run_until_parked();
        assert_eq!(role_of(&mut visual, "Reading the file…"), Some(Role::Status));
        drop(view);
        visual.update(|window, cx| window.close_dialog(cx));
        settle(&mut visual, "the cancel", |_| {
            fake.cancelled.load(Ordering::SeqCst) >= 1
        });
    }

    #[gpui_kit::test]
    fn escape_closes_it(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        let fake = Arc::new(Fake::new(report(r"C:\f.bin")));
        open_fake(&mut visual, &fake, r"C:\f.bin");
        settle(&mut visual, "the report", |cx| has(cx, "Checksums"));
        visual.update(|window, cx| {
            assert!(window.focused(cx).is_some(), "focus moved into the dialog");
            window.render_frame(cx);
            window.press("escape", cx);
        });
        visual.run_until_parked();
        visual.update(|window, cx| assert!(!window.has_active_dialog(cx)));
    }

    #[gpui_kit::test]
    fn arabic_names_and_the_mirrored_layout_render(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let mut visual = window(cx);
        testing::set_rtl(true);
        let path = "C:\\تنزيلات\\برنامج.exe";
        let fake = Arc::new(Fake::new(zone_report(path)));
        open_fake(&mut visual, &fake, path);
        settle(&mut visual, "the Arabic report", |cx| has(cx, "File X-Ray: برنامج.exe"));
        settle(&mut visual, "the banner", |cx| {
            has(cx, "Marked as downloaded · Internet")
        });
        assert_eq!(role_of(&mut visual, "File X-Ray: برنامج.exe"), Some(Role::Group));
        testing::set_rtl(false);
    }
}
