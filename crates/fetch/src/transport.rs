//! Protocol adapters.  FTP and FTPS use libcurl through Rust bindings rather
//! than shelling out to a system command; credentials stay in-memory.

use anyhow::{Context, Result, bail};
use curl::easy::Easy;
use url::Url;

pub async fn content_length(source: Url) -> Result<u64> {
    tokio::task::spawn_blocking(move || {
        let mut handle = configured(&source)?;
        handle.nobody(true)?;
        handle.perform().context("probing FTP(S) source")?;
        let size = handle.content_length_download()?;
        if !size.is_finite() || size < 0.0 {
            bail!("FTP(S) source did not provide a content length");
        }
        Ok(size as u64)
    })
    .await
    .context("FTP(S) probe worker failed")?
}

pub async fn get_range(source: Url, start: u64, end: u64) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let mut handle = configured(&source)?;
        handle.range(&format!("{start}-{end}"))?;
        receive(&mut handle)
    })
    .await
    .context("FTP(S) transfer worker failed")?
}

pub async fn get(source: Url) -> Result<Vec<u8>> {
    tokio::task::spawn_blocking(move || {
        let mut handle = configured(&source)?;
        receive(&mut handle)
    })
    .await
    .context("FTP(S) transfer worker failed")?
}

fn configured(source: &Url) -> Result<Easy> {
    match source.scheme() {
        "ftp" | "ftps" => {}
        other => bail!("unsupported curl transport: {other}"),
    }
    let mut handle = Easy::new();
    handle.url(source.as_str())?;
    handle.follow_location(true)?;
    handle.fail_on_error(true)?;
    // Explicitly retain TLS validation for FTPS.  Never weaken this for a URL.
    if source.scheme() == "ftps" {
        handle.ssl_verify_peer(true)?;
        handle.ssl_verify_host(true)?;
    }
    if !source.username().is_empty() {
        handle.username(source.username())?;
        if let Some(password) = source.password() {
            handle.password(password)?;
        }
    }
    Ok(handle)
}

fn receive(handle: &mut Easy) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    {
        let mut transfer = handle.transfer();
        transfer.write_function(|bytes| {
            body.extend_from_slice(bytes);
            Ok(bytes.len())
        })?;
        transfer.perform()?;
    }
    Ok(body)
}
