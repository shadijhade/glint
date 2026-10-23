//! What the app and the service say over the pipe. One request per connection: a u32 length, then a
//! JSON object. The answer is a series of frames, each a u32 length, a kind byte and a payload: JSON
//! messages, and search results and disk-usage maps in a compact binary form (thousands of paths a
//! frame).

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const FRAME_JSON: u8 = 1;
pub const FRAME_HITS: u8 = 2;
pub const FRAME_USAGE: u8 = 3;
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
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Message {
    Status(Status),
    /// Sent first for a search: the roots the index answers for, and those it can't (not NTFS, not
    /// indexed yet, behind a junction…), which the app searches some other way.
    Plan { indexed: Vec<String>, skipped: Vec<String> },
    /// The search is complete. `elapsed` is the service's time in milliseconds.
    Done { count: u64, limited: bool, elapsed: f64 },
    Sizes(Sizes),
    /// Follows a disk-usage map's frame.
    Usage(UsageSummary),
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
        });
        let mut wire = Vec::new();
        write_request(&mut wire, &request).unwrap();
        assert_eq!(read_request(&mut wire.as_slice()).unwrap(), request);
        assert!(serde_json::to_string(&Request::Status).unwrap().contains(r#""op":"status""#));

        let hits = vec![
            Hit { path: r"C:\Users\me\été.pdf".into(), dir: false, size: 1 << 40, modified: 1_700_000_000_000, attrs: 0x20 },
            Hit { path: r"D:\Reports".into(), dir: true, size: 0, modified: 0, attrs: 0x10 },
        ];
        let mut payload = Vec::new();
        hits.iter().for_each(|h| encode_hit(&mut payload, h));
        let mut wire = Vec::new();
        write_frame(&mut wire, FRAME_HITS, &payload).unwrap();
        write_message(&mut wire, &Message::Done { count: 2, limited: false, elapsed: 1.5 }).unwrap();
        let mut reader = wire.as_slice();
        let (kind, payload) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!((kind, decode_hits(&payload).unwrap()), (FRAME_HITS, hits));
        let (kind, json) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!(kind, FRAME_JSON);
        assert_eq!(serde_json::from_slice::<Message>(&json).unwrap(), Message::Done { count: 2, limited: false, elapsed: 1.5 });
        assert!(read_frame(&mut reader, MAX_FRAME).unwrap().is_none());
        assert!(decode_hits(&payload[..10]).is_err());
    }

    #[test]
    fn usage_maps_round_trip() {
        let request = Request::Usage(UsageRequest { path: r"C:\".into(), depth: 4, max_items: 20_000 });
        let mut wire = Vec::new();
        write_request(&mut wire, &request).unwrap();
        assert_eq!(read_request(&mut wire.as_slice()).unwrap(), request);
        assert!(serde_json::to_string(&request).unwrap().contains(r#""op":"usage","path":"C:\\","depth":4,"maxItems":20000"#));

        let nodes = vec![
            UsageNode { parent: NO_PARENT, name: String::new(), dir: true, size: 3 << 40, modified: 0, attrs: 0x16 },
            UsageNode { parent: 0, name: "Windows".into(), dir: true, size: 2 << 40, modified: 1_700_000_000_000, attrs: 0x10 },
            UsageNode { parent: 0, name: "pagefile.sys".into(), dir: false, size: 1 << 34, modified: -1, attrs: 0x26 },
            UsageNode { parent: 1, name: "Été😀.log".into(), dir: false, size: 7, modified: 5, attrs: 0x20 },
        ];
        let mut payload = Vec::new();
        nodes.iter().for_each(|n| encode_usage_node(&mut payload, n));
        let summary = UsageSummary { path: r"C:\".into(), total: 3 << 40, items: 4, limited: true, elapsed: 2.5 };
        let mut wire = Vec::new();
        write_frame(&mut wire, FRAME_USAGE, &payload).unwrap();
        write_message(&mut wire, &Message::Usage(summary.clone())).unwrap();
        let mut reader = wire.as_slice();
        let (kind, payload) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!((kind, decode_usage(&payload).unwrap()), (FRAME_USAGE, nodes.clone()));
        let (kind, json) = read_frame(&mut reader, MAX_FRAME).unwrap().unwrap();
        assert_eq!((kind, serde_json::from_slice::<Message>(&json).unwrap()), (FRAME_JSON, Message::Usage(summary)));
        assert!(read_frame(&mut reader, MAX_FRAME).unwrap().is_none());

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
    fn converts_filetimes() {
        assert_eq!(unix_ms(116_444_736_000_000_000), 0);
        assert_eq!(unix_ms(133_000_000_000_000_000), 1_655_526_400_000);
        assert_eq!(unix_ms(0), 0);
    }
}
