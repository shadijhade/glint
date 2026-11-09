foliate-js (https://github.com/johnfactotum/foliate-js), MIT, commit 78914aef4466eb960965702401634c2cb348e9b1 (2026-05-01).
Glint changes, in view.js only: zip.js comes from the @zip.js/zip.js package (not a bundled vendor/zip.js), and the PDF branch throws instead of loading PDF.js, which isn't vendored (Glint previews PDFs itself).
