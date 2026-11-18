//! What the engine finds out about one file or folder, a part at a time (`glint_core::aware::probe`).
//!
//! Every field of [`Probed`] is an `Option`: `None` means "not known yet", and what isn't known is never shown
//! (docs/gpui/aware/PLAN.md, R1). [`Want`] names the parts to ask for; [`Part`] is one answer.

use super::hash::{Sums, SumsWant};

/// What has been found out about one file or folder so far.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Probed {
    /// The apps that have it open; `Some(empty)` when nobody has.
    pub locked_by: Option<Vec<String>>,
    /// Where it came from, when Windows marked it as downloaded.
    pub origin: Option<Origin>,
    /// Who signed it (programs and installers).
    pub signer: Option<Signer>,
    /// A picture's size, a video's or a song's length.
    pub media: Option<Media>,
    /// A text file's lines.
    pub text: Option<Text>,
    /// A PDF's pages.
    pub pages: Option<u32>,
    /// What is in a folder, all the way down.
    pub inside: Option<Inside>,
    /// Other files on this PC with the same bytes; `Some(empty)` when none was found.
    pub twins: Option<Vec<String>>,
    /// What a shortcut points to.
    pub target: Option<String>,
    /// The name of the app that opens it ("Photos").
    pub opens_with: Option<String>,
    /// For an archive: the folder beside it that has its name, where it was extracted to; `Some(None)` when
    /// there is none.
    pub extracted: Option<Option<String>>,
    /// The checksums made of it, to compare with one copied from a download page (only what was asked for).
    pub sums: Option<Sums>,
}

/// One answer about a file or folder.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    LockedBy(Vec<String>),
    Origin(Origin),
    Signer(Signer),
    Media(Media),
    Text(Text),
    Pages(u32),
    Inside(Inside),
    Twins(Vec<String>),
    Target(String),
    OpensWith(String),
    Extracted(Option<String>),
    Sums(Sums),
}

/// Where a downloaded file came from (its Mark of the Web).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Origin {
    /// The site ("github.com"), "" when the mark doesn't say.
    pub host: String,
    /// Windows treats it as from the internet (zone 3 or 4): programs warn before they run.
    pub blocked: bool,
}

/// Who vouches for a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Signer {
    /// Signed, and the signature holds: the signer's name.
    Signed(String),
    /// Not signed at all.
    Unsigned,
}

/// A picture's or a video's size in pixels, and a video's or a song's length. Zero for what isn't known.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Media {
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
}

/// A text file's lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Text {
    pub lines: u64,
    /// It isn't text after all (it holds zero bytes): `lines` says nothing.
    pub binary: bool,
}

/// What is in a folder, all the way down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Inside {
    /// Files below it (0 when they weren't counted).
    pub files: u64,
    /// Folders below it (0 when they weren't counted).
    pub folders: u64,
    /// Everything below it, in bytes.
    pub bytes: u64,
    /// The bytes of what a build left in it and can make again (`target`, `node_modules`…).
    pub junk: u64,
    /// The counting has finished: the numbers won't grow.
    pub done: bool,
    /// The instant search index answered (at once), rather than a walk of the folder.
    pub from_index: bool,
}

/// Which parts to ask for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub locked_by: bool,
    pub origin: bool,
    pub signer: bool,
    pub media: bool,
    pub text: bool,
    pub pages: bool,
    pub inside: bool,
    pub twins: bool,
    pub target: bool,
    pub opens_with: bool,
    pub extracted: bool,
    pub sums: SumsWant,
}

impl Probed {
    /// Takes in one answer. An `Inside` replaces the one before it: a walk reports as it goes.
    pub fn take(&mut self, part: Part) {
        match part {
            Part::LockedBy(names) => self.locked_by = Some(names),
            Part::Origin(origin) => self.origin = Some(origin),
            Part::Signer(signer) => self.signer = Some(signer),
            Part::Media(media) => self.media = Some(media),
            Part::Text(text) => self.text = Some(text),
            Part::Pages(pages) => self.pages = Some(pages),
            Part::Inside(inside) => self.inside = Some(inside),
            Part::Twins(paths) => self.twins = Some(paths),
            Part::Target(target) => self.target = Some(target),
            Part::OpensWith(app) => self.opens_with = Some(app),
            Part::Extracted(folder) => self.extracted = Some(folder),
            Part::Sums(made) => self.sums.get_or_insert_with(Sums::default).merge(made),
        }
    }
}

