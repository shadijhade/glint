//! Natural order ("file2" before "file10") as bytes: a key whose plain byte order is natural order, so a sort compares
//! keys with memcmp instead of reading digit runs again on every comparison. The engine's listings
//! (`glint-core`'s `fsx`) and the window's rows (`explorer/rows.rs`) sort with these, so both order names the
//! same way, and exactly as [`crate::entries::natural_compare`] does.

use std::cmp::Ordering;

/// A name's key (`lower` is lowercase; ASCII letters are lowercased here too, so an ASCII name needn't be copied
/// first): each digit run becomes '0', its length, then its digits without leading zeros. Since digits are the
/// only bytes from '0' to '9', a run still sorts against other characters exactly where a digit would.
pub fn natural_key(lower: &str) -> Vec<u8> {
    let mut key = Vec::with_capacity(lower.len() + 4);
    natural_key_into(lower, &mut key);
    key
}

/// [`natural_key`], added to the end of `key`: what's between digit runs lowercased in one go (a loop the compiler
/// vectorizes), not a byte at a time.
///
/// A run's length is one byte up to 254 digits; a longer run is 255 and then how much longer it is as four bytes,
/// so a run of any length is ordered as a number (the length used to stop at 255).
pub fn natural_key_into(lower: &str, key: &mut Vec<u8>) {
    let bytes = lower.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let run = bytes[i..]
            .iter()
            .position(u8::is_ascii_digit)
            .map_or(bytes.len(), |p| i + p);
        key.extend(bytes[i..run].iter().map(u8::to_ascii_lowercase));
        i = run;
        if i < bytes.len() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let digits = trim_zeros(&bytes[start..i]);
            key.push(b'0');
            match u8::try_from(digits.len()) {
                Ok(len) if len < u8::MAX => key.push(len),
                _ => {
                    key.push(u8::MAX);
                    let more = u32::try_from(digits.len() - usize::from(u8::MAX)).unwrap_or(u32::MAX);
                    key.extend_from_slice(&more.to_be_bytes());
                }
            }
            key.extend_from_slice(digits);
        }
    }
}

/// A key's first 32 bytes (zero-padded) as four numbers: comparing them settles nearly every comparison, even among
/// names that share a long start, such as WinSxS's, without touching the key.
pub fn head(key: &[u8]) -> [u64; 4] {
    let mut bytes = [0u8; 32];
    let n = key.len().min(32);
    bytes[..n].copy_from_slice(&key[..n]);
    let word = |n: usize| u64::from_be_bytes(bytes[n * 8..n * 8 + 8].try_into().unwrap_or_default());
    [word(0), word(1), word(2), word(3)]
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let start = digits.iter().position(|&d| d != b'0').unwrap_or(digits.len());
    &digits[start..]
}

/// The keys of a list of names, kept together (one buffer for the keys, one small record per name), made once so
/// that sorting the list compares keys, not names.
pub struct NameKeys {
    keys: Vec<u8>,
    items: Vec<Item>,
}

struct Item {
    head: [u64; 4],
    start: u32,
    end: u32,
}

