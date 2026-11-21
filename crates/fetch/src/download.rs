use std::{path::PathBuf, sync::Arc};

use crate::transport;
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    sync::Mutex,
};
use url::Url;

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub sources: Vec<Url>,
    pub output: PathBuf,
    pub splits: usize,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
}

impl Downloader {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .user_agent("fetch/0.1")
                .build()
                .expect("valid client"),
        }
    }

    pub async fn download(&self, request: DownloadRequest) -> Result<DownloadResult> {
        if request.sources.is_empty() {
            bail!("at least one source is required");
        }
        validate_sources(&request.sources)?;
        let metadata = self.probe(&request.sources[0]).await?;
        let parent = request
            .output
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)
            .await
            .context("creating destination directory")?;
        let staging = request.output.with_extension("fetch.part");
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&staging)
            .await?;
        file.set_len(metadata.length).await?;
        let file = Arc::new(Mutex::new(file));

        if metadata.ranges && request.splits > 1 && metadata.length > 0 {
            let parts = request.splits.min(metadata.length as usize);
            let chunk = metadata.length.div_ceil(parts as u64);
            let mut work = tokio::task::JoinSet::new();
            for index in 0..parts {
                let start = index as u64 * chunk;
                let end = (start + chunk).min(metadata.length) - 1;
                let source = request.sources[index % request.sources.len()].clone();
                let client = self.client.clone();
                let file = file.clone();
                work.spawn(async move { copy_range(client, source, start, end, file).await });
            }
            while let Some(task) = work.join_next().await {
                task.context("download worker panicked")??;
            }
        } else {
            // A server that refuses byte ranges remains supported; no fake segmentation.
            copy_stream(
                self.client.clone(),
                request.sources[0].clone(),
                file.clone(),
            )
            .await?;
        }
        drop(file);
        let (bytes, digest) = digest_file(&staging).await?;
        if bytes != metadata.length {
            bail!(
                "download size mismatch: expected {}, received {bytes}",
                metadata.length
            );
        }
        if let Some(expected) = request.sha256.as_deref() {
            if !digest.eq_ignore_ascii_case(expected.trim()) {
                bail!("SHA-256 mismatch for {}", request.output.display());
            }
        }
        fs::rename(&staging, &request.output)
            .await
            .context("finalizing completed download")?;
        Ok(DownloadResult {
            path: request.output,
            bytes,
            sha256: digest,
        })
    }

    async fn probe(&self, source: &Url) -> Result<RemoteMetadata> {
        if matches!(source.scheme(), "ftp" | "ftps") {
            return Ok(RemoteMetadata {
                length: transport::content_length(source.clone()).await?,
                // libcurl can perform FTP REST offsets; the transfer itself
                // checks the returned byte count before touching the output.
                ranges: true,
            });
        }
        let response = self
            .client
            .head(source.clone())
            .send()
            .await
            .context("probing source")?
            .error_for_status()?;
        let length = response.content_length().context("source did not provide Content-Length; streaming unknown-size files is not enabled yet")?;
        let ranges = response
            .headers()
            .get(reqwest::header::ACCEPT_RANGES)
            .is_some_and(|h| h.as_bytes().eq_ignore_ascii_case(b"bytes"));
        Ok(RemoteMetadata { length, ranges })
    }
}

struct RemoteMetadata {
    length: u64,
    ranges: bool,
}

fn validate_sources(sources: &[Url]) -> Result<()> {
    for source in sources {
        match source.scheme() {
            "http" | "https" | "ftp" | "ftps" => {}
            other => bail!("unsupported transport: {other}"),
        }
        if source.host_str().is_none() {
            bail!("source has no host: {source}");
        }
    }
    Ok(())
}

async fn copy_range(
    client: reqwest::Client,
    source: Url,
    start: u64,
    end: u64,
    file: Arc<Mutex<File>>,
) -> Result<()> {
    let is_ftp = matches!(source.scheme(), "ftp" | "ftps");
    if is_ftp {
        let body = transport::get_range(source, start, end).await?;
        let expected = end - start + 1;
        if body.len() as u64 != expected {
            bail!(
                "FTP(S) range returned {} bytes; expected {expected}",
                body.len()
            );
        }
        let mut output = file.lock().await;
        output.seek(SeekFrom::Start(start)).await?;
        output.write_all(&body).await?;
        return Ok(());
    }
    let response = client
        .get(source)
        .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
        .send()
        .await?
        .error_for_status()?;
    if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        bail!("source ignored byte range request");
    }
    let body = response.bytes().await?;
    let expected = end - start + 1;
    if body.len() as u64 != expected {
        bail!("range returned {} bytes; expected {expected}", body.len());
    }
    let mut output = file.lock().await;
    output.seek(SeekFrom::Start(start)).await?;
    output.write_all(&body).await?;
    Ok(())
}

async fn copy_stream(client: reqwest::Client, source: Url, file: Arc<Mutex<File>>) -> Result<()> {
    if matches!(source.scheme(), "ftp" | "ftps") {
        let body = transport::get(source).await?;
        let mut output = file.lock().await;
        output.seek(SeekFrom::Start(0)).await?;
        output.write_all(&body).await?;
        return Ok(());
    }
    let response = client.get(source).send().await?.error_for_status()?;
    let mut stream = response.bytes_stream();
    let mut output = file.lock().await;
    output.seek(SeekFrom::Start(0)).await?;
    while let Some(chunk) = stream.next().await {
        output.write_all(&chunk?).await?;
    }
    Ok(())
}

async fn digest_file(path: &std::path::Path) -> Result<(u64, String)> {
    let mut input = File::open(path).await?;
    let mut buffer = [0_u8; 64 * 1024];
    let mut hasher = Sha256::new();
    let mut size = 0;
    loop {
        let read = input.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_transports() {
        let source = Url::parse("ftp://example.test/file").unwrap();
        assert!(validate_sources(&[source]).is_ok());
        let source = Url::parse("file:///tmp/example").unwrap();
        assert!(validate_sources(&[source]).is_err());
    }
}
