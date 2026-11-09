//! A remote in rclone's configuration (crate::rclone) as a connection: Google Drive, OneDrive, Dropbox
//! and the rest, listed and read through rclone's remote control.

use super::Item;
use serde_json::{json, Value};

pub struct Remote {
    /// Its name in rclone's configuration.
    pub name: String,
    /// The folder on it to start at ("" for its top), with no "/" at either end.
    pub root: String,
}

impl Remote {
    fn fs(&self) -> String {
        crate::rclone::fs(&self.name, &self.root)
    }

    /// What's in the folder `path` ("" for the folder it starts at).
    pub async fn list(&self, path: &str) -> Result<Vec<Item>, String> {
        let answer = crate::rclone::rc("operations/list", json!({ "fs": self.fs(), "remote": path, "opt": { "noMimeType": true } })).await?;
        Ok(answer.get("list").and_then(Value::as_array).map(|all| all.iter().filter_map(item).collect()).unwrap_or_default())
    }

    pub async fn stat(&self, path: &str) -> Result<Option<Item>, String> {
        let answer = crate::rclone::rc("operations/stat", json!({ "fs": self.fs(), "remote": path, "opt": { "noMimeType": true } })).await?;
        Ok(answer.get("item").and_then(item))
    }

    /// A file's bytes: all of them, or the `first` ones.
    pub async fn get(&self, path: &str, first: Option<u64>) -> Result<reqwest::Response, String> {
        crate::rclone::serve(&self.fs(), path, first).await
    }
}

fn item(value: &Value) -> Option<Item> {
    let (name, dir, size, modified) = crate::rclone::item_of(value)?;
    Some(Item { name, dir, size, modified })
}
