//! A paired PC's shared folders (devices.rs) as a remote: listed, read and downloaded over iroh, one
//! QUIC stream a request on a connection kept open while Glint runs.

use super::Item;
use crate::devices::{self, Answer, Request};
use iroh::endpoint::{RecvStream, SendStream};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};

static OPEN: LazyLock<Mutex<HashMap<iroh::EndpointId, iroh::endpoint::Connection>>> = LazyLock::new(Default::default);

/// Keeps a connection made (pairing's) for what's asked next.
pub fn connected(id: iroh::EndpointId, connection: iroh::endpoint::Connection) {
    OPEN.lock().unwrap_or_else(|e| e.into_inner()).insert(id, connection);
}

/// What it is when the other PC says an item isn't there.
const NOT_THERE: &str = "That isn't there";

pub struct Peer {
    pub id: iroh::EndpointId,
}

fn item(w: devices::Wire) -> Item {
    Item { name: w.name, dir: w.dir, size: w.size, modified: w.modified }
}

impl Peer {
    async fn streams(&self) -> Result<(SendStream, RecvStream), String> {
        let open = OPEN.lock().unwrap_or_else(|e| e.into_inner()).get(&self.id).cloned();
        if let Some(connection) = open {
            if let Ok(pair) = connection.open_bi().await {
                return Ok(pair);
            }
        }
        let connection = devices::connect(self.id).await?;
        let pair = connection.open_bi().await.map_err(|e| e.to_string())?;
        connected(self.id, connection);
        Ok(pair)
    }

    async fn ask(&self, request: &Request) -> Result<(Answer, RecvStream), String> {
        let (mut send, mut recv) = self.streams().await?;
        devices::write_frame(&mut send, request).await?;
        let _ = send.finish();
        let answer: Answer = devices::read_frame(&mut recv, devices::ANSWER_MOST).await?;
        if !answer.error.is_empty() {
            return Err(answer.error);
        }
        Ok((answer, recv))
    }

    pub async fn list(&self, path: &str) -> Result<Vec<Item>, String> {
        let (answer, _) = self.ask(&Request::List { path: path.into() }).await?;
        Ok(answer.items.into_iter().map(item).collect())
    }

    pub async fn stat(&self, path: &str) -> Result<Option<Item>, String> {
        match self.ask(&Request::Stat { path: path.into() }).await {
            Ok((answer, _)) => Ok(answer.item.map(item)),
            Err(e) if e == NOT_THERE => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// A file downloaded to `dest`, as `tick` is told how far it is (false stops it: "cancelled").
    pub async fn fetch(&self, path: &str, dest: &Path, expected: Option<u64>, tick: &mut (dyn FnMut(u64) -> bool + Send)) -> Result<u64, String> {
        use std::io::Write;
        let (answer, mut recv) = self.ask(&Request::Read { path: path.into(), offset: 0 }).await?;
        let size = answer.size;
        if expected.is_some_and(|n| n != size) {
            return Err("It changed on the other PC: open its folder again".into());
        }
        let mut file = std::fs::File::create(dest).map_err(crate::fsx::file_error)?;
        let mut buffer = vec![0u8; 256 << 10];
        let mut written = 0u64;
        while let Some(n) = recv.read(&mut buffer).await.map_err(|e| e.to_string())? {
            written += n as u64;
            if written > size {
                return Err("It sent more than its size".into());
            }
            file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
            if !tick(n as u64) {
                let _ = recv.stop(0u32.into());
                return Err("cancelled".into());
            }
        }
        if written != size {
            return Err(format!("It ended after {written} of its {size} bytes"));
        }
        Ok(written)
    }

    /// The first `limit` bytes of a file, and whether there's more (the rest isn't sent: the stream stops).
    pub async fn prefix(&self, path: &str, limit: usize) -> Result<(Vec<u8>, bool), String> {
        let (answer, mut recv) = self.ask(&Request::Read { path: path.into(), offset: 0 }).await?;
        let mut bytes = Vec::new();
        let mut buffer = vec![0u8; 64 << 10];
        while bytes.len() <= limit {
            match recv.read(&mut buffer).await.map_err(|e| e.to_string())? {
                Some(n) => bytes.extend_from_slice(&buffer[..n]),
                None => break,
            }
        }
        let _ = recv.stop(0u32.into());
        let more = bytes.len() > limit || answer.size > limit as u64;
        bytes.truncate(limit);
        Ok((bytes, more))
    }
}
