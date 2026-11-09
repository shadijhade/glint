# Native replacements for Glint's web-based previews (design note)

> **Coordinator defaults (2026-11-09), until the user decides otherwise:**
> - Typst ships first as an on-demand tool (like Glint's other tools), not linked in.
> - "Good enough" fidelity is accepted for rich HTML mail and Graphviz layouts, with plain-text fallback and "Open in its app".
> - When Windows lacks a codec (HEVC, AV1, VP9 extensions), Glint offers the Store extension and falls back to FFmpeg if it's installed.
>
> **Open for the user:**
> - Is SIL OFL font data (KaTeX's fonts for math, optional Typst fonts) acceptable? It isn't on deny.toml's list.
> - Does Blitz (a native Rust HTML/CSS renderer with no JavaScript and no WebView, still in beta) count as "WebView" under the no-WebView rule?


Date: 2026-11-09. Scope: ten previews that today rely on WebView2 / JS libraries and must become native Rust or Windows-native in the GPUI build. No code in this note.

## 0. How to read this

- Versions, dates and licenses come from the crates.io API (`https://crates.io/api/v1/crates/<name>`) and GitHub on 2026-11-09. Each crate page is `https://crates.io/crates/<name>`. Dates are the date of the newest version unless stated.
- "Verified" means I read the manifest, README or source (I downloaded the `gpui-component` 0.9.1 and `gpui-base` 0.9.1 tarballs and read them). "Unverified" means it is from memory or inference and needs a spike before relying on it. Sizes are crate tarball sizes or estimates unless stated; nothing here was built or measured on this PC.
- License allow list as given. Two traps found while researching: the `epub` crate is GPL-3.0-or-later (cannot link), and `merman` bundles an Eclipse ELK port under EPL-2.0 (not on the list) behind a default feature that must be switched off. Embedded font data (KaTeX fonts, Typst's bundled fonts) is typically SIL OFL-1.1, which is also not on the list; the list is about code, but fonts need a decision (section 6, section 10).

## 1. Cross-cutting findings that shape every section

1. GPUI can already show SVG and raster images (`gpui::Image` with `ImageFormat::Svg`, rasterised by resvg inside GPUI; `gpui-component` pulls `resvg` 0.45 on Windows). So the universal "native preview" pipeline is: produce SVG or RGBA in Rust, hand it to GPUI as an image element. Mermaid, Graphviz, math and 3D all fit this.
2. `gpui-component` 0.9.1 (Apache-2.0, updated 2026-11-09) delegates rich text to `gpui-base` 0.9.1 (Apache-2.0). Verified in source: `TextView` renders Markdown (via the `markdown` crate 1.0) and an HTML subset (html5ever + markup5ever_rcdom). The HTML subset recognises: a, b, blockquote, br, code, del, details, div, em, h1-h6, hr, i, img, li, mark, ol, p, pre, s, span, strong, style(attribute), summary, table/thead/tbody/td/th, u, ul. It parses only `background-color` out of `style=` (a TODO says "use cssparser"). It has no CSS cascade, no `<style>` sheet handling, no `dir`/`lang` attributes found, no floats, no nested layout beyond tables. It has block and inline plugin hooks (`MarkdownPlugin`, `TextViewPlugin`, `InlineElement`, tests mention "formulas" selected and copied inline), and Markdown math nodes (`$..$`, `$$..$$`) are parsed and handed to a plugin if one claims them, otherwise flattened to text. This is the integration point for math, Mermaid and DOT blocks. Source: tarballs at `https://crates.io/api/v1/crates/gpui-component/0.9.1/download` and `.../gpui-base/0.9.1/download`.
3. Bidi/RTL: the grep found only selection-painting code that is bidi-aware; there is no explicit paragraph-direction support in `gpui-base` text. Glint already vendors an RTL-fixed GPUI, so Arabic text shaping and bidi runs at the line level are presumably handled by the vendored text system, but paragraph direction (right-aligned, mirrored lists/tables) will need to be added in our fork of the text view. Treat this as the main RTL cost in sections 1, 2, 7 and 10.
4. Zed itself replaced `mermaid-rs` with `merman` (merged 2026-05-27, `https://github.com/zed-industries/zed/pull/57644`): merman SVG, post-processed with a quick-xml pass, rasterised through usvg/resvg inside GPUI. Zed's approach is the best existing precedent for sections 4, 5 and 6 (SVG in, GPUI image out). Zed's code is GPL/AGPL-licensed overall, so we learn from the design (and the PR description lists the clean-up passes needed), we do not copy it.

---

## 2. EPUB books (today: foliate-js)

### Reading the container
An EPUB is a zip, `META-INF/container.xml` points to the OPF (manifest, spine, metadata), plus a nav document (EPUB 3) or NCX (EPUB 2) for the table of contents. Only the `epub` crate is a poor fit because of its license.

| Name | Version | Date | License | Maturity | Pure Rust | Notes |
|---|---|---|---|---|---|---|
| epub | 2.1.6 | 2026-11-01 | GPL-3.0-or-later | popular (306k dl) | yes | Disqualified: GPL. |
| rbook | 0.7.12 | 2026-10-10 | Apache-2.0 | active, 72k dl | yes | "Format-agnostic ebook library for reading, building and editing EPUB 2/3". Best candidate. |
| epub-parser | 0.3.4 | 2026-02-07 | MIT | 586k dl | yes | Metadata, TOC, text, cover, images only. |
| iepub | 1.3.8 | 2026-11-05 | MIT | 27k dl | yes | EPUB and MOBI read/write; Chinese-focused docs. |
| DIY: zip 9.5.0 + roxmltree 0.21.2 | | 2026-11-09 / 2026-10-24 | MIT / MIT OR Apache-2.0 | very mature | yes | About 300-500 lines for container, OPF, nav/NCX. Glint already has a zip dependency in other tools. |

Sources: `https://crates.io/crates/rbook`, `/epub`, `/epub-parser`, `/iepub`, `/zip`, `/roxmltree`.

### Rendering chapter XHTML natively
Options:
- A. Feed chapter XHTML into `gpui-base` `TextView` HTML mode. Works for typical novels (p, h1-6, em, strong, blockquote, img, ul/ol, table). Fails or degrades for: CSS from the book (drop caps, indents, font choices), `<svg>` cover pages, MathML, `<ruby>`, footnote popups (epub:type="noteref"), `dir="rtl"` / `xml:lang="ar"`.
- B. Own converter XHTML to an internal block/inline model (using `html5ever` or `quick-xml`, plus `simplecss` 0.2.2 Apache-2.0/MIT, or `cssparser` 0.38.0 MPL-2.0 for a small CSS subset: font-style, font-weight, text-align, text-indent, margin, direction, display:none) and render with GPUI's own text system via `parley` 0.12.0 (Apache-2.0 OR MIT) or the existing GPUI text layout. This gives control of RTL, paging and selection, at roughly M-L cost.
- C. `html2text` 0.17.4 (MIT) gives plain text only; useful for search indexing and TOC titles, not display.

### Images, TOC, paging
- Images: resolve manifest hrefs inside the zip, decode with the `image` crate Glint already has; SVG images through resvg. Cover from `properties="cover-image"`.
- TOC: nav `<ol>` tree or NCX `navMap`, mapped to spine item + fragment id. Fragment anchors need the renderer to keep an id-to-scroll-offset map.
- Right-to-left books: read `<spine page-progression-direction="rtl">` and each chapter's `dir`/`xml:lang`. Arabic books will have `dir="rtl"` on `html`/`body` and often `text-align: justify`. Requires: paragraph base direction, right alignment, mirrored list markers and table column order, page turning direction (right-to-left means "next" is left). This is the part no off-the-shelf crate does; budget it in option B.
- Paging vs scrolling: scrolling is trivial (one scroll view per chapter, lazy chapters). Paging (CSS-columns style) needs line-breaking measured to a page height; with a custom block model (option B) it is a pagination pass over laid-out lines. Recommend shipping scroll mode first, paged mode second.

### Recommendation
Use rbook (Apache-2.0) or the DIY zip+roxmltree reader for the container, option B for rendering with option A as the quick first milestone (get Latin-script books on screen in days using `TextView`, then replace with the own converter once RTL and CSS gaps are felt). Decide rbook vs DIY after a one-day spike: DIY avoids a dependency whose API is still 0.x.

Cost: M for reader plus scroll view via TextView; L if paged mode and full RTL layout are required.
Risks: the HTML subset silently drops content; fixed-layout and media-overlay EPUBs are out of scope; DRM'd books fail anyway; RTL paragraph handling lives in our fork of the text view.

---

## 3. Word .docx (today: docx-preview)

| Name | Version | Date | License | Maturity | Pure Rust | Notes |
|---|---|---|---|---|---|---|
| docx-rs | 0.4.22 | 2026-07-21 | MIT | 3.9M dl, active | yes | Primarily a writer with a reader (`read_docx`) into a full document model (paragraphs, runs, tables, images, numbering, styles, section props). Also compiles to wasm. |
| docx-rust | 0.1.11 | 2026-01-22 | MIT | 2.3M dl | yes | Parse and generate; model is closer to the raw XML. |
| undocx | 0.5.2 | 2026-03-23 | MIT | tiny | yes | DOCX to Markdown converter. Lossy but gives a quick text view. |
| docx-lite | 0.2.0 | 2025-09-27 | MIT OR Apache-2.0 | 120k dl | yes | Text extraction only. |
| litchi | 0.0.1 | 2025-10-19 | Apache-2.0 | experimental | yes | Claims Office and iWork parsing; far too young. |
| DIY: zip + quick-xml | | | MIT | | yes | `word/document.xml` is simple; a rendering-oriented reader of w:p / w:r / w:tbl / w:drawing is about 1,500 lines. |

Sources: `https://crates.io/crates/docx-rs`, `/docx-rust`, `/undocx`, `/docx-lite`, `/litchi`.

### Rendering
- Paragraphs and runs map well to GPUI styled text (bold, italic, underline, strike, colour, size, font family, highlight, super/subscript). Lists via numbering definitions (w:numPr / numbering.xml), headings via style names, tables via w:tbl with gridSpan/vMerge and shading, images via w:drawing inline (a:blip r:embed to `word/media/*`). Page geometry (w:sectPr) can be shown as a "paper" card with margins for fidelity, or ignored for a continuous reflowed view. Recommend reflow with page-width cap; it is what a preview needs.
- Skipped initially: text boxes, SmartArt, charts (draw a placeholder), equations (OMML; could convert to LaTeX-ish and feed RaTeX later), tracked changes (show final), headers/footers (optional), footnotes (append at end).
- RTL: `w:bidi`, `w:rtl` and `w:jc="start/end"` are explicit in docx, so direction is better specified here than in HTML. Map `w:bidi` to paragraph base direction; same fork-of-text-view requirement as EPUB.

### Can Windows render it?
No usable path without Office. Office preview handlers (Preview Handler for .docx, `IPreviewHandler`) are registered by Microsoft Office or Word Viewer, so they are absent on a plain Windows install; the existing Quick Look-style preview would show nothing. WordPad was removed from Windows 11 24H2. `Windows.Data.Pdf` renders PDF only. Optionally, if the user has LibreOffice installed, `soffice --headless --convert-to pdf` can be an opt-in external process (MPL-2.0 product, process-only use is fine), then Glint's existing PDF preview shows exact pages. Treat it as a "high fidelity" menu action, not the default. (Unverified: Word-preview handler presence on a given machine; check `HKCR\.docx\ShellEx\{8895b1c6-b41f-4c1c-a562-0d564250836f}` before relying on this either way.)

### Recommendation
docx-rs reader (it is the maintained, widely used option with a full model) mapped into the same block/inline model as the EPUB renderer, so one rich-text view serves both; DIY only if docx-rs's reader loses too much (check with a few real Arabic documents). Keep undocx as a cheap fallback for "convert to Markdown" and search.

Cost: M (paragraphs, runs, lists, tables, images) plus S-M for RTL.
Risks: docx-rs reader drops unknown elements silently and its model is writer-shaped (sometimes awkward to read back); complex layouts (floating images, columns) will not look like Word; fonts missing on the machine (Calibri exists on Windows, fine).

---

## 4. 3D models: glTF/GLB, OBJ, STL, PLY, 3MF (today: three.js)

### Loaders (all pure Rust)

| Format | Crate | Version | Date | License | Notes |
|---|---|---|---|---|---|
| glTF/GLB | gltf | 1.4.1 | 2024-05-10 | MIT OR Apache-2.0 | 11.6M dl; stable though old; skip Draco/meshopt compression (KHR_draco_mesh_compression not decoded; flag as "unsupported"). |
| OBJ | tobj | 4.0.5 | 2026-08-02 | MIT | 3.3M dl, active; MTL materials. |
| STL | stl_io | 0.11.0 | 2026-03-15 | MIT | ASCII + binary. |
| PLY | ply-rs-bw | 4.0.1 | 2026-07-24 | MIT | Maintained fork; original `ply-rs` 0.1.3 (2020-08-27) is dormant. |
| 3MF | threemf | 0.8.1 | 2026-10-20 | 0BSD | Active; read/write; 9k dl. |
| 3MF | lib3mf | 0.1.7 | 2026-10-23 | MIT | "Pure Rust 3MF parsing and writing"; larger feature set (beam lattice, slices), 3k dl. |

Sources: `https://crates.io/crates/gltf`, `/tobj`, `/stl_io`, `/ply-rs-bw`, `/threemf`, `/lib3mf`. 3MF is a zip of XML; a DIY reader with zip + quick-xml for vertices/triangles/components/build transforms is about 300 lines if the crates prove awkward.

### Rendering approaches

| Approach | Version/lib | License | Pure Rust | Cost/size | Orbit at 30 fps, several hundred thousand triangles? |
|---|---|---|---|---|---|
| CPU rasteriser to RGBA (own: transform, z-buffer, Lambert + matcap shading, rayon tiles) | rayon etc. | n/a | yes | no GPU, tiny; +0 deps | Borderline. 300k triangles at 960x540 needs about 9M triangle setups per second plus fill; multi-threaded tile binning can reach 30 fps on a modern 8-core but burns CPU, and drag latency suffers. Fine for static/auto-rotate, weak for large meshes. Mitigation: decimate or render a bounding/point proxy while dragging, full quality on release. |
| Offscreen wgpu, readback to GPUI image | wgpu 30.0.1 (2026-08-22, MIT OR Apache-2.0) | ok | yes | Needs a second GPU device next to GPUI's D3D11 device; adds wgpu-core/hal/naga to the binary (my estimate +4-8 MB, unmeasured); runtime memory for a second device unmeasured (spike). Readback of 1280x720 BGRA is 3.7 MB per frame, trivial at 30 fps. | Yes, GPU handles millions of triangles. |
| Offscreen raw Direct3D 11 with the `windows` crate (0.62.2, MIT OR Apache-2.0), HLSL via `D3DCompile`, staging-texture readback | windows crate already in tree | ok | no unsafe-free, but no new deps | No new crates; second D3D11 device on the same adapter (can use `D3D_DRIVER_TYPE_WARP` as fallback when no GPU). Smaller than wgpu. | Yes. |
| Same-device texture sharing with GPUI (zero copy) | requires patching vendored GPUI | ok | | We already vendor GPUI, so we could expose its `ID3D11Device` and let a custom element draw an external texture. Best performance; deeper coupling. | Yes, best. |
| High-level engines: three-d 0.19.0 (MIT, 2026-04-17), kiss3d 0.47.0 (BSD-3-Clause, 2026-10-04), bevy_render 0.20.1 (MIT OR Apache-2.0) | | ok | yes | They bring windowing/GL/wgpu layers, large dependency trees; not designed for offscreen embedding. | Not recommended. |
| Windows' own 3D | 3D Viewer / Print 3D / `Windows.Graphics.Printing3D` | | | Microsoft deprecated 3D Viewer and Print 3D in 2024 (unverified date) and they are not on clean installs; Printing3D is a model/package API, not a renderer. | Not viable. |

### Recommendation
Phase 1: loaders to a common indexed mesh (positions, normals, optional vertex colours, optional base-colour texture), then a CPU rasteriser for thumbnails and auto-rotate, plus automatic decimation to about 150k triangles while the user is dragging. Phase 2, only if drag feels poor on real models: an offscreen raw D3D11 renderer (lazily created on first 3D preview, destroyed on close), with CPU readback into a GPUI image. Prefer raw D3D11 over wgpu to avoid new dependencies and a second abstraction stack; choose wgpu only if the team wants shader portability or an eventual non-Windows port (wgpu is the cleaner code, D3D11 is the smaller footprint). Zero-copy shared device is a later optimisation.

Cost: S for loaders + unified mesh; M for CPU rasteriser with orbit/zoom/pan and shading; M for D3D11 renderer; L for shared-device integration.
Risks: gltf crate is two years old with no recent releases (reads fine; PBR materials and animations are out of scope); no Draco/KTX2/meshopt; very large meshes need decimation; color management and textures in 3MF (materials extension) incomplete; the second D3D11 device adds GPU memory (measure) and WARP fallback is slow.

---

## 5. Mermaid diagrams in Markdown (today: mermaid.js)

| Name | Version | Date | License | Maturity | Pure Rust | Notes |
|---|---|---|---|---|---|---|
| merman | 0.9.3 stable / 0.10.0-alpha.2 | 2026-11-06 | MIT OR Apache-2.0 (default feature set bundles an ELK port, EPL-2.0) | very active, 244k dl, 659 stars, used by Zed | yes | Parity with Mermaid 12.3.0 (pinned reference), diagram-family cargo features, SVG output (+PNG/JPEG/PDF/ASCII features), themes, deterministic text measurement with pluggable callback, typed diagnostics, "rendering security" and resource limits docs. |
| mermaid-rs-renderer (mmdr) | 0.3.1 | 2026-07-06 | MIT | 256k dl | yes | 23 diagram types (flowchart, sequence, class, state, ER, pie, XY, quadrant, sankey, gantt, timeline, journey, kanban, C4, block, architecture, requirement, mindmap, git graph, ZenUML, packet, radar, treemap). README says visual quality "may not yet match mermaid-cli". Very fast (about 1-50 ms). |
| selkie-rs | 0.3.0 | 2026-02-07 | MIT | 1.8M dl, "still actively under development" | yes | Rust mermaid parser and renderer, evaluation harness against mermaid.js, also WASM. Built largely by coding agents. |
| mermaid-render | 0.10.0 | (recent) | MIT | 68k dl | yes | Related to the merman ecosystem (unverified relationship). |
| mermaid-rs | 0.1.1 | 2024-10-27 | (not checked) | 5.9k dl | no | Dormant; Zed replaced it. |
| mermaid-parser | 0.1.0 | 2025-06-06 | | tiny | | Parser only. |

Sources: `https://crates.io/crates/merman`, `https://github.com/Latias94/merman`, `https://github.com/zed-industries/zed/pull/57644`, `https://crates.io/crates/mermaid-rs-renderer`, `https://github.com/1jehuang/mermaid-rs-renderer`, `https://crates.io/crates/selkie-rs`, `https://github.com/btucker/selkie`.

Merman license detail (verified in its `THIRD_PARTY_NOTICES.md` and `docs/rendering/RUST_EMBEDDING.md`): defaults include the `layout-elk` feature (a source translation of Eclipse ELK, EPL-2.0). Build with `default-features = false, features = ["svg", "layout-cytoscape", "diagram-*"...]`, or `complete-svg`, never `complete-svg-elk`. Without ELK, ELK-requesting diagrams fall back to Dagre, which is the common case anyway. The math feature links RaTeX (MIT) and embeds KaTeX fonts; leave it off at first. Mermaid's own bundled components (dagre, cytoscape, roughjs, d3-shape etc.) are MIT/BSD/ISC family. Run `cargo deny` against the final feature set before shipping.

### Rendering path
merman (SVG string) then a post-processing pass (Zed needed CSS injection and fixes so usvg/resvg handle foreignObject-style HTML labels; merman exposes a "resvg-safe pipeline" in its docs) then GPUI `Image::Svg`. Provide the host fonts to the text measurer so labels fit the actual rendered font: Segoe UI for Latin, and an Arabic-capable fallback (Segoe UI covers Arabic) so Arabic node labels are measured correctly. Text shaping in resvg uses rustybuzz/fontdb through usvg's text feature, which handles Arabic shaping.

### Recommendation
merman, ELK off, as the primary renderer, with mmdr (MIT, tiny, fast) as a second choice if merman proves too heavy to compile or too slow (it also covers 23 types). Fallbacks when parsing fails or a diagram type is unsupported: show the fenced code block with a small "diagram could not be rendered: <diagnostic>" banner (merman returns typed diagnostics), never an empty box. Add a render timeout and cancellation (merman has `OperationControl`) and cache SVG by hash of source plus theme.

Cost: M (integration, theme mapping to Glint palette, SVG post-pass, caching, error UI).
Risks: merman is pre-1.0 with an alpha line (pin an exact version; the stable 0.9.3 vs 0.10 alpha decision should favour stable); output uses CSS that resvg only partly supports (Zed's PR describes the clean-up passes: we will have to rewrite similar ones); HTML labels (foreignObject) are not supported by resvg; icons/network features off; EPL-2.0 ELK must not ship. Visual parity is not 100% by the project's own admission.

---

## 6. Graphviz DOT files and blocks (today: @hpcc-js/wasm-graphviz)

| Name | Version | Date | License | Maturity | Pure Rust | Notes |
|---|---|---|---|---|---|---|
| layout-rs | 0.1.3 | 2025-04-24 | MIT | 890k dl, 740 stars, last push 2025-05 | yes | Parses DOT and lays out/renders to SVG itself (Sugiyama-style with crossing minimisation, records, shapes, Unicode and RTL labels shown in its gallery). Dormant but stable. Subset of DOT (no ports on all shapes, limited clusters, no HTML-like labels, no neato/fdp/twopi). |
| dot-parser | 0.6.1 | 2026-01-07 | GPL-2.0-or-later | | yes | Disqualified (GPL). |
| graphviz-rust | 0.9.9 | 2026-09-14 | "non-standard" in metadata (check repository) | 2.5M dl | partly | DOT AST printer/parser; rendering shells out to the `dot` binary. Needs external Graphviz (EPL-1.0, process only). |
| fdp | 0.3.0 | 2025-02-24 | (not checked) | 17k dl | yes | Force-directed layout crates; not DOT-compatible. |
| rust-sugiyama | 0.4.0 | 2025-09-21 | MIT | | yes | Layered layout algorithm only (no DOT, no rendering). |
| merman's Dagre layout | see section 5 | | | | yes | Could be reused as layout engine with our own DOT parser; speculative. |

Graphviz itself is EPL-1.0 (unverified exact version), which is not on the allow list; compiling real Graphviz into WASM and running it under wasmtime/wasmi would avoid JS but not the license, so it is not recommended.

### Recommendation
layout-rs for parse and layout, output SVG, rasterised by resvg inside GPUI. Wrap it: size cap (node/edge count), timeout on a worker thread, cache by hash. For DOT features it cannot handle (HTML-like labels, `splines=ortho`, neato/fdp/circo engines), show a banner "simplified layout" or the source with an "Open with Graphviz" action when `dot.exe` is found on PATH (optional external process, same pattern as the other Glint extra tools; Graphviz is detected, never bundled). If layout-rs's maintenance risk worries the team, vendoring it (MIT) into the repo is cheap: the code is about 5k lines.

Cost: S for the layout-rs integration; M if we fork it to fix gaps (ports, clusters, RTL edge cases).
Risks: abandoned upstream, layout not identical to Graphviz (users will notice for big graphs), crash/hang on pathological input (run in a thread with a time budget).

---

## 7. Math in Markdown, LaTeX syntax (today: temml producing MathML)

Key point: MathML needs a layout engine, so "LaTeX to MathML" crates are not enough. We need something that outputs glyph positions or SVG/image.

| Name | Version | Date | License | Maturity | Pure Rust | Output | Notes |
|---|---|---|---|---|---|---|---|
| RaTeX (ratex-parser/layout/svg/render) | 0.1.15 | 2026-10-16 | MIT (code) | young (1.6k stars, active), small | yes | display list, SVG (ratex-svg, uses resvg + optional KaTeX web fonts), PNG (tiny-skia), PDF | "KaTeX-compatible math rendering engine in pure Rust, no JS, no WebView, no DOM." Supports mhchem `\ce`, `\pu`, proof trees. Embedded KaTeX fonts (ratex-katex-fonts, crate says MIT, but KaTeX_*.ttf are SIL OFL-1.1 upstream; verify NOTICE). Repo license metadata shows NOASSERTION on GitHub. Same engine used inside merman's optional `math` feature. |
| Typst math via mitex + typst | mitex 0.2.4 (2024-06-17, Apache-2.0); typst 0.15.1 (2026-07-17, Apache-2.0); typst-svg/typst-render 0.15.1 | | Apache-2.0 | typst mature; mitex has had no release for 17 months | yes | SVG/PNG | Best typographic quality (OpenType MATH tables) and RTL-capable, but heavy: embeds the Typst compiler (Windows CLI zip is 21 MB, so estimate +12-20 MB in the exe) and bundled fonts (typst-assets 6.7 MB crate, fonts are OFL/free but not on the allow list as code licenses). Latency per formula is tens of ms with a warm world. |
| pulldown-latex | 0.8.0 | 2026-07-28 | MIT | 106k dl | yes | MathML Core only | A parser/renderer to MathML, which we cannot display. Only useful as a validator. |
| katex-rs | 0.3.0 | 2026-09-05 | MIT | 178k dl, reimplements KaTeX 0.18.5 in Rust | yes (no JS engine; verified by README and workspace manifest) | HTML + MathML strings | Needs a browser to lay the spans out. Not usable without WebView. Exclude. |
| katex (crate) | 0.4.6 | 2023-02-05 | MIT | | no | Embeds QuickJS/Duktape/wasm-bindgen JS | Excluded: embeds a JS engine. |
| latex2mathml | 0.2.3 | 2020-04-27 | MIT | dormant | yes | MathML | Excluded. |
| "ReX" crates.io name | 4.0.0 | 2026-08-24 | MIT | | | | This name is now used by an unrelated workflow-language project; the old ReX math engine (ReTeX) is unmaintained. Exclude. |

Sources: `https://crates.io/crates/ratex-render`, `https://github.com/erweixin/RaTeX`, `https://crates.io/crates/mitex`, `https://crates.io/crates/typst`, `https://crates.io/crates/pulldown-latex`, `https://crates.io/crates/katex-rs`, `https://github.com/katex-rs/katex-rs`.

### Recommendation
RaTeX as primary: pure Rust, SVG output that GPUI shows through resvg, and a growing KaTeX-compatible command set, at a small size cost (single-digit MB with the bundled font, estimate). Hook it into `gpui-base`'s Markdown math nodes and inline plugin API so inline formulas flow with text and display formulas become block elements; cache by (source, size, color). If a formula fails to parse, render the raw LaTeX in a monospace, tinted span. Typst+mitex stays the fallback only if Typst is adopted anyway for PDF export (section 11), in which case it can render the hard cases for free.

Cost: S-M (plugin, sizing/baseline alignment, theme colour in SVG, caching).
Risks: young project (0.1.x) with API churn, one maintainer; GitHub shows no SPDX for the repo, crates say MIT: confirm the LICENSE file and font notices before shipping; baseline alignment of inline SVG with surrounding text needs measured ascent/descent; no Arabic math layout (RTL math notation) in either engine; the KaTeX fonts' OFL licence needs a policy decision (fonts are data, but the allow list should be amended or the fonts replaced by the user's choice).

---

## 8. Email: .eml HTML bodies and .msg (today: postal-mime, msgreader, rendered as HTML)

| Name | Version | Date | License | Maturity | Pure Rust | Notes |
|---|---|---|---|---|---|---|
| mail-parser | 1.0.1 | 2026-11-02 | Apache-2.0 OR MIT | 5.4M dl, Stalwart Labs | yes | RFC 5322/MIME, charsets (many legacy encodings), attachments, inline images with Content-ID, `message/rfc822` nesting, RFC 2047 headers, text/html parts. Best choice. |
| msg_parser | 0.3.6 | 2026-04-15 | MIT | 346k dl | yes (own CFB reader; deps: hex, regex, serde, serde_json, thiserror) | Parses .msg (OLE) metadata, bodies, recipients, attachments, transport headers per MS-OXMSG/OXPROPS. Unverified: compressed-RTF body decoding, HTML body property (0x1013 PR_HTML) coverage, embedded .msg. |
| cfb | 0.16.0 | 2026-10-31 | MIT | 83.9M dl | yes | Low-level Compound File Binary reader. Use as a fallback to read properties directly (`__substg1.0_1013001F` HTML, `__substg1.0_1000001F` body, `__properties_version1.0`, attachment storages). |
| outlook-pst | 1.2.0 | | MIT | Microsoft | yes | PST/OST, not needed now. |
| html2text | 0.17.4 | 2026-10-18 | MIT | 7.1M dl | yes | HTML to wrapped plain text, handles tables; an excellent fallback body. |
| ammonia | 4.2.4 | 2026-10-17 | MIT OR Apache-2.0 | | yes | html5ever-based sanitiser: allowlist tags/attributes, strips scripts/styles/iframes/forms, can block remote URLs via URL schemes/`url_relative` policy and `attribute_filter`. |

Sources: `https://crates.io/crates/mail-parser`, `/msg_parser`, `/cfb`, `/html2text`, `/ammonia`.

### Rendering HTML bodies without a browser
Marketing emails are table-heavy with inline CSS, background images and `<style>` blocks. `gpui-base` HTML subset (section 1) handles simple mail (paragraphs, links, lists, simple tables, images), not layout tables with colspans and styled cells beyond `background-color`. Pipeline: ammonia sanitise (remove script/style/link/iframe/object/form/meta refresh; drop `http(s)` `img src`, `background`, CSS `url()`; keep `cid:` and `data:`) then TextView HTML mode, with a per-message "Load remote images" toggle that is off by default and, when turned on, fetches through Glint's own HTTP client with no cookies, no referrer, and a size cap. Always offer a "plain text" tab (text/plain part, else html2text). Mailto, http links: open with confirmation via ShellExecute, never inside the preview. Inline attachments (`cid:`) resolve from MIME parts or .msg attachment storages. Show headers (From, To, Cc, Date, Subject), attachment list with save/open actions, and charset-correct Arabic subjects/bodies (mail-parser decodes; add RTL direction detection per paragraph for Arabic mail).

Security: no remote content by default (tracking pixels), no scripts (ammonia), no external stylesheet, no iframes/objects, size cap on parsed input (zip-bomb style nested messages), depth cap for nested rfc822, treat `.msg` OLE parsing as untrusted input (run parsing in a worker with panic catch).

### Recommendation
mail-parser + msg_parser (with cfb as fallback), ammonia + TextView HTML, html2text fallback. Cost: M. Risks: fidelity of rich HTML mail will be visibly worse than a browser; msg_parser maintenance by one author; confirm .msg HTML/RTF body support on real Outlook files (compressed RTF to HTML conversion may need a small decompressor; the MS-OXRTFCP LZFu algorithm is about 150 lines).

Excluded by policy: Blitz (DioxusLabs, `blitz-html` 0.3.0-beta.4, MIT OR Apache-2.0) is a genuine HTML/CSS engine (Stylo, Taffy, Vello) with no JS; it would give far better email fidelity but falls under "no HTML engine of any kind". Noted here only so the decision is explicit.

---

## 9. Terminal pane and asciinema .cast playback (today: xterm.js / ghostty-web)

| Name | Version | Date | License | Maturity | Pure Rust | Notes |
|---|---|---|---|---|---|---|
| alacritty_terminal | 0.26.0 | 2026-04-06 | Apache-2.0 | 2.6M dl, the engine under Alacritty and Zed's terminal | yes | Grid, scrollback, selection, search, damage tracking, vte-driven. `Term` can be fed bytes directly (`vte::ansi::Processor`), so its own PTY layer (which has a ConPTY backend) can be skipped. Used by `gpui-terminal`. |
| vte | 0.15.0 | 2025-02-02 | Apache-2.0 OR MIT | 93.6M dl | yes | Parser only. |
| wezterm-term (not on crates.io by that name) | git only | | MIT | | yes | Fork `tattoy-wezterm-term` 0.1.0-fork.5 (2025-07-11) exists; richer features (sixel, kitty graphics, hyperlinks) but heavy and not published upstream. |
| termwiz | 0.23.3 | 2025-03-20 | MIT | 32M dl | yes | wezterm's terminal-cell library; escape parser and surface, not a full emulator for GPUI use. |
| vt100 | 0.18.1 | 2026-10-19 | MIT | 18.7M dl | yes | Small screen-grid emulator. Simple but limited (no scrollback API richness). |
| avt | 0.20.0 | 2026-11-04 | Apache-2.0 | 317k dl | yes | asciinema's own virtual terminal. Perfect for .cast playback and for seekable rendering (feed events up to time t into a Vt, read lines). |
| gpui-terminal | 0.1.0 | 2025-12-24 | MIT OR Apache-2.0 | 2.6k dl | yes | A GPUI terminal element built on alacritty_terminal 0.25 + portable-pty 0.9 (+ arboard, ropey) against the registry `gpui` 0.2.2. A usable precedent for the element design; cannot be consumed as-is (different GPUI). |
| portable-pty | 0.9.0 | 2025-02-11 | MIT | 27M dl | yes | Already in Glint (ConPTY). |
| gpui-libghostty / gpui-ghostty | 0.4.1-rc.1 / 0.0.1 | 2026-11-02 / 2026-08-04 | (not checked) | | no (libghostty is Zig/C) | Not pure Rust; skip. |
| Zed `terminal` / `terminal_view` | in zed repo | | GPL-3.0 | | | Cannot copy; learn architecture only (a model of the terminal state, a custom `Element` that paints cell runs, batching adjacent cells of equal style into a text run, cursor and selection layers, key-to-escape mapping). |

Sources: `https://crates.io/crates/alacritty_terminal`, `/avt`, `/gpui-terminal`, `/vt100`, `/termwiz`, `/portable-pty`.

### Recommendation
Architecture: portable-pty (existing ConPTY wrapper) reads child output on a thread, forwards bytes over a channel to an `alacritty_terminal::Term` driven by `vte::ansi::Processor`; a GPUI `Element` paints visible cells using the damage list, grouping runs by style into shaped text runs, drawing background rects, cursor, selection and scrollbar; key events map to escape sequences (application cursor mode, modifier encodings, bracketed paste), mouse reporting, IME. Use `gpui-terminal` (MIT OR Apache-2.0) as reference for the element/event wiring. For .cast playback: parse asciicast v2/v3 (JSON lines, trivial) and drive `avt::Vt` (or the same alacritty `Term` for consistent rendering with the live terminal; avt has proven asciinema fidelity). Recommend: playback via the same alacritty-based view for one render path, using avt only if alacritty's resize/reflow behaviour on seek misrenders recordings. Timeline scrubber: pre-index absolute times; seek by resetting and replaying up to t (fast, since events are cheap), with periodic snapshots if recordings are long.

RTL/Arabic in a terminal grid is a known hard problem (no terminal does it well); out of scope, mention in the UI.

Cost: L for the live terminal (rendering, input, selection, IME, scrollback, links, performance); S-M for .cast playback on top of it.
Risks: performance under heavy output (needs batched repaint at most 60 Hz, parsing off the UI thread); Windows specifics (ConPTY quirks with resize and Unicode width, bracketed paste); alacritty_terminal's API changes between minor versions (pin); font fallback for wide/emoji cells; accessibility.

---

## 10. Video and audio preview (today: HTML5 video)

### Facts about the compositing problem
GPUI windows are `WS_EX_NOREDIRECTIONBITMAP`, drawn with Direct3D 11 into a DirectComposition swap chain. A phase-0 test showed GDI child windows are invisible there, while WebView2's DirectComposition child displayed. Conclusion: anything that wants to appear inside the window must either (a) be composed into the same DComp tree, or (b) be drawn by GPUI itself. A plain `CreateWindowEx(WS_CHILD)` with its own DXGI swap chain is not guaranteed to appear (unverified; the test only covered GDI children, but a child HWND is composed through the redirection surface that this window lacks, so I would expect the same failure) and should not be the plan.

### Options

| Approach | Visible in GPUI window? | Notes |
|---|---|---|
| A. `IMFMediaEngine` in frame-server mode: create via `IMFMediaEngineClassFactory` with a DXGI device manager (`MF_MEDIA_ENGINE_DXGI_MANAGER`), call `TransferVideoFrame` into an `ID3D11Texture2D` on our own device each time `OnVideoStreamTick`/`HasVideoFrame` says a new frame is ready; the engine plays audio, handles A/V sync, seeking, rate, volume, loop, and subtitle text tracks metadata. We copy the frame into a GPUI image. | Yes: GPUI draws the image element itself. | Recommended. Throughput: 1080p BGRA is 8 MB per frame, 30 fps is 250 MB/s via CPU readback (fine); 4K is 33 MB/frame, 60 fps would be 2 GB/s (heavy; downscale to the displayed size in the GPU first with a video processor or a shader before readback, so cost scales with preview size not source). Zero-copy variant: patch vendored GPUI to accept a shared D3D11 texture (`D3D11_RESOURCE_MISC_SHARED_NTHANDLE`) and sample it directly. |
| B. `IMFMediaEngine` in DComp mode (`MF_MEDIA_ENGINE_PLAYBACK_VISUAL` / `GetVideoSwapchainHandle`), attach the visual to GPUI's DirectComposition tree | Only if our vendored GPUI exposes its `IDCompositionDevice`/target and lets us add a sibling visual; WebView2's visibility suggests DComp siblings do show. | Better GPU path and lower CPU, but couples to GPUI internals and layering/z-order with UI chrome (controls drawn over video need the video visual below GPUI's visual, which breaks because GPUI's swap chain is a single opaque surface). Spike only. |
| C. `IMFSourceReader` (software pull) + own audio (WASAPI via `cpal` or `rodio`) | Yes | Full control (subtitle burn-in, filters), but you rebuild A/V sync, buffering, seeking. More code than A. |
| D. External ffmpeg process (Glint already supports the user's FFmpeg as a tool): decode to raw frames over a pipe plus separate audio output | Yes | Covers codecs Windows lacks; poor seeking latency; only as a fallback "play with FFmpeg" for unsupported files. |
| E. GStreamer (`gpui-video-player` 0.1.3, MIT) | Yes | Needs GStreamer runtime install on Windows; LGPL runtime; too heavy. |
| F. `libmpv2` 7.0.1 | | LGPL-2.1: must be an external process only; windowing issues same as above. Skip. |
| G. `ffmpeg-next` 9.0.0 | | WTFPL crate, but links FFmpeg (LGPL/GPL builds); not allowed to link. Skip. |
| H. Audio only: `symphonia` 0.6.1 (MPL-2.0, pure Rust decoders for MP3, AAC, FLAC, Vorbis, WAV, ALAC, MKV/MP4 demux) + `rodio` 0.23.0 (MIT OR Apache-2.0) or `cpal` for output, `lofty` 0.25.5 (MIT OR Apache-2.0) for tags/cover art | Yes | For audio files this is lighter and more controllable than Media Foundation; also yields samples for waveform/spectrum drawing. |

Sources: Media Foundation `IMFMediaEngine` docs (`https://learn.microsoft.com/windows/win32/api/mfmediaengine/nn-mfmediaengine-imfmediaengine`, frame-server mode and `TransferVideoFrame`, unverified details from memory), `https://crates.io/crates/symphonia`, `/rodio`, `/lofty`, `/gpui-video-player`, `/windows`.

### Codec coverage (unverified, from Microsoft docs memory; re-check on the target Windows)
Windows 11 decodes H.264, MPEG-4, WMV, VP8-ish via MF; HEVC needs the "HEVC Video Extensions" Store package (some OEM machines have it free, otherwise paid); AV1 needs the free "AV1 Video Extension"; VP9 needs the free "VP9 Video Extensions" (WebM container); Opus/Vorbis/FLAC/MKV are supported in modern Windows 10/11. The app should probe via `MFTEnumEx` or just catch the engine's `MF_MEDIA_ENGINE_EVENT_ERROR` / format-not-supported event, then show a message with one-click Store links for the extension and an "Open with FFmpeg player" fallback (option D) or default app.

### Seeking, subtitles
Seeking: `SetCurrentTime` plus the `SEEKED` event; scrubbing should use a seek on release with thumbnail preview from the frame-server mode throttled to about 10 Hz. Subtitles: sidecar `.srt` via `srtparse` 0.3.0 (MIT) or `vtt` 1.0.0 (MIT OR Apache-2.0), drawn by GPUI as a text overlay at the bottom (also fixes RTL and font control for Arabic subtitles, which a browser track would not); embedded subtitle tracks in MKV are not exposed by Media Foundation, so embedded subs are an ffmpeg-fallback feature. Audio track selection: limited.

### Recommendation
Approach A for video, option H for audio-only files (so audio previews never need MF or a texture), D as the unsupported-codec fallback, B as an optional later optimisation if profiling demands it. This keeps all pixels inside GPUI and sidesteps the DWM/child-window problem.

Cost: M-L (COM interop with the `windows` crate, event callback object, texture copy and downscale, play/pause/seek/volume UI, extension detection, error paths, audio lifecycle) ; S-M for audio-only with symphonia+rodio.
Risks: frame-server memory copy at high resolutions; `IMFMediaEngine` is documented as video-rendering-friendly but its frame-server path has had driver-specific bugs (spike on this PC, Intel/AMD/NVIDIA); HEVC/AV1/VP9 availability varies per machine; subtitle streams embedded in files not reachable; Media Foundation threading/apartment requirements (MTA init, callbacks arrive on MF threads, marshal to the UI thread).

---

## 11. Export Markdown as PDF (today: WebView2 PrintToPdf)

| Name | Version | Date | License | Maturity | Pure Rust | Notes |
|---|---|---|---|---|---|---|
| Typst (compiler crates: typst, typst-pdf, typst-kit, typst-assets, typst-svg, typst-render) | 0.15.1 | 2026-07-17 | Apache-2.0 | 4M dl, very active, ships its own PDF writer (krilla-based since 0.12) | yes | Full typesetting: real paragraphs, tables, figures, code blocks, page headers/footers, outlines, links, RTL and Arabic shaping, math, SVG images (also from merman/RaTeX), tagged PDF/PDF-A, hyphenation. In-process via the crates (+ fonts) or out-of-process via the CLI (Windows x86_64 zip is 21 MB, 2026-07-17). |
| typst-as-lib | 0.17.0 | 2026-10-07 | MIT | 1.7M dl | yes | A convenient wrapper to compile Typst sources in-process with in-memory files and fonts. |
| krilla | 0.8.2 | 2026-06-04 | MIT OR Apache-2.0 | 3.7M dl | yes | Low-level high-level-ish PDF writer (fonts, subsetting, images, gradients, tagged PDF, links); we would implement layout ourselves. |
| printpdf | 0.12.8 | 2026-09-05 | MIT | 3.9M dl | yes | Direct PDF writer with its own HTML/layout experiments; weaker typography, RTL not handled. |
| pulldown-cmark | 0.13.6 | 2026-10-30 | MIT | 202M dl | yes | The Markdown parser to drive the mapping (GPUI side already uses `markdown` 1.0 crate; the PDF path can use either; pulldown-cmark's event stream is simpler for a streaming writer). |
| hayro | 0.9.0 | 2026-11-09 | Apache-2.0 OR MIT | | yes | PDF *renderer*, only relevant to previewing the result inside Glint, not creating it. |
| Windows "Microsoft Print to PDF" | OS | | n/a | | | Requires drawing via GDI/DirectWrite print contexts: awkward for GPUI content; skip. |

Sources: `https://crates.io/crates/typst`, `/typst-pdf`, `/typst-as-lib`, `/krilla`, `/printpdf`, `https://github.com/typst/typst/releases/latest`.

### Markdown to Typst mapping
Walk pulldown-cmark events and emit Typst markup: headings `=` levels, emphasis `_ _` / strong `* *` (escaped), inline code and fenced code as `raw()` with language (Typst has built-in syntax highlighting via syntect), lists with `-` / `+`, task lists as glyphs, block quotes `#quote`, links `#link("url")[..]`, images `#image("path")` (SVG supported), tables to `#table(columns: n, ..)`, thematic break `#line(length: 100%)`, footnotes `#footnote[..]`, math through RaTeX SVG or mitex, Mermaid/DOT blocks via SVG (render with the same functions the preview uses so output matches). Escape Typst special characters (`#`, `$`, `*`, `_`, `` ` ``, `@`, `<`, `>`, `\`). Set `#set text(dir: rtl, lang: "ar")` automatically when the document is predominantly Arabic and use paragraph-level `#par` direction otherwise; supply a font stack (Segoe UI/Tahoma for Arabic, Consolas for code) via Typst's font search of system fonts (`typst-kit` font searcher) so we do not need to embed fonts. Raw HTML in Markdown: dropped or kept as escaped text.

### Recommendation
Typst. Two ways to ship: (1) the CLI as an optional Extra tool, downloaded on first PDF export, matching how Glint already treats FFmpeg, pdfcpu, etc. (zero binary cost, S-M); (2) the crates in-process behind a cargo feature (estimate +12-20 MB exe, unmeasured; typst-as-lib simplifies; the compile is single-digit hundreds of ms for ordinary docs). I recommend starting with (1) since export is occasional, and moving to (2) only if the tool-download flow annoys users or if the same engine ends up serving math (section 7) so the size is shared. Output PDF/A or tagged PDF is one config line later.

Cost: M for the mapping (tables, nested lists, images, code, RTL are where the time goes); S for wiring the CLI/tool.
Risks: Typst's markup escaping bugs (fuzz with real READMEs); large images; fonts in the PDF are subset from system fonts (license of Segoe UI embedding is permitted for subsetting but check); Typst semantics differ from CommonMark (loose lists, nested block quotes) — need golden tests; the CLI route needs a download and a SHA check; Typst is Apache-2.0, the bundled fonts in typst-assets are OFL-style (they are only needed if we do not rely on system fonts).

---

## 12. Summary table

| # | Preview | Recommended | License | Cost | Biggest risk |
|---|---|---|---|---|---|
| 1 | EPUB | rbook (or zip+roxmltree) reader; render with TextView first, own XHTML-to-block converter with RTL later | Apache-2.0 / MIT | M (L with paging + full RTL) | `epub` crate is GPL; RTL paragraph support in text view; subset HTML |
| 2 | DOCX | docx-rs reader, own mapping to the shared rich-text view; no Office dependence | MIT | M | writer-shaped model, unsupported features, RTL |
| 3 | 3D | gltf + tobj + stl_io + ply-rs-bw + threemf into one mesh; CPU raster first, offscreen raw D3D11 renderer if drag is slow | MIT / Apache / 0BSD | S + M (+ M) | orbit smoothness on big meshes; second device memory |
| 4 | Mermaid | merman (ELK feature off) to SVG to resvg in GPUI; mmdr as second choice | MIT OR Apache-2.0 | M | ELK is EPL-2.0; pre-1.0 alpha; resvg CSS limits |
| 5 | Graphviz | layout-rs to SVG; optional external `dot` | MIT | S | dormant upstream; not full Graphviz |
| 6 | Math | RaTeX to SVG into TextView math plugin; Typst/mitex fallback if Typst is in | MIT | S-M | young crate; font licence (OFL); baseline alignment |
| 7 | Email | mail-parser + msg_parser (+ cfb) + ammonia + TextView HTML; html2text fallback; remote content off | MIT / Apache | M | fidelity of HTML mail; .msg RTF/HTML coverage |
| 8 | Terminal / cast | portable-pty + alacritty_terminal + custom GPUI element; avt or same Term for .cast | Apache-2.0 / MIT | L (live) + S-M (cast) | size of the element work, performance, IME |
| 9 | Video / audio | Media Foundation IMFMediaEngine frame-server into GPUI image; symphonia+rodio for audio; ffmpeg fallback | MIT/Apache/MPL | M-L | codec extensions, CPU copy at 4K, driver quirks |
| 10 | PDF export | Typst (CLI tool first, crates later) with a pulldown-cmark to Typst mapper | Apache-2.0 | M | markup escaping, RTL, Typst size/tool download |

## 13. Suggested order

1. Mermaid + Graphviz + Math together (sections 5, 6, 7): they share the same "SVG string to GPUI image, cached by hash, with error banner" pipeline and the `TextView` plugin hooks; deliver the pipeline once, three previews fall out. Start with merman (the largest uncertainty is the SVG post-pass), then layout-rs, then RaTeX.
2. Email (section 8): independent, needs only parsing crates and sanitiser; exercises the HTML-subset renderer, which also reveals what to fix for EPUB.
3. Audio (symphonia + rodio) and the Media Foundation video spike (section 10): spike first (1-2 days) to confirm the frame-server path on this PC, since it is the highest-uncertainty item; audio can ship before video.
4. 3D (section 4): loaders and CPU rasteriser first; D3D11 only if needed.
5. DOCX and EPUB (sections 2, 3): build the shared rich-text block model once (with RTL), then two readers feed it.
6. PDF export (section 11): Typst CLI as a tool; reuse the SVG pipelines for diagrams and math.
7. Terminal pane and .cast (section 9): the largest build; do it after the simpler previews, as it is more a feature than a preview and `gpui-terminal` plus Zed's design are good guides.

Things to settle with the user before building: (a) whether embedded font data under SIL OFL (KaTeX fonts, optional Typst fonts) is acceptable although OFL is not on the license list; (b) Typst as an on-demand tool vs linked crates; (c) acceptance of "good enough" fidelity for rich HTML mail and Graphviz layouts; (d) the HEVC/AV1/VP9 extension story.
