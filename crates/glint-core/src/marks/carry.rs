//! Marks follow what Glint copies and moves (PLAN.md 6.3, F21). Between drives that keep streams Windows'
//! engine and `CopyFileExW` take them along; where the homes differ (NTFS to exFAT and back) they are read at
//! the source and written at the destination, and a move takes them out of the source's sidecar or the ledger.

/// After Glint copied (`moved` false) or moved each `(source, destination)` pair: their marks where the homes
/// differ.
pub fn carry(pairs: &[(String, String)], moved: bool) {
    // S0: card B3 fills this in.
    let _ = (pairs, moved);
}
