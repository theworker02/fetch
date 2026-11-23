use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use fetch::{
    config::FetchConfig,
    daemon,
    download::{DownloadRequest, Downloader, NetworkOptions},
    input_file, metalink, torrent,
};
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "fetch", version, about = "Rust-native multi-source downloader")]
struct Cli {
    /// Fetch configuration file. Unsupported aria2 options fail explicitly.
    #[arg(long, global = true)]
    conf_path: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download one or more HTTP(S), FTP(S), or SFTP sources into a local file.
    Get {
        /// Source URIs supplied directly on the command line.
        sources: Vec<Url>,
        /// Read additional source URIs from an aria2-style input file. Repeatable.
        #[arg(long = "input-file", short = 'i')]
        input_files: Vec<PathBuf>,
        /// Output for direct sources, or fallback for one input-file job without `out=`.
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(short = 's', long, default_value_t = 4, value_parser = parse_splits)]
        splits: usize,
        /// Maximum concurrent requests to one server (aria2: -x).
        #[arg(short = 'x', long = "max-connection-per-server", default_value_t = 1, value_parser = parse_splits)]
        max_connections_per_server: usize,
        /// Expected SHA-256 digest, in lowercase or uppercase hex.
        #[arg(long)]
        sha256: Option<String>,
        /// Proxy URL for every HTTP(S) request (compatible with aria2's --all-proxy).
        #[arg(long, env = "ALL_PROXY")]
        all_proxy: Option<Url>,
        /// Proxy URL for HTTP requests (compatible with --http-proxy).
        #[arg(long, env = "HTTP_PROXY")]
        http_proxy: Option<Url>,
        /// Proxy URL for HTTPS requests (compatible with --https-proxy).
        #[arg(long, env = "HTTPS_PROXY")]
        https_proxy: Option<Url>,
        /// Comma-separated hosts that must bypass configured HTTP(S) proxies.
        #[arg(long, value_delimiter = ',', env = "NO_PROXY")]
        no_proxy: Vec<String>,
        /// HTTP user-agent value (compatible with --user-agent).
        #[arg(long)]
        user_agent: Option<String>,
        /// Additional HTTP request header in 'Name: value' form; repeatable.
        #[arg(long = "header")]
        headers: Vec<String>,
        /// Load HTTP(S) cookies from a Netscape-format cookie file.
        #[arg(long = "load-cookies")]
        cookie_file: Option<PathBuf>,
        /// Total HTTP(S) request deadline in seconds. This differs from aria2's socket timeout.
        #[arg(long)]
        timeout: Option<u64>,
        /// HTTP(S) connection-establishment timeout in seconds.
        #[arg(long = "connect-timeout")]
        connect_timeout: Option<u64>,
    },
    /// Resolve a Metalink document and download its selected file.
    Metalink {
        manifest: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
        #[arg(short = 's', long, default_value_t = 4)]
        splits: usize,
        /// Maximum concurrent requests to one server (aria2: -x).
        #[arg(short = 'x', long = "max-connection-per-server", default_value_t = 1, value_parser = parse_splits)]
        max_connections_per_server: usize,
        /// Load HTTP(S) cookies from a Netscape-format cookie file.
        #[arg(long = "load-cookies")]
        cookie_file: Option<PathBuf>,
    },
    /// Run the local JSON-RPC daemon. It binds only to the selected local address.
    Daemon {
        #[arg(long, default_value = "127.0.0.1:6800")]
        listen: std::net::SocketAddr,
        /// Require this RPC token. Prefer the FETCH_RPC_SECRET environment variable.
        #[arg(long, env = "FETCH_RPC_SECRET", hide_env_values = true)]
        rpc_secret: Option<String>,
    },
    /// Download a magnet link or .torrent URL through the BitTorrent engine.
    Torrent {
        input: String,
        #[arg(short, long)]
        output_directory: PathBuf,
    },
}

