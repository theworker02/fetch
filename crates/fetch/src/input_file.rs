//! Strict parser for aria2-style input URI lists.
//!
//! Fetch currently accepts one absolute URI per non-comment line.  The richer
//! indented per-download option grammar is deliberately rejected until each
//! option has a typed Fetch equivalent; accepting and discarding it would make
//! migrations unsafe.

use std::{fs, path::Path};

use anyhow::{Context, Result, bail};
use url::Url;

pub fn read(path: &Path) -> Result<Vec<Url>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading input URI file {}", path.display()))?;
    parse(&content).with_context(|| format!("parsing input URI file {}", path.display()))
}

pub fn parse(input: &str) -> Result<Vec<Url>> {
    let mut sources = Vec::new();
    for (index, raw) in input.lines().enumerate() {
        let line_number = index + 1;
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        if raw.chars().next().is_some_and(char::is_whitespace) {
            bail!("input line {line_number}: per-download option lines are not supported yet");
        }
        let source = Url::parse(raw.trim())
            .with_context(|| format!("input line {line_number} is not an absolute URI"))?;
        if source.scheme().is_empty() {
            bail!("input line {line_number} is not an absolute URI");
        }
        sources.push(source);
    }
    if sources.is_empty() {
        bail!("input URI list contains no download sources");
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn parses_uri_list_with_comments_and_blank_lines() {
        let values =
            parse("# mirrors\n\nhttps://one.example/file\nftp://two.example/file\n").unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0].host_str(), Some("one.example"));
    }

    #[test]
    fn rejects_empty_lists_and_unimplemented_option_lines() {
        assert!(parse("# only a comment\n").is_err());
        assert!(parse("https://one.example/file\n  out=file\n").is_err());
    }
}
