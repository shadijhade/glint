//! The preview pane and Quick Look: which previewer a file gets, the pane beside the files, the Quick Look
//! overlay (Space), pictures, and Windows' own preview handlers (FilePreview.tsx, Preview.tsx,
//! PreviewState.ts, QuickLook.tsx). Wave 7 slot: the preview-frame agent owns this file and any it adds here,
//! except the previewers below, which the preview-text agent owns.

/// Text and code, highlighted (FilePreview.tsx's text view, highlighter.ts).
pub mod text;
/// Markdown, rendered (MarkdownView.tsx without Mermaid and math, which come later).
pub mod markdown;
/// Bytes as hex (HexView.tsx, BinaryPreview.tsx).
pub mod hex;
/// JSON, YAML and TOML as a tree; CSV and TSV as a table (DataPreview.tsx, DataTree.tsx).
pub mod data;
