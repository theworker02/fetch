//! Typed, deliberately small compatibility configuration surface.
//!
//! Fetch refuses unknown aria2 options instead of silently ignoring them.  New
//! compatibility options are added here with validation and tests before they
//! are wired into a transport or daemon subsystem.

use std::{collections::BTreeSet, fs, path::Path};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchConfig {
    pub max_concurrent_downloads: usize,
    pub max_tries: usize,
    pub retry_wait_seconds: u64,
    pub continue_downloads: bool,
    pub user_agent: Option<String>,
    pub headers: Vec<String>,
    pub timeout_seconds: Option<u64>,
    pub connect_timeout_seconds: Option<u64>,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            max_concurrent_downloads: 4,
            max_tries: 5,
            retry_wait_seconds: 0,
            continue_downloads: true,
            user_agent: None,
            headers: Vec::new(),
            timeout_seconds: None,
            connect_timeout_seconds: None,
        }
    }
}

impl FetchConfig {
    pub fn from_file(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).context("reading Fetch configuration")?)
    }

    pub fn parse(input: &str) -> Result<Self> {
        let mut config = Self::default();
        let mut seen = BTreeSet::new();
        for (line_number, raw) in input.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let (key, value) = line.split_once('=').with_context(|| {
                format!("configuration line {} must be key=value", line_number + 1)
            })?;
            let key = key.trim();
            let value = value.trim();
            // aria2 permits repeated header entries. All other Fetch options
            // remain singular so a typo cannot quietly change precedence.
            if key != "header" && !seen.insert(key.to_owned()) {
                bail!("duplicate configuration option: {key}");
            }
            match key {
                "max-concurrent-downloads" => {
                    config.max_concurrent_downloads = positive(key, value)?
                }
                "max-tries" => config.max_tries = positive(key, value)?,
                "retry-wait" => {
                    config.retry_wait_seconds = value
                        .parse()
                        .with_context(|| format!("{key} must be a non-negative integer"))?
                }
                "continue" => config.continue_downloads = parse_bool(key, value)?,
                "user-agent" => {
                    if value.is_empty() {
                        bail!("user-agent must not be empty");
                    }
                    config.user_agent = Some(value.to_owned());
                }
                "header" => {
                    if !value.contains(':') {
                        bail!("header must use the form 'Name: value'");
                    }
                    config.headers.push(value.to_owned());
                }
                "timeout" => config.timeout_seconds = Some(nonzero_u64(key, value)?),
                "connect-timeout" => {
                    config.connect_timeout_seconds = Some(nonzero_u64(key, value)?)
                }
                _ => bail!("unsupported Fetch configuration option: {key}"),
            }
        }
        Ok(config)
    }
}

fn nonzero_u64(key: &str, value: &str) -> Result<u64> {
    let parsed: u64 = value
        .parse()
        .with_context(|| format!("{key} must be a positive integer"))?;
    if parsed == 0 {
        bail!("{key} must be greater than zero");
    }
    Ok(parsed)
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
            "max-concurrent-downloads=2\nmax-tries=7\nretry-wait=3\ncontinue=false\nuser-agent=fetch-test\nheader=X-One: 1\nheader=X-Two: 2\ntimeout=10\nconnect-timeout=3\n",
        )
        .unwrap();
        assert_eq!(config.max_concurrent_downloads, 2);
        assert_eq!(config.max_tries, 7);
        assert_eq!(config.retry_wait_seconds, 3);
        assert!(!config.continue_downloads);
        assert_eq!(config.user_agent.as_deref(), Some("fetch-test"));
        assert_eq!(config.headers, ["X-One: 1", "X-Two: 2"]);
        assert_eq!(config.timeout_seconds, Some(10));
        assert_eq!(config.connect_timeout_seconds, Some(3));
    }

    #[test]
    fn rejects_unknown_or_unsafe_options() {
        assert!(FetchConfig::parse("check-certificate=false").is_err());
        assert!(FetchConfig::parse("max-tries=0").is_err());
        assert!(FetchConfig::parse("timeout=0").is_err());
        assert!(FetchConfig::parse("header=invalid").is_err());
    }
}
