//! Markdown previews: GitHub's flavor (tables, task lists, strikethrough, autolinks, footnotes) as
//! HTML, which the preview builds its view from (MarkdownView.tsx, through an allowlist of tags).
//! Raw HTML in the file is shown as text, never run, and links that could run something
//! (javascript:, vbscript:, file:) are emptied.

/// The HTML for Markdown `text`.
#[tauri::command]
pub fn markdown_html(text: String) -> String {
    render(&text)
}

pub fn render(text: &str) -> String {
    let mut options = comrak::Options::default();
    options.extension.strikethrough = true;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.footnotes = true;
    // Headings get ids, as on GitHub, so links within the file go somewhere.
    options.extension.header_id_prefix = Some(String::new());
    options.render.tasklist_classes = true;
    // `unsafe` stays off: raw HTML isn't passed through, and dangerous links are emptied.
    options.render.escape = true;
    comrak::markdown_to_html(text, &options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_githubs_markdown_and_nothing_that_runs() {
        let html = render("# Title\n\n- [x] done\n- [ ] not yet\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n~~old~~ https://example.com\n");
        assert!(html.contains(r#"<h1 id="title">"#), "{html}");
        assert!(html.contains("<table>") && html.contains("<del>old</del>") && html.contains(r#"<a href="https://example.com">"#), "{html}");
        assert!(html.contains(r#"<li class="task-list-item"><input type="checkbox""#), "{html}");

        let hostile = render("<script>alert(1)</script>\n\n<img src=x onerror=alert(1)>\n\n[click](javascript:alert(1)) ![p](relative/pic.png)\n");
        assert!(!hostile.contains("<script>") && hostile.contains("&lt;script&gt;"), "{hostile}");
        assert!(!hostile.contains("<img src=x"), "{hostile}");
        assert!(!hostile.contains("javascript:"), "{hostile}");
        assert!(hostile.contains(r#"<img src="relative/pic.png""#), "{hostile}");
    }
}
