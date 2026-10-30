//! What the app and the service say over the pipe. One request per connection: a u32 length, then a
//! JSON object. The answer is a series of frames, each a u32 length, a kind byte and a payload: JSON
//! messages, and search results and disk-usage maps in a compact binary form (thousands of paths a
//! frame).

pub use crate::history::Kind;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const FRAME_JSON: u8 = 1;
pub const FRAME_HITS: u8 = 2;
pub const FRAME_USAGE: u8 = 3;
/// After a map asked with `since`: each item's size at the mark, as u64s in the map's order.
pub const FRAME_GROWTH: u8 = 4;
/// An item that wasn't there at the mark.
pub const NEW: u64 = u64::MAX;
/// A file written since the mark (what it held then isn't kept).
pub const WRITTEN: u64 = u64::MAX - 1;
/// Requests are small; anything bigger is refused before it's read.
pub const MAX_REQUEST: usize = 1 << 20;
pub const MAX_FRAME: usize = 64 << 20;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    Status,
    Search(SearchRequest),
    /// Folder sizes: the folder's total and the total of each folder in it.
    Sizes { path: String },
    /// A disk-usage map: the biggest files and folders below a folder, a few levels deep.
    Usage(UsageRequest),
    /// Everything right inside a folder, for one Windows keeps the user out of: hit frames, as a
    /// search's results come, then Done.
    List { path: String },
    /// What was renamed, moved and deleted on the indexed drives, newest first.
    History(HistoryRequest),
    /// A copy of Glint that isn't the Microsoft Store's says it uses the service, so it stays when the
    /// Store's copy that installed it goes (store.rs). Answered with the status. A service from before
    /// closes the pipe on it.
    Claim,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryRequest {
    /// Only what has this in a path (any case); empty for everything.
    pub query: String,
    /// Only what happened since then (Unix ms).
    pub since: i64,
    /// At most this many (10,000 at most).
    pub max: u32,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryAnswer {
    pub events: Vec<HistoryEvent>,
    /// More matched than were sent.
    pub limited: bool,
    /// When the oldest event kept happened (Unix ms, 0 when there's none): how far back it goes.
    pub oldest: i64,
    /// The service's time, in ms.
    pub elapsed: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEvent {
    /// Unix ms.
    pub time: i64,
    pub kind: Kind,
    pub dir: bool,
    /// Where it was (for something in the Recycle Bin, its place there).
    pub path: String,
    /// Where it went (renamed, moved, recycled, restored).
    pub to: Option<String>,
    /// For something in the Recycle Bin: where it was before.
    pub was: Option<String>,
    /// Where it is now, if it's still there (it may have moved on since).
    pub now: Option<String>,
    /// A folder deleted with what was in it: how many items went with it. A burst: how many more
    /// were deleted in the folder `path`.
    pub items: u32,
    pub burst: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    /// Folders to search below (each with its subfolders), e.g. `C:\Users\me` or `D:\`.
    pub roots: Vec<String>,
    /// Include hidden files and folders.
    pub hidden: bool,
    /// Names or globs to skip, with everything inside them (fd's --exclude).
    pub exclude: Vec<String>,
    pub max_results: Option<usize>,
    /// When nothing matches as typed, look again with typos allowed (`NameMatcher::near`); Done then
    /// says `fuzzy`. A service from before this ignores it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub fuzzy: bool,
    /// The best `max_results` of every drive's matches, best first (`Tree::rank`), instead of the first
    /// ones in index order, drive by drive. A service from before this ignores it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rank: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageRequest {
    /// The folder to map.
    pub path: String,
    /// How many levels down to go: 1 is only what's right inside the folder.
    pub depth: u32,
    /// The most items to send. The biggest are sent, with the folders they're in; the rest only
    /// count in their folders' sizes.
    pub max_items: u32,
    /// What grew since then (Unix ms): each item's size at the daily mark closest to it comes too
    /// (FRAME_GROWTH). A service from before marks leaves it out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Message {
    Status(Status),
    /// Sent first for a search: the roots the index answers for, and those it can't (not NTFS, not
    /// indexed yet, behind a junction…), which the app searches some other way.
    Plan { indexed: Vec<String>, skipped: Vec<String> },
    /// The search is complete. `elapsed` is the service's time in milliseconds; `fuzzy` when nothing
    /// matched as typed and these are close matches (a search that asked for them).
    Done {
        count: u64,
        limited: bool,
        elapsed: f64,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        fuzzy: bool,
    },
    Sizes(Sizes),
    /// Follows a disk-usage map's frame.
    Usage(UsageSummary),
    History(HistoryAnswer),
    Error { message: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub version: String,
    pub volumes: Vec<VolumeStatus>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct VolumeStatus {
    /// Drive, e.g. "C:".
    pub volume: String,
    /// "scanning", "ready" or "error".
    pub state: String,
    pub files: u64,
    pub folders: u64,
    /// Bytes the index of this volume takes in memory.
    pub memory: u64,
    /// How the index was built: "scan" (the MFT) or "snapshot"; and how long that took, in ms.
    pub source: String,
    pub build_ms: f64,
    /// Records read so far while scanning, of `progress_total`.
    pub progress: u64,
    pub progress_total: u64,
    /// When the last change from the journal was applied (Unix ms), 0 if none yet.
    pub updated: i64,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Sizes {
    pub path: String,
    /// Everything below the folder, in bytes.
    pub total: u64,
    /// Folder name → bytes, for each folder right inside it.
    pub sizes: Vec<(String, u64)>,
    pub elapsed: f64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub path: String,
    /// Everything below the folder, in bytes.
    pub total: u64,
    /// Items in the map, the folder itself included.
    pub items: u64,
    /// Whether smaller items were left out (they still count in their folders' sizes).
    pub limited: bool,
    /// The service's time, in ms.
    pub elapsed: f64,
    /// For a map asked with `since`: the mark its items' earlier sizes are from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub growth: Option<GrowthSummary>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GrowthSummary {
    /// When the mark was taken (Unix ms).
    pub since: i64,
    /// The oldest mark folders can be compared with: how far back growth goes.
    pub oldest: i64,
    /// The whole drive's total at the mark, and now.
    pub drive_then: u64,
    pub drive_now: u64,
}

/// A disk-usage map as the service sends it.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageMap {
    pub nodes: Vec<UsageNode>,
    /// Each node's size at the mark (NEW, WRITTEN), when asked for and known.
    pub previous: Option<Vec<u64>>,
    pub summary: UsageSummary,
}

/// One item of a disk-usage map. The folder mapped comes first; each item's folder comes before it.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageNode {
    /// Where the folder it's in is in the map; NO_PARENT for the folder mapped.
    pub parent: u32,
    pub name: String,
    pub dir: bool,
    /// Files: their size. Folders: everything below them.
    pub size: u64,
    /// Last write time, Unix milliseconds (0 when unknown).
    pub modified: i64,
    pub attrs: u32,
}

pub const NO_PARENT: u32 = u32::MAX;

/// A search result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub dir: bool,
    /// Files: their size. Folders: everything below them.
    pub size: u64,
    /// Last write time, Unix milliseconds (0 when unknown).
    pub modified: i64,
    pub attrs: u32,
}

/// FILETIME (100 ns since 1601) to Unix milliseconds.
pub fn unix_ms(filetime: i64) -> i64 {
    if filetime <= 0 {
        0
    } else {
        (filetime - 116_444_736_000_000_000) / 10_000
    }
}

/// Appends one hit: flags, attributes, size, modified, then the path as length-prefixed UTF-8.
pub fn encode_hit(out: &mut Vec<u8>, hit: &Hit) {
    // A path too long for its 16-bit length (deep \\?\ folders) is left out: cut mid-character, it
    // would make the app drop the whole batch.
    let Ok(len) = u16::try_from(hit.path.len()) else { return };
    out.push(hit.dir as u8);
    out.extend_from_slice(&hit.attrs.to_le_bytes());
    out.extend_from_slice(&hit.size.to_le_bytes());
    out.extend_from_slice(&hit.modified.to_le_bytes());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(hit.path.as_bytes());
}

pub fn decode_hits(mut bytes: &[u8]) -> Result<Vec<Hit>, String> {
    let mut hits = Vec::new();
    let bad = || "truncated search results".to_string();
    while !bytes.is_empty() {
        if bytes.len() < 23 {
            return Err(bad());
        }
        let len = u16::from_le_bytes([bytes[21], bytes[22]]) as usize;
        let path = bytes.get(23..23 + len).ok_or_else(bad)?;
        hits.push(Hit {
            dir: bytes[0] & 1 != 0,
            attrs: u32::from_le_bytes(bytes[1..5].try_into().unwrap()),
            size: u64::from_le_bytes(bytes[5..13].try_into().unwrap()),
            modified: i64::from_le_bytes(bytes[13..21].try_into().unwrap()),
            path: String::from_utf8(path.to_vec()).map_err(|_| "search result path isn't UTF-8".to_string())?,
        });
        bytes = &bytes[23 + len..];
    }
    Ok(hits)
}

/// Appends one map item: its folder's place, flags, attributes, size, modified, then the name as
/// length-prefixed UTF-8.
pub fn encode_usage_node(out: &mut Vec<u8>, node: &UsageNode) {
    out.extend_from_slice(&node.parent.to_le_bytes());
    out.push(node.dir as u8);
    out.extend_from_slice(&node.attrs.to_le_bytes());
    out.extend_from_slice(&node.size.to_le_bytes());
    out.extend_from_slice(&node.modified.to_le_bytes());
    let name = &node.name.as_bytes()[..node.name.len().min(u16::MAX as usize)];
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(name);
}

/// Reads a map back, checking it's a tree: the folder mapped first, every other item after the
/// folder it's in.
pub fn decode_usage(mut bytes: &[u8]) -> Result<Vec<UsageNode>, String> {
    let mut nodes: Vec<UsageNode> = Vec::new();
    let bad = |what: &str| format!("disk-usage map {what}");
    while !bytes.is_empty() {
        if bytes.len() < 27 {
            return Err(bad("truncated"));
        }
        let len = u16::from_le_bytes([bytes[25], bytes[26]]) as usize;
        let name = bytes.get(27..27 + len).ok_or_else(|| bad("truncated"))?;
        let node = UsageNode {
            parent: u32::from_le_bytes(bytes[0..4].try_into().unwrap()),
            dir: bytes[4] & 1 != 0,
            attrs: u32::from_le_bytes(bytes[5..9].try_into().unwrap()),
            size: u64::from_le_bytes(bytes[9..17].try_into().unwrap()),
            modified: i64::from_le_bytes(bytes[17..25].try_into().unwrap()),
            name: String::from_utf8(name.to_vec()).map_err(|_| bad("name isn't UTF-8"))?,
        };
        let placed = match nodes.get(node.parent as usize) {
            Some(folder) => folder.dir,
            None => nodes.is_empty() && node.parent == NO_PARENT && node.dir,
        };
        if !placed {
            return Err(bad("out of order"));
        }
        nodes.push(node);
        bytes = &bytes[27 + len..];
    }
    Ok(nodes)
}

pub fn encode_growth(previous: &[u64]) -> Vec<u8> {
    previous.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn decode_growth(bytes: &[u8]) -> Result<Vec<u64>, String> {
    let (words, rest) = bytes.as_chunks::<8>();
    if !rest.is_empty() {
        return Err("growth frame truncated".into());
    }
    Ok(words.iter().map(|w| u64::from_le_bytes(*w)).collect())
}

pub fn write_frame<W: Write + ?Sized>(w: &mut W, kind: u8, payload: &[u8]) -> io::Result<()> {
    let mut frame = Vec::with_capacity(5 + payload.len());
    frame.extend_from_slice(&(payload.len() as u32 + 1).to_le_bytes());
    frame.push(kind);
    frame.extend_from_slice(payload);
    w.write_all(&frame)
}

pub fn write_message<W: Write + ?Sized>(w: &mut W, message: &Message) -> io::Result<()> {
    write_frame(w, FRAME_JSON, &serde_json::to_vec(message).map_err(io::Error::other)?)
}

/// The next frame, or None once the other side has finished.
pub fn read_frame<R: Read + ?Sized>(r: &mut R, max: usize) -> io::Result<Option<(u8, Vec<u8>)>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > max {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("frame of {len} bytes")));
    }
    let mut frame = vec![0u8; len];
    r.read_exact(&mut frame)?;
    let kind = frame.remove(0);
    Ok(Some((kind, frame)))
}

