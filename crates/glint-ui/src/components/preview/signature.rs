//! Who signed a program, and whether Windows trusts it (SignatureChip.tsx).
//!
//! | TypeScript                  | Here                                                                      |
//! |-----------------------------|---------------------------------------------------------------------------|
//! | `Signature` (`lib/fs`)      | [`Signature`], [`State`]                                                  |
//! | `describeSignature`         | [`describe`]                                                              |
//! | `useSignature`, `verifySignature` | [`SignatureChip`] asks [`Verify`] (glint-core's `verify_signature`, in the background) |
//! | `SignatureChip`             | [`SignatureChip`], a view of its own                                      |
//!
//! # Where this goes (for the coordinator)
//!
//! `hex.rs`, at the `PORT ME(coordinator): the signature chip` after the `pe-subsystem` chip, in the row of
//! chips of a program's header: `.child(cx.new(|cx| SignatureChip::new(path.clone(), cx)))` (the chip is
//! `inline-flex`, `min-w-0`, so it truncates in a wrapping row). The chip needs no event. A program inside an
//! archive has no path Windows can check; the TypeScript asked all the same, and so does this.

use super::sources::Fut;
use crate::kit::prelude::*;
use crate::{core_bridge, t, theme};
use glint_model::format::format_full_date;
use std::rc::Rc;

/// What a signature check found (`Signature.state`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    Valid,
    Unsigned,
    /// Changed since it was signed, or a damaged signature.
    Invalid,
    /// Signed with a certificate Windows doesn't trust.
    Untrusted,
    Expired,
    Revoked,
    #[default]
    Unknown,
}

/// A file's signature, as glint-core reports it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Signature {
    pub state: State,
    /// Signed in a Windows catalog rather than in the file itself.
    pub catalog: bool,
    pub signer: Option<String>,
    pub issuer: Option<String>,
    /// When it was signed, by its timestamp (ms).
    pub signed_at: Option<i64>,
    /// What Windows said, when it doesn't hold.
    pub detail: Option<String>,
}

impl Signature {
    /// glint-core's answer as this module's.
    pub fn from_core(core: glint_core::sign::Signature) -> Self {
        Signature {
            state: match core.state {
                "valid" => State::Valid,
                "unsigned" => State::Unsigned,
                "invalid" => State::Invalid,
                "untrusted" => State::Untrusted,
                "expired" => State::Expired,
                "revoked" => State::Revoked,
                _ => State::Unknown,
            },
            catalog: core.catalog,
            signer: core.signer,
            issuer: core.issuer,
            signed_at: core.signed_at,
            detail: core.detail,
        }
    }
}

/// How the chip is colored (`Tone`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Warn,
    Plain,
}

/// What a check found, in words: a short line, and the rest for a tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct Described {
    pub text: SharedString,
    pub tone: Tone,
    pub details: Vec<SharedString>,
}

