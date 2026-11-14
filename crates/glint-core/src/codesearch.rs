//! Code patterns for the MCP server's `search_code`, through ast-grep (an optional tool): code found by its
//! syntax tree, in the 20-odd languages it parses, `console.log($$$ARGS)` or `$X.unwrap()`, whatever the
//! spacing and line breaks. (Text mode searched and rewrote code this way until 0.11; the window's one search
//! has no code patterns.)

/// ast-grep's arguments for a search of `folder` answering lines of JSON, leaving out `exclusions` (globs).
pub fn args(pattern: &str, folder: &str, exclusions: &[&str]) -> Vec<String> {
    // `--pattern=…`: a pattern starting with "-" is still the pattern.
    let mut args = vec!["run".to_string(), format!("--pattern={pattern}"), "--json=stream".into()];
    args.extend(exclusions.iter().map(|exclusion| format!("--globs=!{exclusion}")));
    args.extend(["--".into(), folder.into()]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_its_arguments() {
        assert_eq!(
            args("-x", r"C:\proj", &["node_modules", ".git"]),
            ["run", "--pattern=-x", "--json=stream", "--globs=!node_modules", "--globs=!.git", "--", r"C:\proj"]
        );
    }
}