impl NameKeys {
    /// The keys of `lowers` (lowercase names), in order; `None` for a list whose keys don't fit 4 GB, which is
    /// sorted by comparing names instead.
    pub fn new<'a>(lowers: impl ExactSizeIterator<Item = &'a str>) -> Option<NameKeys> {
        let mut keys: Vec<u8> = Vec::new();
        let mut items = Vec::with_capacity(lowers.len());
        for lower in lowers {
            let start = keys.len();
            natural_key_into(lower, &mut keys);
            let key = &keys[start..];
            items.push(Item {
                head: head(key),
                start: u32::try_from(start).ok()?,
                end: u32::try_from(keys.len()).ok()?,
            });
        }
        Some(NameKeys { keys, items })
    }

    /// How the `a`th and the `b`th name compare in natural order.
    pub fn compare(&self, a: usize, b: usize) -> Ordering {
        let (Some(a), Some(b)) = (self.items.get(a), self.items.get(b)) else {
            return Ordering::Equal;
        };
        a.head.cmp(&b.head).then_with(|| self.key(a).cmp(self.key(b)))
    }

    fn key(&self, item: &Item) -> &[u8] {
        self.keys.get(item.start as usize..item.end as usize).unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entries::natural_compare;

    /// A small deterministic generator.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// Names built from pieces that are hard for a key: digits and runs of them, leading zeros, case, accents,
    /// scripts, a shared long start, and digit runs past what a length byte holds.
    fn names(count: usize, seed: u64) -> Vec<String> {
        let parts = [
            "amd64_microsoft-windows-",
            "file",
            "File",
            "IMG_",
            "é",
            "ß",
            "z",
            "0",
            "00",
            "007",
            "10",
            "9",
            "1",
            ".",
            " ",
            "-",
            "a",
            "A",
            "日本",
            "😀",
            "İ",
            "Σ",
            "\u{ffff}",
            "_copy",
            "(2)",
            "x99",
            "x100",
            "~",
        ];
        let mut random = Random(seed);
        let mut out: Vec<String> = (0..count)
            .map(|_| {
                (0..1 + random.below(7))
                    .map(|_| parts[random.below(parts.len())])
                    .collect::<String>()
                    .to_lowercase()
            })
            .collect();
        // Names that differ only after 32 bytes of key, and in long digit runs.
        let long = "windows-component-store-assembly-".repeat(2);
        for i in 0..40 {
            out.push(format!("{long}{i}"));
            out.push(format!("{long}{i:03}x"));
            out.push(format!("{long}{}", i % 7));
        }
        for digits in [253usize, 254, 255, 256, 257, 300, 600] {
            for (lead, last) in [("", '1'), ("", '2'), ("00", '1'), ("", '0')] {
                out.push(format!("n{lead}{}{last}x", "9".repeat(digits - 1)));
                out.push(format!("n{lead}{}", "1".repeat(digits)));
            }
        }
        out.push(String::new());
        out
    }

    /// The order of the keys is natural_compare's, for every pair out of many random and tricky names.
    #[test]
    fn keys_order_names_as_natural_compare_does() {
        let names = names(260, 0x2545_f491_4f6c_dd1d);
        let keys = NameKeys::new(names.iter().map(String::as_str)).expect("keys");
        for (i, a) in names.iter().enumerate() {
            for (j, b) in names.iter().enumerate() {
                assert_eq!(keys.compare(i, j), natural_compare(a, b), "{a:?} against {b:?}");
                assert_eq!(
                    natural_key(a).cmp(&natural_key(b)),
                    natural_compare(a, b),
                    "{a:?} against {b:?}"
                );
            }
        }
    }

    /// A sort by the keys leaves the same list as a sort by `natural_compare`, ties by the name as written.
    #[test]
    fn sorting_by_keys_gives_the_order_of_sorting_by_names() {
        for seed in [1u64, 99, 0xdead_beef] {
            let names = names(3_000, seed);
            let keys = NameKeys::new(names.iter().map(String::as_str)).expect("keys");
            let mut by_keys: Vec<usize> = (0..names.len()).collect();
            by_keys.sort_by(|&a, &b| keys.compare(a, b).then_with(|| names[a].cmp(&names[b])));
            let mut by_names: Vec<usize> = (0..names.len()).collect();
            by_names.sort_by(|&a, &b| natural_compare(&names[a], &names[b]).then_with(|| names[a].cmp(&names[b])));
            let list = |order: &[usize]| order.iter().map(|&i| names[i].clone()).collect::<Vec<_>>();
            assert_eq!(list(&by_keys), list(&by_names), "seed {seed}");
        }
    }

    #[test]
    fn names_that_differ_only_after_the_head_are_told_apart() {
        let a = format!("{}1", "q".repeat(40));
        let b = format!("{}2", "q".repeat(40));
        assert_eq!(head(&natural_key(&a)), head(&natural_key(&b)));
        let keys = NameKeys::new([a.as_str(), b.as_str()].into_iter()).expect("keys");
        assert_eq!(keys.compare(0, 1), Ordering::Less);
        assert_eq!(keys.compare(1, 0), Ordering::Greater);
        assert_eq!(keys.compare(0, 0), Ordering::Equal);
    }

    #[test]
    fn a_name_that_is_not_there_compares_equal() {
        let keys = NameKeys::new(["a"].into_iter()).expect("keys");
        assert_eq!(keys.compare(0, 5), Ordering::Equal);
    }
}
