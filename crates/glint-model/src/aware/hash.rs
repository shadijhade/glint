//! A checksum copied from a download page, and whether a file matches it.
//!
//! The clipboard may hold the checksum a site gives for a download. [`copied_hash`] recognises one by its length,
//! [`Sums`] holds what Glint worked out about a file, and [`verdict`] says whether the two agree (docs/gpui/aware
//! plan, C2). Pure: the file is read by `glint_core::checksums`.

/// How big a file may be before Glint won't read it only because the pointer rests on it or it is selected:
/// 512 MiB. Past this, reading it for a checksum takes seconds of the disk's time that nobody asked for.
pub const HASH_CAP: u64 = 512 * 1024 * 1024;

/// A way of making a checksum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algo {
    Md5,
    Sha1,
    Sha256,
    Sha512,
    Blake3,
}

impl Algo {
    /// As a person writes it.
    pub fn name(self) -> &'static str {
        match self {
            Algo::Md5 => "MD5",
            Algo::Sha1 => "SHA-1",
            Algo::Sha256 => "SHA-256",
            Algo::Sha512 => "SHA-512",
            Algo::Blake3 => "BLAKE3",
        }
    }

    /// How many hexadecimal characters its checksum has.
    pub fn hex_len(self) -> usize {
        match self {
            Algo::Md5 => 32,
            Algo::Sha1 => 40,
            Algo::Sha256 | Algo::Blake3 => 64,
            Algo::Sha512 => 128,
        }
    }

    /// Whether Glint can make this checksum itself. MD5 and SHA-1 aren't made by any hashing crate the engine
    /// depends on directly, so a copied MD5 or SHA-1 is recognised but a file is never compared with it.
    pub fn can_make(self) -> bool {
        matches!(self, Algo::Sha256 | Algo::Sha512 | Algo::Blake3)
    }
}

/// A checksum found on the clipboard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CopiedHash {
    /// The checksum, in lower case.
    pub hex: String,
}

impl CopiedHash {
    /// The ways it could have been made, from its length: 32 characters is MD5, 40 SHA-1, 128 SHA-512, and 64
    /// SHA-256 or BLAKE3 (a file that matches either is said to match it).
    pub fn candidates(&self) -> &'static [Algo] {
        match self.hex.len() {
            32 => &[Algo::Md5],
            40 => &[Algo::Sha1],
            64 => &[Algo::Sha256, Algo::Blake3],
            128 => &[Algo::Sha512],
            _ => &[],
        }
    }

    /// The checksums of a file worth making to compare it with this one: none when no candidate can be made.
    pub fn sums_wanted(&self) -> SumsWant {
        let mut want = SumsWant::default();
        for algo in self.candidates() {
            match algo {
                Algo::Sha256 => want.sha256 = true,
                Algo::Sha512 => want.sha512 = true,
                Algo::Blake3 => want.blake3 = true,
                Algo::Md5 | Algo::Sha1 => {}
            }
        }
        want
    }
}

/// The checksum on the clipboard, when the text is one: a single line whose first word is 32, 40, 64 or 128
/// hexadecimal characters (what `sha256sum` and `certutil` print, with or without the file's name after it).
pub fn copied_hash(text: &str) -> Option<CopiedHash> {
    let text = text.trim();
    if text.is_empty() || text.len() > 512 || text.contains(['\n', '\r']) {
        return None;
    }
    let word = text.split_whitespace().next()?;
    let known = [Algo::Md5, Algo::Sha1, Algo::Sha256, Algo::Sha512]
        .iter()
        .any(|algo| algo.hex_len() == word.len());
    (known && word.bytes().all(|b| b.is_ascii_hexdigit())).then(|| CopiedHash {
        hex: word.to_ascii_lowercase(),
    })
}

/// Which checksums of a file to make (one pass over the file makes them all).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SumsWant {
    pub sha256: bool,
    pub sha512: bool,
    pub blake3: bool,
}

impl SumsWant {
    /// What is still to make, given what is known.
    pub fn minus(self, have: Option<&Sums>) -> SumsWant {
        let have = have.cloned().unwrap_or_default();
        SumsWant {
            sha256: self.sha256 && have.sha256.is_none(),
            sha512: self.sha512 && have.sha512.is_none(),
            blake3: self.blake3 && have.blake3.is_none(),
        }
    }