pub fn write_request<W: Write + ?Sized>(w: &mut W, request: &Request) -> io::Result<()> {
    let json = serde_json::to_vec(request).map_err(io::Error::other)?;
    let mut frame = (json.len() as u32).to_le_bytes().to_vec();
    frame.extend_from_slice(&json);
    w.write_all(&frame)
}

pub fn read_request<R: Read + ?Sized>(r: &mut R) -> io::Result<Request> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_REQUEST {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "request too large"));
    }
    let mut json = vec![0u8; len];
    r.read_exact(&mut json)?;
    serde_json::from_slice(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_frames_round_trip() {
        let request = Request::Search(SearchRequest {
            query: "*.pdf report".into(),
            roots: vec![r"C:\Users\me".into(), r"D:\".into()],
            hidden: false,
            exclude: vec!["node_modules".into()],
            max_results: Some(50_000),
            fuzzy: false,
            rank: false,
        });
        let mut wire = Vec::new();
        write_request(&mut wire, &request).unwrap();
        assert_eq!(read_request(&mut wire.as_slice()).unwrap(), request);
        assert!(serde_json::to_string(&Request::Status).unwrap().contains(r#""op":"status""#));
        assert_eq!(serde_json::to_string(&Request::Claim).unwrap(), r#"{"op":"claim"}"#);
        // Without close matches asked for, a request is as a service from before them reads it; with
        // them, an older service reads it too (it ignores what it doesn't know).
        assert!(serde_json::to_string(&request).unwrap().ends_with(r#""maxResults":50000}"#));
        let Request::Search(search) = &request else { unreachable!() };
        let fuzzy = Request::Search(SearchRequest { fuzzy: true, ..search.clone() });
        let ranked = Request::Search(SearchRequest { rank: true, ..search.clone() });
        assert!(serde_json::to_string(&ranked).unwrap().contains(r#""rank":true"#));
        assert!(serde_json::to_string(&fuzzy).unwrap().ends_with(r#""maxResults":50000,"fuzzy":true}"#));
        let newer = r#"{"op":"search","query":"a","roots":[],"hidden":false,"exclude":[],"maxResults":null,"fuzzy":true,"fromLater":1}"#;
        assert!(matches!(serde_json::from_str::<Request>(newer), Ok(Request::Search(SearchRequest { fuzzy: true, .. }))), "what a service doesn't know is left");
        assert_eq!(serde_json::from_str::<Message>(r#"{"type":"done","count":1,"limited":false,"elapsed":0.5}"#).unwrap(), Message::Done { count: 1, limited: false, elapsed: 0.5, fuzzy: false });

        let hits = vec![
            Hit { path: r"C:\Users\me\été.pdf".into(), dir: false, size: 1 << 40, modified: 1_700_000_000_000, attrs: 0x20 },
            Hit { path: r"D:\Reports".into(), dir: true, size: 0, modified: 0, attrs: 0x10 },
        ];
        let mut payload = Vec::new();
        hits.iter().for_each(|h| encode_hit(&mut payload, h));
        let mut wire = Vec::new();
        write_frame(&mut wire, FRAME_HITS, &payload).unwrap();
        write_message(&mut wire, &Message::Done { count: 2, limited: false, elapsed: 1.5, fuzzy: true }).unwrap();
        let mut reader = wire.as_slice();
        let (kind, payload) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!((kind, decode_hits(&payload).unwrap()), (FRAME_HITS, hits));
        let (kind, json) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!(kind, FRAME_JSON);
        assert_eq!(serde_json::from_slice::<Message>(&json).unwrap(), Message::Done { count: 2, limited: false, elapsed: 1.5, fuzzy: true });
        assert!(read_frame(&mut reader, MAX_FRAME).unwrap().is_none());
        assert!(decode_hits(&payload[..10]).is_err());
    }

    #[test]
    fn usage_maps_round_trip() {
        let request = Request::Usage(UsageRequest { path: r"C:\".into(), depth: 4, max_items: 20_000, since: None });
        let mut wire = Vec::new();
        write_request(&mut wire, &request).unwrap();
        assert_eq!(read_request(&mut wire.as_slice()).unwrap(), request);
        // As a service from before growth reads it, with `since` or without.
        assert!(serde_json::to_string(&request).unwrap().ends_with(r#""op":"usage","path":"C:\\","depth":4,"maxItems":20000}"#));
        let asked = Request::Usage(UsageRequest { path: r"C:\".into(), depth: 4, max_items: 9, since: Some(1_700_000_000_000) });
        let json = serde_json::to_string(&asked).unwrap();
        assert!(json.contains(r#""since":1700000000000"#), "{json}");
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), asked);

        let nodes = vec![
            UsageNode { parent: NO_PARENT, name: String::new(), dir: true, size: 3 << 40, modified: 0, attrs: 0x16 },
            UsageNode { parent: 0, name: "Windows".into(), dir: true, size: 2 << 40, modified: 1_700_000_000_000, attrs: 0x10 },
            UsageNode { parent: 0, name: "pagefile.sys".into(), dir: false, size: 1 << 34, modified: -1, attrs: 0x26 },
            UsageNode { parent: 1, name: "Été😀.log".into(), dir: false, size: 7, modified: 5, attrs: 0x20 },
        ];
        let mut payload = Vec::new();
        nodes.iter().for_each(|n| encode_usage_node(&mut payload, n));
        let growth = GrowthSummary { since: 1_700_000_000_000, oldest: 1_690_000_000_000, drive_then: 2 << 40, drive_now: 3 << 40 };
        let summary = UsageSummary { path: r"C:\".into(), total: 3 << 40, items: 4, limited: true, elapsed: 2.5, growth: Some(growth) };
        let previous = [2 << 40, NEW, 1 << 34, WRITTEN];
        let mut wire = Vec::new();
        write_frame(&mut wire, FRAME_USAGE, &payload).unwrap();
        write_frame(&mut wire, FRAME_GROWTH, &encode_growth(&previous)).unwrap();
        write_message(&mut wire, &Message::Usage(summary.clone())).unwrap();
        let mut reader = wire.as_slice();
        let (kind, payload) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!((kind, decode_usage(&payload).unwrap()), (FRAME_USAGE, nodes.clone()));
        let (kind, earlier) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!((kind, decode_growth(&earlier).unwrap()), (FRAME_GROWTH, previous.to_vec()));
        assert!(decode_growth(&earlier[..9]).is_err());
        let (kind, json) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!((kind, serde_json::from_slice::<Message>(&json).unwrap()), (FRAME_JSON, Message::Usage(summary.clone())));
        assert!(read_frame(&mut reader, MAX_FRAME).unwrap().is_none());
        // A summary without growth reads as one from before it, and the other way round.
        let plain = UsageSummary { growth: None, ..summary };
        let json = serde_json::to_string(&Message::Usage(plain.clone())).unwrap();
        assert!(!json.contains("growth"));
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), Message::Usage(plain));

        // Damage is refused rather than handed to the UI.
        assert!(decode_usage(&payload[..payload.len() - 1]).is_err());
        assert!(decode_usage(&payload[..20]).is_err());
        let mut orphan = Vec::new();
        encode_usage_node(&mut orphan, &nodes[0]);
        encode_usage_node(&mut orphan, &UsageNode { parent: 1, ..nodes[1].clone() });
        assert!(decode_usage(&orphan).is_err());
        let mut inside_a_file = Vec::new();
        nodes[..3].iter().for_each(|n| encode_usage_node(&mut inside_a_file, n));
        encode_usage_node(&mut inside_a_file, &UsageNode { parent: 2, ..nodes[3].clone() });
        assert!(decode_usage(&inside_a_file).is_err());
        let mut rootless = Vec::new();
        encode_usage_node(&mut rootless, &nodes[1]);
        assert!(decode_usage(&rootless).is_err());
        assert_eq!(decode_usage(&[]).unwrap(), []);
    }

    #[test]
    fn history_round_trips() {
        let request = Request::History(HistoryRequest { query: "report".into(), since: 1, max: 100 });
        let mut wire = Vec::new();
        write_request(&mut wire, &request).unwrap();
        assert_eq!(read_request(&mut wire.as_slice()).unwrap(), request);
        assert!(serde_json::to_string(&request).unwrap().contains(r#""op":"history""#));
        let moved = HistoryEvent { time: 5, kind: Kind::Moved, dir: false, path: r"C:\a.txt".into(), to: Some(r"D:\a.txt".into()), was: None, now: Some(r"D:\a.txt".into()), items: 0, burst: false };
        let answer = HistoryAnswer { events: vec![moved], limited: true, oldest: 1, elapsed: 0.5 };
        let json = serde_json::to_string(&Message::History(answer.clone())).unwrap();
        assert!(json.contains(r#""type":"history""#) && json.contains(r#""kind":"moved""#), "{json}");
        assert_eq!(serde_json::from_str::<Message>(&json).unwrap(), Message::History(answer));
    }

    #[test]
    fn converts_filetimes() {
        assert_eq!(unix_ms(116_444_736_000_000_000), 0);
        assert_eq!(unix_ms(133_000_000_000_000_000), 1_655_526_400_000);
        assert_eq!(unix_ms(0), 0);
    }
}
