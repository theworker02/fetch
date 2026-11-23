//! Parser for the safe, implemented subset of aria2 input files.
//!
//! A non-indented line starts one download job and may contain several mirror
//! URIs. Indented `out=FILE` applies to that job. Other per-job options remain
//! explicit errors until Fetch can implement their behavior end-to-end.

use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputJob {
    pub sources: Vec<Url>,
    pub output: Option<PathBuf>,
}

pub fn read(path: &Path) -> Result<Vec<InputJob>> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("reading input URI file {}", path.display()))?;
    parse(&content).with_context(|| format!("parsing input URI file {}", path.display()))
}

pub fn parse(input: &str) -> Result<Vec<InputJob>> {
    let mut jobs: Vec<InputJob> = Vec::new();
    for (index, raw) in input.lines().enumerate() {
        let line_number = index + 1;
        if raw.trim().is_empty() || raw.trim_start().starts_with('#') {
            continue;
        }
        if raw.chars().next().is_some_and(char::is_whitespace) {
            let (key, value) = raw
                .trim()
                .split_once('=')
                .with_context(|| format!("input line {line_number}: option must use key=value"))?;
            let job = jobs.last_mut().with_context(|| {
                format!("input line {line_number}: option appears before a URI job")
            })?;
            match key.trim() {
                "out" => {
                    if job.output.is_some() {
                        bail!("input line {line_number}: duplicate out option");
                    }
                    job.output = Some(safe_output_path(value.trim(), line_number)?);
                }
                option => bail!(
                    "input line {line_number}: per-download option '{option}' is not supported yet"
                ),
            }
            continue;
        }
        let sources = raw
            .split_ascii_whitespace()
            .map(|source| {
                Url::parse(source)
                    .with_context(|| format!("input line {line_number} is not an absolute URI"))
            })
            .collect::<Result<Vec<_>>>()?;
        if sources.is_empty() || sources.iter().any(|source| source.scheme().is_empty()) {
            bail!("input line {line_number} contains no absolute URI");
        }
        jobs.push(InputJob {
            sources,
            output: None,
        });
    }
    if jobs.is_empty() {
        bail!("input URI list contains no download jobs");
    }
    Ok(jobs)
}

fn safe_output_path(value: &str, line_number: usize) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if value.is_empty()
        || path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("input line {line_number}: out must be a relative path without '..'");
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::parse;
    use std::path::PathBuf;

    #[test]
    fn parses_mirrors_and_per_job_output() {
        let jobs = parse("# mirrors\nhttps://one.example/file https://two.example/file\n  out=file.iso\nhttps://three.example/other\n  out=other.bin\n").unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].sources.len(), 2);
        assert_eq!(jobs[0].output, Some(PathBuf::from("file.iso")));
        assert_eq!(jobs[1].output, Some(PathBuf::from("other.bin")));
    }

    #[test]
    fn rejects_unsafe_or_unimplemented_options() {
        assert!(parse("# only a comment\n").is_err());
        assert!(parse("https://one.example/file\n  dir=downloads\n").is_err());
        assert!(parse("https://one.example/file\n  out=../escape\n").is_err());
        assert!(parse("  out=file\n").is_err());
    }
}
