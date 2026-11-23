use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::transport;
use anyhow::{Context, Result, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, File, OpenOptions},
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, SeekFrom},
    sync::Mutex,
};
use url::Url;

/// Network controls shared by all HTTP(S) transfers in a download.
///
/// Proxy credentials belong in the proxy URI and are intentionally never
/// persisted by the daemon's job store.
#[derive(Debug, Clone, Default)]
pub struct NetworkOptions {
    pub all_proxy: Option<Url>,
    pub http_proxy: Option<Url>,
    pub https_proxy: Option<Url>,
    pub no_proxy: Vec<String>,
    pub user_agent: Option<String>,
    pub headers: Vec<String>,
    /// Time allowed to establish a TCP/TLS connection before the request fails.
    pub connect_timeout_seconds: Option<u64>,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub sources: Vec<Url>,
    pub output: PathBuf,
    pub splits: usize,
    /// Maximum simultaneous range requests sent to one scheme/host/port.
    pub max_connections_per_server: usize,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadResult {
    pub path: PathBuf,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    end: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ResumeState {
    length: u64,
    sources: Vec<String>,
    completed: Vec<ByteRange>,
}

#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
}

impl Downloader {
    pub fn new() -> Self {
        Self::with_network(NetworkOptions::default()).expect("default network options are valid")
    }

    pub fn with_network(network: NetworkOptions) -> Result<Self> {
        // Do not inherit ambient proxy variables: proxy behavior must be
        // explicit and therefore visible in Fetch configuration/CLI output.
        let mut builder = reqwest::Client::builder().no_proxy();
        if let Some(timeout) = network.connect_timeout_seconds {
            if timeout == 0 {
                bail!("connect timeout must be greater than zero");
            }
            builder = builder.connect_timeout(std::time::Duration::from_secs(timeout));
        }
        if let Some(timeout) = network.timeout_seconds {
            if timeout == 0 {
                bail!("timeout must be greater than zero");
            }
            builder = builder.timeout(std::time::Duration::from_secs(timeout));
        }
        if let Some(user_agent) = network.user_agent {
            builder = builder.user_agent(user_agent);
        } else {
            builder = builder.user_agent("fetch/0.1");
        }
        let mut headers = reqwest::header::HeaderMap::new();
        for header in network.headers {
            let (name, value) = header
                .split_once(':')
                .context("request header must use the form 'Name: value'")?;
            let name = reqwest::header::HeaderName::from_bytes(name.trim().as_bytes())
                .context("invalid request header name")?;
            let value = reqwest::header::HeaderValue::from_str(value.trim())
                .context("invalid request header value")?;
            headers.insert(name, value);
        }
        if !headers.is_empty() {
            builder = builder.default_headers(headers);
        }
        if network.all_proxy.is_some()
            || network.http_proxy.is_some()
            || network.https_proxy.is_some()
        {
            builder = builder.proxy(reqwest::Proxy::custom(move |url| {
                if bypasses_proxy(url, &network.no_proxy) {
                    return None;
                }
                match url.scheme() {
                    "http" => network
                        .http_proxy
                        .as_ref()
                        .or(network.all_proxy.as_ref())
                        .map(|proxy| proxy.as_str().to_owned()),
                    "https" => network
                        .https_proxy
                        .as_ref()
                        .or(network.all_proxy.as_ref())
                        .map(|proxy| proxy.as_str().to_owned()),
                    _ => None,
                }
            }));
        }
        Ok(Self {
            client: builder.build().context("building HTTP client")?,
        })
    }

    pub async fn download(&self, request: DownloadRequest) -> Result<DownloadResult> {
        if request.sources.is_empty() {
            bail!("at least one source is required");
        }
        validate_request(&request)?;
        let metadata = self.probe(&request.sources[0]).await?;
        let parent = request
            .output
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)
            .await
            .context("creating destination directory")?;
        let staging = request.output.with_extension("fetch.part");
        let resume_path = request.output.with_extension("fetch.resume.json");
        let sources = request
            .sources
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let resume = load_resume(&resume_path, metadata.length, &sources).await?;
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!staging.exists())
            .open(&staging)
            .await?;
        file.set_len(metadata.length).await?;
        let file = Arc::new(Mutex::new(file));