    /// What this wants that `asked` doesn't.
    pub fn without(self, asked: SumsWant) -> SumsWant {
        SumsWant {
            sha256: self.sha256 && !asked.sha256,
            sha512: self.sha512 && !asked.sha512,
            blake3: self.blake3 && !asked.blake3,
        }
    }

    /// Everything either wants.
    pub fn and(self, other: SumsWant) -> SumsWant {
        SumsWant {
            sha256: self.sha256 || other.sha256,
            sha512: self.sha512 || other.sha512,
            blake3: self.blake3 || other.blake3,
        }
    }

    /// Nothing is wanted.
    pub fn is_empty(self) -> bool {
        self == SumsWant::default()
    }
}

/// The checksums made of one file, in lower-case hexadecimal.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sums {
    pub sha256: Option<String>,
    pub sha512: Option<String>,
    pub blake3: Option<String>,
}

impl Sums {
    /// Takes in what `other` has made.
    pub fn merge(&mut self, other: Sums) {
        if other.sha256.is_some() {
            self.sha256 = other.sha256;
        }
        if other.sha512.is_some() {
            self.sha512 = other.sha512;
        }
        if other.blake3.is_some() {
            self.blake3 = other.blake3;
        }
    }

    /// The checksum made with `algo`, if it was.
    pub fn get(&self, algo: Algo) -> Option<&str> {
        match algo {
            Algo::Sha256 => self.sha256.as_deref(),
            Algo::Sha512 => self.sha512.as_deref(),
            Algo::Blake3 => self.blake3.as_deref(),
            Algo::Md5 | Algo::Sha1 => None,
        }
    }
}

/// Whether a file agrees with a copied checksum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The file's checksum made with this algorithm is the copied one.
    Match(Algo),
    /// The file has been checked with every way the copied checksum could have been made, and none agrees.
    Mismatch,
}

