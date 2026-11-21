//! Typed, deliberately small compatibility configuration surface.
//!
//! Fetch refuses unknown aria2 options instead of silently ignoring them.  New
//! compatibility options are added here with validation and tests before they
//! are wired into a transport or daemon subsystem.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchConfig {
    pub max_concurrent_downloads: usize,
    pub max_tries: usize,
    pub retry_wait_seconds: u64,
    pub continue_downloads: bool,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 4,
            max_tries: 5,
            retry_wait_seconds: 0,
            continue_downloads: true,
        }
    }
}

impl FetchConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).context("reading Fetch configuration")?)
    }

    pub fn parse(input: &str) -> Result<Self> {
        let mut values = BTreeMap::new();
        for (line_number, raw) in input.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let (key, value) = line.split_once('=').with_context(|| {
                format!("configuration line {} must be key=value", line_number + 1)
            })?;
            if values
                .insert(key.trim().to_owned(), value.trim().to_owned())
                .is_some()
            {
                bail!("duplicate configuration option: {}", key.trim());
            }
        }
        let mut config = Self::default();
        for (key, value) in values {
            match key.as_str() {
                "max-concurrent-downloads" => {
                    config.max_concurrent_downloads = positive(&key, &value)?
                }
                "max-tries" => config.max_tries = positive(&key, &value)?,
                "retry-wait" => {
                    config.retry_wait_seconds = value
                        .parse()
                        .with_context(|| format!("{key} must be a non-negative integer"))?
                }
                "continue" => config.continue_downloads = parse_bool(&key, &value)?,
                _ => bail!("unsupported Fetch configuration option: {key}"),
            }
        }
        Ok(config)
    }
}

fn positive(key: &str, value: &str) -> Result<usize> {
    let parsed: usize = value
        .parse()
        .with_context(|| format!("{key} must be a positive integer"))?;
    if parsed == 0 {
        bail!("{key} must be greater than zero");
    }
    Ok(parsed)
}

fn parse_bool(key: &str, value: &str) -> Result<bool> {
    match value {
        "true" | "1" => Ok(true),
        "false" | "0" => Ok(false),
        _ => bail!("{key} must be true/false or 1/0"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_supported_compatibility_subset() {
        let config = FetchConfig::parse(
            "max-concurrent-downloads=2\nmax-tries=7\nretry-wait=3\ncontinue=false\n",
        )
        .unwrap();
        assert_eq!(config.max_concurrent_downloads, 2);
        assert_eq!(config.max_tries, 7);
        assert_eq!(config.retry_wait_seconds, 3);
        assert!(!config.continue_downloads);
    }

    #[test]
    fn rejects_unknown_or_unsafe_options() {
        assert!(FetchConfig::parse("check-certificate=false").is_err());
        assert!(FetchConfig::parse("max-tries=0").is_err());
    }
}