/// `describeSignature`.
pub fn describe(signature: &Signature) -> Described {
    let signer = signature.signer.clone().filter(|s| !s.is_empty()).map(isolate);
    let (text, tone) = match signature.state {
        State::Valid => (
            match &signer {
                Some(name) => t!("Signed by {signer}", signer = name.clone()),
                None => t!("Signed"),
            },
            Tone::Good,
        ),
        State::Unsigned => {
            return Described {
                text: t!("Not signed"),
                tone: Tone::Plain,
                details: vec![t!("No digital signature, in the file or in a Windows catalog")],
            }
        }
        State::Invalid => (
            match &signer {
                Some(name) => t!(
                    "Signature doesn't hold: changed after it was signed by {signer}",
                    signer = name.clone()
                ),
                None => t!("Signature doesn't hold: changed after it was signed"),
            },
            Tone::Bad,
        ),
        State::Untrusted => (
            match &signer {
                Some(name) => t!(
                    "Signed by {signer}, but not with a certificate Windows trusts",
                    signer = name.clone()
                ),
                None => t!("Signed, but not with a certificate Windows trusts"),
            },
            Tone::Warn,
        ),
        State::Expired => (
            match &signer {
                Some(name) => t!(
                    "Signed by {signer}, with a certificate that expired (and no timestamp)",
                    signer = name.clone()
                ),
                None => t!("Signed, with a certificate that expired (and no timestamp)"),
            },
            Tone::Warn,
        ),
        State::Revoked => (
            match &signer {
                Some(name) => t!(
                    "Signed by {signer}, with a certificate that was revoked",
                    signer = name.clone()
                ),
                None => t!("Signed, with a certificate that was revoked"),
            },
            Tone::Bad,
        ),
        State::Unknown => (t!("Couldn't check the signature"), Tone::Plain),
    };
    let mut details: Vec<SharedString> = Vec::new();
    if let Some(issuer) = signature.issuer.as_ref().filter(|i| !i.is_empty()) {
        details.push(t!(
            "Its certificate was issued by {issuer}",
            issuer = isolate(issuer.clone())
        ));
    }
    if signature.catalog {
        details.push(t!("Signed in a Windows catalog, as Windows' own files are"));
    }
    if let Some(at) = signature.signed_at.filter(|&at| at != 0) {
        details.push(t!("Signed {date}", date = format_full_date(at)));
    }
    if signature.state != State::Valid {
        if let Some(detail) = signature.detail.clone().filter(|d| !d.is_empty()) {
            details.push(detail.into());
        }
    }
    details.push(t!("Checked offline: certificates aren't looked up as revoked"));
    Described { text, tone, details }
}

/// Whatever checks a file's signature: glint-core in the app, a fake in tests.
#[derive(Clone)]
pub struct Verify(pub CheckFn);

/// Checks a file's signature.
pub type CheckFn = Rc<dyn Fn(&str) -> Fut<Result<Signature, String>>>;

impl Verify {
    /// glint-core's `verify_signature` (WinVerifyTrust, offline), on core's runtime.
    pub fn core() -> Self {
        Verify(Rc::new(|path| {
            let running = core_bridge::run(glint_core::sign::verify_signature(path.to_string()));
            Box::pin(async move { running.await.and_then(|inner| inner).map(Signature::from_core) })
        }))
    }
}

struct Shared(Verify);

impl Global for Shared {}

/// Sets what the chips ask (tests; the app uses [`Verify::core`] without this).
pub fn init_with(cx: &mut App, verify: Verify) {
    cx.set_global(Shared(verify));
}

fn verify(cx: &App) -> Verify {
    cx.try_global::<Shared>()
        .map_or_else(Verify::core, |shared| shared.0.clone())
}

enum Check {
    /// Windows is checking it.
    Checking,
    Done(Signature),
}

/// A chip saying whether the file's signature holds, and who signed it (`SignatureChip`).
pub struct SignatureChip {
    path: String,
    check: Check,
    _task: Task<()>,
}

impl SignatureChip {
    /// Checks `path` in the background; the chip says so until Windows answers. A check that fails to run is
    /// "Couldn't check the signature".
    pub fn new(path: impl Into<String>, cx: &mut Context<Self>) -> Self {
        let path = path.into();
        let running = (verify(cx).0)(&path);
        let task = cx.spawn(async move |this, cx| {
            let signature = running.await.unwrap_or_default();
            this.update(cx, |this, cx| {
                this.check = Check::Done(signature);
                cx.notify();
            })
            .ok();
        });
        SignatureChip {
            path,
            check: Check::Checking,
            _task: task,
        }
    }

    /// What the check found, once it has.
    pub fn signature(&self) -> Option<&Signature> {
        match &self.check {
            Check::Done(signature) => Some(signature),
            Check::Checking => None,
        }
    }

    /// The file checked.
    pub fn path(&self) -> &str {
        &self.path
    }
}