/// What `sums` says about `copied`: None until the file's checksums of every candidate Glint can make are in, and
/// always None for an MD5 or SHA-1 (it can't be compared, so nothing is claimed).
pub fn verdict(sums: &Sums, copied: &CopiedHash) -> Option<Verdict> {
    let makeable: Vec<Algo> = copied
        .candidates()
        .iter()
        .copied()
        .filter(|algo| algo.can_make())
        .collect();
    if makeable.is_empty() {
        return None;
    }
    for algo in &makeable {
        if sums.get(*algo).is_some_and(|sum| sum.eq_ignore_ascii_case(&copied.hex)) {
            return Some(Verdict::Match(*algo));
        }
    }
    makeable
        .iter()
        .all(|algo| sums.get(*algo).is_some())
        .then_some(Verdict::Mismatch)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA256_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    const BLAKE3_ABC: &str = "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85";

    #[test]
    fn a_checksum_is_known_by_its_length_and_its_characters() {
        let md5 = "d41d8cd98f00b204e9800998ecf8427e";
        let sha1 = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
        let sha512 = "ab".repeat(64);
        assert_eq!(copied_hash(md5).map(|h| h.candidates()), Some(&[Algo::Md5][..]));
        assert_eq!(copied_hash(sha1).map(|h| h.candidates()), Some(&[Algo::Sha1][..]));
        assert_eq!(
            copied_hash(SHA256_ABC).map(|h| h.candidates()),
            Some(&[Algo::Sha256, Algo::Blake3][..])
        );
        assert_eq!(copied_hash(&sha512).map(|h| h.candidates()), Some(&[Algo::Sha512][..]));
        for not in [
            "",
            "hello",
            &SHA256_ABC[..63],
            &format!("{SHA256_ABC}0"),
            &SHA256_ABC.replace('b', "g"),
        ] {
            assert_eq!(copied_hash(not), None, "{not:?}");
        }
    }

    #[test]
    fn spaces_a_name_after_it_and_capitals_are_allowed_but_not_a_second_line() {
        let upper = SHA256_ABC.to_uppercase();
        assert_eq!(
            copied_hash(&format!("  {upper}\r\n")).map(|h| h.hex),
            Some(SHA256_ABC.to_string())
        );
        assert_eq!(
            copied_hash(&format!("{SHA256_ABC}  setup.exe")).map(|h| h.hex),
            Some(SHA256_ABC.to_string())
        );
        assert_eq!(copied_hash(&format!("{SHA256_ABC}\n{SHA256_ABC}")), None);
        assert_eq!(
            copied_hash(&format!("{SHA256_ABC}{}", " x".repeat(300))),
            None,
            "a long text is not a checksum"
        );
    }

    #[test]
    fn only_what_can_be_made_is_wanted_and_md5_is_not() {
        fn want(text: &str) -> SumsWant {
            copied_hash(text).map(|h| h.sums_wanted()).unwrap_or_default()
        }
        assert_eq!(
            want(SHA256_ABC),
            SumsWant {
                sha256: true,
                blake3: true,
                ..SumsWant::default()
            }
        );
        assert_eq!(
            want(&"ab".repeat(64)),
            SumsWant {
                sha512: true,
                ..SumsWant::default()
            }
        );
        assert!(want("d41d8cd98f00b204e9800998ecf8427e").is_empty());
        assert!(want("da39a3ee5e6b4b0d3255bfef95601890afd80709").is_empty());
    }

    #[test]
    fn a_file_matches_by_either_way_a_sixty_four_character_checksum_could_have_been_made() {
        let copied = copied_hash(SHA256_ABC).expect("a checksum");
        let sums = Sums {
            sha256: Some(SHA256_ABC.into()),
            blake3: Some(BLAKE3_ABC.into()),
            ..Sums::default()
        };
        assert_eq!(verdict(&sums, &copied), Some(Verdict::Match(Algo::Sha256)));
        let b3 = copied_hash(&BLAKE3_ABC.to_uppercase()).expect("a checksum");
        assert_eq!(verdict(&sums, &b3), Some(Verdict::Match(Algo::Blake3)));
        let other = copied_hash(&"0".repeat(64)).expect("a checksum");
        assert_eq!(verdict(&sums, &other), Some(Verdict::Mismatch));
    }

    #[test]
    fn nothing_is_said_until_every_candidate_is_made_or_when_the_checksum_cannot_be_compared() {
        let other = copied_hash(&"0".repeat(64)).expect("a checksum");
        let half = Sums {
            sha256: Some(SHA256_ABC.into()),
            ..Sums::default()
        };
        assert_eq!(verdict(&half, &other), None, "BLAKE3 is still to come");
        assert_eq!(verdict(&Sums::default(), &other), None);
        let md5 = copied_hash("d41d8cd98f00b204e9800998ecf8427e").expect("a checksum");
        assert_eq!(verdict(&half, &md5), None, "an MD5 can't be compared");
        let sha512 = copied_hash(&"ab".repeat(64)).expect("a checksum");
        assert_eq!(verdict(&half, &sha512), None);
        let both = Sums {
            sha512: Some("ab".repeat(64)),
            ..Sums::default()
        };
        assert_eq!(verdict(&both, &sha512), Some(Verdict::Match(Algo::Sha512)));
    }

    #[test]
    fn sums_add_up_and_take_away() {
        let mut have = Sums {
            sha256: Some("a".into()),
            ..Sums::default()
        };
        have.merge(Sums {
            blake3: Some("b".into()),
            ..Sums::default()
        });
        have.merge(Sums::default());
        assert_eq!(
            (have.sha256.as_deref(), have.blake3.as_deref(), have.sha512.as_deref()),
            (Some("a"), Some("b"), None)
        );
        let want = SumsWant {
            sha256: true,
            sha512: true,
            blake3: true,
        };
        assert_eq!(
            want.minus(Some(&have)),
            SumsWant {
                sha512: true,
                ..SumsWant::default()
            }
        );
        assert_eq!(want.minus(None), want);
        let asked = SumsWant {
            sha512: true,
            ..SumsWant::default()
        };
        assert_eq!(
            want.without(asked),
            SumsWant {
                sha256: true,
                blake3: true,
                ..SumsWant::default()
            }
        );
        assert_eq!(
            asked.and(SumsWant {
                sha256: true,
                ..SumsWant::default()
            }),
            SumsWant {
                sha256: true,
                sha512: true,
                ..SumsWant::default()
            }
        );
    }
}
