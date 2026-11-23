//! What a volume can do, asked of the volume rather than guessed from an error's text (G7): whether it keeps
//! named streams (`GetVolumeInformationW`'s `FILE_NAMED_STREAMS`), asked once per volume.

/// Whether the volume `path` is on keeps named streams; false when it can't be asked.
pub fn has_streams(path: &str) -> bool {
    // S0: card B1 fills this in.
    let _ = path;
    false
}
