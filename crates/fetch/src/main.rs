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
        #[arg(short, long)]
        output: PathBuf,
        #[arg(short = 's', long, default_value_t = 4, value_parser = parse_splits)]
        splits: usize,
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
            sha256,
            all_proxy,
            http_proxy,
            https_proxy,
            no_proxy,
            user_agent,
            headers,
            timeout,
            connect_timeout,
        } => {
            let sources = input_files.into_iter().try_fold(sources, |mut all, path| {
                all.extend(input_file::read(&path)?);
                Ok::<_, anyhow::Error>(all)
            })?;
            if sources.is_empty() {
                anyhow::bail!("provide at least one source URI or --input-file");
            }
            let network = NetworkOptions {
                all_proxy,
                http_proxy,
                https_proxy,
                no_proxy,
                user_agent,
                headers,
                connect_timeout_seconds: connect_timeout,
                timeout_seconds: timeout,
            };
            let result = Downloader::with_network(network)?
                .download(DownloadRequest {
                    sources,
                    output,
                    splits,
                    sha256,
                })
                .await?;
            println!(
                "completed {} ({} bytes, sha256={})",
                result.path.display(),
                result.bytes,
                result.sha256
            );
        }
        Command::Metalink {
            manifest,
            output,
            splits,
        } => {
            let entry = metalink::read(&manifest).await?;
            let result = Downloader::new()
                .download(DownloadRequest {
                    sources: entry.sources.clone(),
                    output,
                    splits,
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