impl Render for SignatureChip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = theme::tokens(cx).clone();
        let pill = row_start()
            .id("signature-chip")
            .flex_none()
            .min_w_0()
            .items_center()
            .gap(space(1.))
            .px(space(2.))
            .py(space(0.5))
            .rounded(theme::size::RADIUS)
            .text_size(theme::size::XXS)
            .test_support();
        match &self.check {
            Check::Checking => {
                let what = t!("Checking the signature…");
                pill.role(Role::Status)
                    .aria_label(what.clone())
                    .border_1()
                    .border_color(t.border)
                    .bg(t.background)
                    .text_color(t.muted_foreground)
                    .child(Spinner::new().color(t.muted_foreground))
                    .child(what)
                    .into_any_element()
            }
            Check::Done(signature) => {
                let described = describe(signature);
                let (name, ground, color, edge) = match described.tone {
                    Tone::Good => (IconName::ShieldCheck, t.success.opacity(0.15), t.success, None),
                    Tone::Bad => (IconName::ShieldX, t.destructive.opacity(0.15), t.destructive, None),
                    Tone::Warn => (IconName::ShieldAlert, t.warning.opacity(0.15), t.warning, None),
                    Tone::Plain => (
                        IconName::ShieldQuestionMark,
                        t.background,
                        t.muted_foreground,
                        Some(t.border),
                    ),
                };
                let tip = std::iter::once(described.text.to_string())
                    .chain(described.details.iter().map(ToString::to_string))
                    .collect::<Vec<_>>()
                    .join("\n");
                let tip = SharedString::from(tip);
                pill.role(Role::Status)
                    .aria_label(described.text.clone())
                    .font_weight(FontWeight::MEDIUM)
                    .bg(ground)
                    .text_color(color)
                    .when_some(edge, |this, edge| this.border_1().border_color(edge))
                    .tooltip(move |window, cx| crate::kit::tooltip(tip.clone(), window, cx))
                    .child(icon(name).size_3().flex_none())
                    .child(div().min_w_0().truncate().child(described.text))
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;
    use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
    use gpui_kit::TestAppContext;
    use std::time::Duration;

    fn signed(state: State) -> Signature {
        Signature {
            state,
            signer: Some("شركة النور".into()),
            issuer: Some("DigiCert".into()),
            ..Signature::default()
        }
    }

    #[test]
    fn each_state_is_told_in_the_typescripts_words() {
        let by = isolate("شركة النور");
        let valid = describe(&signed(State::Valid));
        assert_eq!(valid.text, format!("Signed by {by}"));
        assert_eq!(valid.tone, Tone::Good);
        let invalid = describe(&signed(State::Invalid));
        assert_eq!(
            invalid.text,
            format!("Signature doesn't hold: changed after it was signed by {by}")
        );
        assert_eq!(invalid.tone, Tone::Bad);
        assert_eq!(
            describe(&signed(State::Untrusted)).text,
            format!("Signed by {by}, but not with a certificate Windows trusts")
        );
        assert_eq!(
            describe(&signed(State::Expired)).text,
            format!("Signed by {by}, with a certificate that expired (and no timestamp)")
        );
        let revoked = describe(&signed(State::Revoked));
        assert_eq!(
            revoked.text,
            format!("Signed by {by}, with a certificate that was revoked")
        );
        assert_eq!(revoked.tone, Tone::Bad);
        assert_eq!(
            describe(&Signature {
                state: State::Valid,
                ..Signature::default()
            })
            .text,
            "Signed"
        );
        let unsigned = describe(&Signature {
            state: State::Unsigned,
            ..Signature::default()
        });
        assert_eq!(unsigned.text, "Not signed");
        assert_eq!(
            unsigned.details,
            vec!["No digital signature, in the file or in a Windows catalog"]
        );
        assert_eq!(describe(&Signature::default()).text, "Couldn't check the signature");
    }

    #[test]
    fn the_details_name_the_issuer_the_catalog_the_time_and_what_windows_said() {
        let described = describe(&Signature {
            state: State::Untrusted,
            catalog: true,
            signer: None,
            issuer: Some("Home CA".into()),
            signed_at: Some(1_794_000_000_000),
            detail: Some("A certificate chain processed, but terminated in a root certificate".into()),
        });
        assert_eq!(described.text, "Signed, but not with a certificate Windows trusts");
        let details: Vec<String> = described.details.iter().map(ToString::to_string).collect();
        assert_eq!(details[0], "Its certificate was issued by Home CA");
        assert_eq!(details[1], "Signed in a Windows catalog, as Windows' own files are");
        assert!(details[2].starts_with("Signed "), "{}", details[2]);
        assert_eq!(
            details[3],
            "A certificate chain processed, but terminated in a root certificate"
        );
        assert_eq!(details[4], "Checked offline: certificates aren't looked up as revoked");
        // A valid signature has nothing to explain.
        let valid = describe(&Signature {
            state: State::Valid,
            detail: Some("x".into()),
            ..Signature::default()
        });
        assert!(!valid.details.iter().any(|d| d == "x"));
    }

    #[test]
    fn glint_cores_answer_is_read_by_its_state_names() {
        let core = glint_core::sign::Signature {
            state: "untrusted",
            catalog: true,
            signer: Some("a".into()),
            ..Default::default()
        };
        let read = Signature::from_core(core);
        assert_eq!(read.state, State::Untrusted);
        assert!(read.catalog);
        let other = glint_core::sign::Signature {
            state: "surprise",
            ..Default::default()
        };
        assert_eq!(Signature::from_core(other).state, State::Unknown);
    }

    fn chip(
        cx: &mut TestAppContext,
        answer: Result<Signature, String>,
        rtl: bool,
    ) -> (Entity<SignatureChip>, gpui_kit::VisualTestContext) {
        cx.update(testing::init);
        testing::set_rtl(rtl);
        cx.update(|cx| {
            init_with(
                cx,
                Verify(Rc::new(move |_| {
                    let answer = answer.clone();
                    Box::pin(async move { answer })
                })),
            )
        });
        let (visual, view) = testing::open_window(cx, |_, cx| cx.new(|cx| SignatureChip::new(r"C:pp.exe", cx)));
        (view, visual)
    }

    fn has(visual: &mut gpui_kit::VisualTestContext, label: &str) -> bool {
        visual.update(|window, cx| {
            window.render_frame(cx);
            window.try_find_by_label(label).is_some()
        })
    }

    fn settle(view: &Entity<SignatureChip>, visual: &mut gpui_kit::VisualTestContext) {
        for _ in 0..200 {
            visual.run_until_parked();
            if view.read_with(visual, |v, _| v.signature().is_some()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[gpui_kit::test]
    fn the_chip_says_it_is_checking_then_who_signed(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (view, mut visual) = chip(cx, Ok(signed(State::Valid)), true);
        // Before the first turn of the executor, Windows is still checking.
        assert!(has(&mut visual, "Checking the signature…") || view.read_with(&visual, |v, _| v.signature().is_some()));
        settle(&view, &mut visual);
        assert_eq!(view.read_with(&visual, |v, _| v.path().to_string()), r"C:pp.exe");
        assert!(has(&mut visual, &format!("Signed by {}", isolate("شركة النور"))));
        assert!(!has(&mut visual, "Checking the signature…"));
        testing::set_rtl(false);
    }

    #[gpui_kit::test]
    fn a_check_that_fails_to_run_is_a_chip_that_says_so(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (view, mut visual) = chip(cx, Err("core is gone".into()), false);
        settle(&view, &mut visual);
        assert!(has(&mut visual, "Couldn't check the signature"));
    }

    #[gpui_kit::test]
    fn an_unsigned_program_is_a_plain_chip(cx: &mut TestAppContext) {
        let _guard = testing::lock();
        let (view, mut visual) = chip(
            cx,
            Ok(Signature {
                state: State::Unsigned,
                ..Signature::default()
            }),
            false,
        );
        settle(&view, &mut visual);
        assert!(has(&mut visual, "Not signed"));
    }
}