fn parse_splits(input: &str) -> Result<usize, String> {
    let value = input
        .parse::<usize>()
        .map_err(|_| "splits must be a positive integer".to_owned())?;
    if (1..=64).contains(&value) {
        Ok(value)
    } else {
        Err("splits must be between 1 and 64".to_owned())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = match cli.conf_path {
        Some(path) => FetchConfig::from_file(&path)?,
        None => FetchConfig::default(),
    };
    match cli.command {
        Command::Get {
            sources,
            input_files,
            output,
            splits,
            max_connections_per_server,
            sha256,
            all_proxy,
            http_proxy,
            https_proxy,
            no_proxy,
            user_agent,
            headers,
            cookie_file,
            timeout,
            connect_timeout,
        } => {
            let mut jobs = input_files
                .into_iter()
                .try_fold(Vec::new(), |mut all, path| {
                    all.extend(input_file::read(&path)?);
                    Ok::<_, anyhow::Error>(all)
                })?;
            if !sources.is_empty() {
                jobs.insert(
                    0,
                    input_file::InputJob {
                        sources,
                        output: output.clone(),
                    },
                );
            }
            if jobs.is_empty() {
                anyhow::bail!("provide at least one source URI or --input-file");
            }
            if sha256.is_some() && jobs.len() > 1 {
                anyhow::bail!("--sha256 applies to one download; use a single source job");
            }
            if jobs.len() == 1 && jobs[0].output.is_none() {
                jobs[0].output = output;
            }
            if jobs.iter().any(|job| job.output.is_none()) {
                anyhow::bail!(
                    "each input-file job needs an indented out=FILE option when downloading multiple jobs"
                );
            }
            let network = NetworkOptions {
                all_proxy: all_proxy.or(config.all_proxy.clone()),
                http_proxy: http_proxy.or(config.http_proxy.clone()),
                https_proxy: https_proxy.or(config.https_proxy.clone()),
                no_proxy: config.no_proxy.iter().cloned().chain(no_proxy).collect(),
                user_agent: user_agent.or_else(|| config.user_agent.clone()),
                headers: config.headers.iter().cloned().chain(headers).collect(),
                cookie_file,
                connect_timeout_seconds: connect_timeout.or(config.connect_timeout_seconds),
                timeout_seconds: timeout.or(config.timeout_seconds),
            };
            let downloader = Downloader::with_network(network)?;
            for job in jobs {
                let output = job.output.expect("validated above");
                let result = downloader
                    .clone()
                    .download(DownloadRequest {
                        sources: job.sources,
                        output,
                        splits,
                        max_connections_per_server,
                        sha256: sha256.clone(),
                    })
                    .await?;
                println!(
                    "completed {} ({} bytes, sha256={})",
                    result.path.display(),
                    result.bytes,
                    result.sha256
                );
            }
        }
        Command::Metalink {
            manifest,
            output,
            splits,
            max_connections_per_server,
            cookie_file,
        } => {
            let entry = metalink::read(&manifest).await?;
            let result = Downloader::with_network(NetworkOptions {
                all_proxy: config.all_proxy.clone(),
                http_proxy: config.http_proxy.clone(),
                https_proxy: config.https_proxy.clone(),
                no_proxy: config.no_proxy.clone(),
                user_agent: config.user_agent.clone(),
                headers: config.headers.clone(),
                cookie_file,
                connect_timeout_seconds: config.connect_timeout_seconds,
                timeout_seconds: config.timeout_seconds,
                ..Default::default()
            })?
            .download(DownloadRequest {
                sources: entry.sources.clone(),
                output,
                splits,
                max_connections_per_server,
                sha256: entry.sha256.clone(),
            })
            .await?;
            metalink::verify_pieces(&result.path, &entry).await?;
            println!(
                "completed {} ({} bytes, sha256={})",
                result.path.display(),
                result.bytes,
                result.sha256
            );
        }
        Command::Daemon { listen, rpc_secret } => {
            daemon::serve_with_config(listen, config, rpc_secret).await?
        }
        Command::Torrent {
            input,
            output_directory,
        } => torrent::download(input, output_directory).await?,
    }
    Ok(())
}