impl Want {
    /// What is still to ask, given what `have` already holds.
    pub fn minus(self, have: &Probed) -> Want {
        Want {
            locked_by: self.locked_by && have.locked_by.is_none(),
            origin: self.origin && have.origin.is_none(),
            signer: self.signer && have.signer.is_none(),
            media: self.media && have.media.is_none(),
            text: self.text && have.text.is_none(),
            pages: self.pages && have.pages.is_none(),
            inside: self.inside && have.inside.is_none(),
            twins: self.twins && have.twins.is_none(),
            target: self.target && have.target.is_none(),
            opens_with: self.opens_with && have.opens_with.is_none(),
            extracted: self.extracted && have.extracted.is_none(),
            sums: self.sums.minus(have.sums.as_ref()),
        }
    }

    /// What this asks for that `asked` doesn't: what has been asked already isn't asked again.
    pub fn without(self, asked: Want) -> Want {
        Want {
            locked_by: self.locked_by && !asked.locked_by,
            origin: self.origin && !asked.origin,
            signer: self.signer && !asked.signer,
            media: self.media && !asked.media,
            text: self.text && !asked.text,
            pages: self.pages && !asked.pages,
            inside: self.inside && !asked.inside,
            twins: self.twins && !asked.twins,
            target: self.target && !asked.target,
            opens_with: self.opens_with && !asked.opens_with,
            extracted: self.extracted && !asked.extracted,
            sums: self.sums.without(asked.sums),
        }
    }

    /// Everything either asks for.
    pub fn and(self, other: Want) -> Want {
        Want {
            locked_by: self.locked_by || other.locked_by,
            origin: self.origin || other.origin,
            signer: self.signer || other.signer,
            media: self.media || other.media,
            text: self.text || other.text,
            pages: self.pages || other.pages,
            inside: self.inside || other.inside,
            twins: self.twins || other.twins,
            target: self.target || other.target,
            opens_with: self.opens_with || other.opens_with,
            extracted: self.extracted || other.extracted,
            sums: self.sums.and(other.sums),
        }
    }

    /// Nothing is asked for.
    pub fn is_empty(self) -> bool {
        self == Want::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn everything() -> Want {
        Want {
            locked_by: true,
            origin: true,
            signer: true,
            media: true,
            text: true,
            pages: true,
            inside: true,
            twins: true,
            target: true,
            opens_with: true,
            extracted: true,
            sums: SumsWant { sha256: true, sha512: true, blake3: true },
        }
    }

    #[test]
    fn an_answer_is_kept_and_a_later_inside_replaces_the_one_before() {
        let mut probed = Probed::default();
        probed.take(Part::LockedBy(vec!["Word".into()]));
        probed.take(Part::Pages(3));
        let inside = |files, done| Inside { files, folders: 1, bytes: 10, junk: 0, done, from_index: false };
        probed.take(Part::Inside(inside(5, false)));
        probed.take(Part::Inside(inside(9, true)));
        assert_eq!(probed.locked_by.as_deref(), Some(&["Word".to_string()][..]));
        assert_eq!(probed.pages, Some(3));
        assert_eq!(probed.inside, Some(inside(9, true)));
        assert!(probed.origin.is_none(), "what wasn't answered stays unknown");
    }

    #[test]
    fn what_is_known_is_not_asked_for_again() {
        let mut probed = Probed::default();
        assert_eq!(everything().minus(&probed), everything());
        probed.take(Part::Twins(Vec::new()));
        probed.take(Part::Text(Text { lines: 0, binary: true }));
        let left = everything().minus(&probed);
        assert!(!left.twins && !left.text, "an empty answer is an answer");
        assert!(left.origin && left.media);
    }

    #[test]
    fn wants_add_up_and_take_away() {
        let media = Want { media: true, ..Want::default() };
        let text = Want { text: true, ..Want::default() };
        assert!(Want::default().is_empty() && !media.is_empty());
        assert_eq!(media.and(text), Want { media: true, text: true, ..Want::default() });
        assert_eq!(media.and(text).without(media), text);
        assert!(media.without(media).is_empty());
    }
}
