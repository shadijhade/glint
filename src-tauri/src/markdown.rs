//! Markdown previews: GitHub's flavor (tables, task lists, strikethrough, autolinks, footnotes) as
//! HTML, which the preview builds its view from (MarkdownView.tsx). Raw HTML in the file is passed on,
//! as READMEs use it (`<div align="center">`, `<details>`…): the view keeps only an allowlist of tags
//! and attributes from it, as GitHub does, drops `<script>`, `<style>` and the like with what's in
//! them, and never puts any of it in the page as HTML.

/// The HTML for Markdown `text`, made off the UI thread (a long README takes a while).
#[tauri::command]
pub async fn markdown_html(text: String) -> Result<String, String> {
    crate::fsx::blocking(move || Ok(render(&text))).await
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
    options.render.r#unsafe = true;
    comrak::markdown_to_html(text, &options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_githubs_markdown_and_readme_html() {
        let html = render("# Title\n\n- [x] done\n- [ ] not yet\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n~~old~~ https://example.com\n");
        assert!(html.contains(r#"<h1 id="title">"#), "{html}");
        assert!(html.contains("<table>") && html.contains("<del>old</del>") && html.contains(r#"<a href="https://example.com">"#), "{html}");
        assert!(html.contains(r#"<li class="task-list-item"><input type="checkbox""#), "{html}");

        let readme = render("<div align=\"center\">\n\n<img src=\"logo.svg\" width=\"96\">\n\n# Name\n\n</div>\n");
        assert!(readme.contains(r#"<div align="center">"#) && readme.contains(r#"<img src="logo.svg" width="96">"#), "{readme}");
        assert!(readme.contains(r#"<h1 id="name">"#), "{readme}");
    }
}