        if metadata.ranges && request.splits > 1 && metadata.length > 0 {
            let state = Arc::new(Mutex::new(resume.unwrap_or(ResumeState {
                length: metadata.length,
                sources,
                completed: Vec::new(),
            })));
            let parts = request.splits.min(metadata.length as usize);
            let chunk = metadata.length.div_ceil(parts as u64);
            let connection_limits = request
                .sources
                .iter()
                .map(|source| {
                    (
                        server_key(source),
                        Arc::new(tokio::sync::Semaphore::new(
                            request.max_connections_per_server,
                        )),
                    )
                })
                .collect::<HashMap<_, _>>();
            let mut work = tokio::task::JoinSet::new();
            for index in 0..parts {
                let start = index as u64 * chunk;
                let end = (start + chunk).min(metadata.length) - 1;
                let range = ByteRange { start, end };
                if state.lock().await.completed.contains(&range) {
                    continue;
                }
                let source = request.sources[index % request.sources.len()].clone();
                let connection_limit = connection_limits
                    .get(&server_key(&source))
                    .expect("every request source has a connection limiter")
                    .clone();
                let client = self.client.clone();
                let file = file.clone();
                let state = state.clone();
                let resume_path = resume_path.clone();
                work.spawn(async move {
                    copy_range(client, source, start, end, file, connection_limit).await?;
                    record_completed(state, resume_path, range).await
                });
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
        if resume_path.exists() {
            fs::remove_file(&resume_path).await?;
        }
        Ok(DownloadResult {
            path: request.output,
            bytes,
            sha256: digest,
        })
    }

    async fn probe(&self, source: &Url) -> Result<RemoteMetadata> {
        if matches!(source.scheme(), "ftp" | "ftps" | "sftp") {
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

fn bypasses_proxy(url: &Url, rules: &[String]) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    rules.iter().any(|rule| {
        let rule = rule.trim().trim_start_matches('.');
        rule == "*" || host.eq_ignore_ascii_case(rule) || host.ends_with(&format!(".{rule}"))
    })
}

async fn load_resume(path: &Path, length: u64, sources: &[String]) -> Result<Option<ResumeState>> {
    let Ok(encoded) = fs::read(path).await else {
        return Ok(None);
    };
    let state: ResumeState = serde_json::from_slice(&encoded).context("parsing resume metadata")?;
    if state.length != length || state.sources != sources {
        return Ok(None);
    }
    Ok(Some(state))
}

async fn record_completed(
    state: Arc<Mutex<ResumeState>>,
    path: PathBuf,
    range: ByteRange,
) -> Result<()> {
    let encoded = {
        let mut state = state.lock().await;
        if !state.completed.contains(&range) {
            state.completed.push(range);
        }
        serde_json::to_vec_pretty(&*state)?
    };
    let temporary = path.with_extension("fetch.resume.tmp");
    fs::write(&temporary, encoded).await?;
    fs::rename(temporary, path).await?;
    Ok(())
}

struct RemoteMetadata {
    length: u64,
    ranges: bool,
}

fn validate_request(request: &DownloadRequest) -> Result<()> {
    if request.max_connections_per_server == 0 {
        bail!("max connections per server must be greater than zero");
    }
    for source in &request.sources {
        match source.scheme() {
            "http" | "https" | "ftp" | "ftps" | "sftp" => {}
            other => bail!("unsupported transport: {other}"),
        }
        if source.host_str().is_none() {
            bail!("source has no host: {source}");
        }
    }
    Ok(())
}

fn server_key(source: &Url) -> String {
    format!(
        "{}://{}:{}",
        source.scheme(),
        source.host_str().unwrap_or_default(),
        source.port_or_known_default().unwrap_or_default()
    )
}

async fn copy_range(
    client: reqwest::Client,
    source: Url,
    start: u64,
    end: u64,
    file: Arc<Mutex<File>>,
    connection_limit: Arc<tokio::sync::Semaphore>,
) -> Result<()> {
    let _permit = connection_limit
        .acquire_owned()
        .await
        .context("connection limiter shut down")?;
    let is_ftp = matches!(source.scheme(), "ftp" | "ftps" | "sftp");
    if is_ftp {
        let body = transport::get_range(source, start, end).await?;
        let expected = end - start + 1;
        if body.len() as u64 != expected {
            bail!(
                "FTP(S)/SFTP range returned {} bytes; expected {expected}",
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
    if matches!(source.scheme(), "ftp" | "ftps" | "sftp") {
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
        assert!(
            validate_request(&DownloadRequest {
                sources: vec![source],
                output: PathBuf::from("file"),
                splits: 1,
                max_connections_per_server: 1,
                sha256: None,
            })
            .is_ok()
        );
        let source = Url::parse("sftp://example.test/file").unwrap();
        assert!(
            validate_request(&DownloadRequest {
                sources: vec![source],
                output: PathBuf::from("file"),
                splits: 1,
                max_connections_per_server: 1,
                sha256: None,
            })
            .is_ok()
        );
        let source = Url::parse("file:///tmp/example").unwrap();
        assert!(
            validate_request(&DownloadRequest {
                sources: vec![source],
                output: PathBuf::from("file"),
                splits: 1,
                max_connections_per_server: 1,
                sha256: None,
            })
            .is_err()
        );
    }

    #[test]
    fn validates_connection_limit_and_authority_key() {
        let source = Url::parse("https://example.test:8443/file").unwrap();
        assert_eq!(server_key(&source), "https://example.test:8443");
        assert!(
            validate_request(&DownloadRequest {
                sources: vec![source],
                output: PathBuf::from("file"),
                splits: 1,
                max_connections_per_server: 0,
                sha256: None,
            })
            .is_err()
        );
    }

    #[tokio::test]
    async fn persists_and_loads_compatible_resume_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("file.fetch.resume.json");
        let state = Arc::new(Mutex::new(ResumeState {
            length: 16,
            sources: vec!["https://example.test/file".into()],
            completed: Vec::new(),
        }));
        record_completed(state, path.clone(), ByteRange { start: 0, end: 7 })
            .await
            .unwrap();
        let restored = load_resume(&path, 16, &["https://example.test/file".into()])
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.completed, vec![ByteRange { start: 0, end: 7 }]);
    }

    #[test]
    fn matches_no_proxy_hosts_without_overmatching() {
        let url = Url::parse("https://packages.example.test/archive").unwrap();
        assert!(bypasses_proxy(&url, &["example.test".into()]));
        assert!(!bypasses_proxy(&url, &["ample.test".into()]));
        assert!(bypasses_proxy(&url, &["*".into()]));
    }

    #[test]
    fn rejects_a_zero_request_timeout() {
        assert!(
            Downloader::with_network(NetworkOptions {
                timeout_seconds: Some(0),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn rejects_a_zero_connect_timeout() {
        assert!(
            Downloader::with_network(NetworkOptions {
                connect_timeout_seconds: Some(0),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[tokio::test]
    #[ignore = "requires loopback networking, which the desktop sandbox forbids"]
    async fn downloads_segmented_bytes_from_a_real_http_server() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 1024];
                let length = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..length]);
                let body = b"abcdef";
                if request.starts_with("HEAD ") {
                    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n").await.unwrap();
                    continue;
                }
                let range = request
                    .lines()
                    .find_map(|line| {
                        line.strip_prefix("range: bytes=")
                            .or_else(|| line.strip_prefix("Range: bytes="))
                    })
                    .unwrap();
                let (start, end) = range.split_once('-').unwrap();
                let start: usize = start.parse().unwrap();
                let end: usize = end.parse().unwrap();
                let response = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/6\r\nConnection: close\r\n\r\n",
                    end - start + 1,
                    start,
                    end
                );
                socket.write_all(response.as_bytes()).await.unwrap();
                socket.write_all(&body[start..=end]).await.unwrap();
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("download.bin");
        let result = Downloader::new()
            .download(DownloadRequest {
                sources: vec![Url::parse(&format!("http://{address}/file")).unwrap()],
                output: output.clone(),
                splits: 2,
                max_connections_per_server: 2,
                sha256: None,
            })
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(result.bytes, 6);
        assert_eq!(fs::read(output).await.unwrap(), b"abcdef");
    }

    #[tokio::test]
    #[ignore = "requires loopback networking, which the desktop sandbox forbids"]
    async fn routes_http_transfers_through_an_explicit_proxy() {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::TcpListener,
        };

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 1024];
                let length = socket.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..length]);
                if request.starts_with("HEAD ") {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\n",
                        )
                        .await
                        .unwrap();
                } else {
                    socket
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nvia",
                        )
                        .await
                        .unwrap();
                }
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("proxied.bin");
        let downloader = Downloader::with_network(NetworkOptions {
            all_proxy: Some(proxy),
            ..Default::default()
        })
        .unwrap();
        downloader
            .download(DownloadRequest {
                sources: vec![Url::parse("http://origin.invalid/file").unwrap()],
                output: output.clone(),
                splits: 1,
                max_connections_per_server: 1,
                sha256: None,
            })
            .await
            .unwrap();
        server.await.unwrap();
        assert_eq!(fs::read(output).await.unwrap(), b"via");
    }
}
