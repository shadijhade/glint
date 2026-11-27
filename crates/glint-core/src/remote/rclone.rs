//! A remote in rclone's configuration (crate::rclone) as a connection: Google Drive, OneDrive, Dropbox
//! and the rest, listed and read through rclone's remote control. Also a Windows share's connection opened
//! through rclone where Windows won't sign in to it (smb.rs): rclone's on-the-fly SMB remote, no configuration.

use super::Item;
use serde_json::{json, Value};
use std::sync::OnceLock;

pub struct Remote {
    /// Its name in rclone's configuration ("" for a share's).
    name: String,
    /// The folder on it to start at ("" for its top), with no "/" at either end.
    root: String,
    /// A share reached by rclone's own SMB client, and its password (obscured by rclone when first needed).
    share: Option<(super::smb::Rclone, String)>,
    /// What rclone calls it ("drive:Photos", ":smb,host=…:photos"), made on first use.
    fs: OnceLock<String>,
}

impl Remote {
    /// A remote in rclone's configuration.
    pub fn configured(name: String, root: String) -> Self {
        Remote { name, root, share: None, fs: OnceLock::new() }
    }

    /// A Windows share, through rclone's own SMB client.
    pub fn share(share: super::smb::Rclone, password: String) -> Self {
        Remote { name: String::new(), root: String::new(), share: Some((share, password)), fs: OnceLock::new() }
    }

    async fn fs(&self) -> Result<String, String> {
        if let Some(fs) = self.fs.get() {
            return Ok(fs.clone());
        }
        let fs = match &self.share {
            None => crate::rclone::fs(&self.name, &self.root),
            Some((share, password)) => {
                // rclone takes a password only obscured, as its configuration keeps them.
                let obscured = match password.is_empty() {
                    true => String::new(),
                    false => obscure(password).await?,
                };
                crate::rclone::fs(&share.remote(&obscured), &share.share_path)
            }
        };
        let _ = self.fs.set(fs.clone());
        Ok(fs)
    }

    /// What's in the folder `path` ("" for the folder it starts at).
    pub async fn list(&self, path: &str) -> Result<Vec<Item>, String> {
        let answer = crate::rclone::rc("operations/list", json!({ "fs": self.fs().await?, "remote": path, "opt": { "noMimeType": true } })).await?;
        Ok(answer.get("list").and_then(Value::as_array).map(|all| all.iter().filter_map(item).collect()).unwrap_or_default())
    }

    pub async fn stat(&self, path: &str) -> Result<Option<Item>, String> {
        let answer = crate::rclone::rc("operations/stat", json!({ "fs": self.fs().await?, "remote": path, "opt": { "noMimeType": true } })).await?;
        Ok(answer.get("item").and_then(item))
    }

    /// A file's bytes: all of them, or the `first` ones.
    pub async fn get(&self, path: &str, first: Option<u64>) -> Result<reqwest::Response, String> {
        crate::rclone::serve(&self.fs().await?, path, first).await
    }
}

/// A password as rclone keeps them (its own reversible scrambling, not encryption), by rclone itself.
async fn obscure(clear: &str) -> Result<String, String> {
    let answer = crate::rclone::rc("core/obscure", json!({ "clear": clear })).await?;
    answer.get("obscured").and_then(Value::as_str).map(str::to_string).ok_or_else(|| "rclone didn't take the password".into())
}

fn item(value: &Value) -> Option<Item> {
    let (name, dir, size, modified) = crate::rclone::item_of(value)?;
    Some(Item { name, dir, size, modified })
}
