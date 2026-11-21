use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use fetch::{
    daemon,
    download::{DownloadRequest, Downloader, NetworkOptions},
    metalink, torrent,
};
use url::Url;

#[derive(Debug, Parser)]
#[command(name = "fetch", version, about = "Rust-native multi-source downloader")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Download one or more HTTP(S), FTP(S), or SFTP sources into a local file.
    Get {
        #[arg(required = true)]
        sources: Vec<Url>,
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
    match Cli::parse().command {
        Command::Get {
            sources,
            output,
            splits,
            sha256,
            all_proxy,
            http_proxy,
            https_proxy,
            no_proxy,
            user_agent,
            headers,
        } => {
            let network = NetworkOptions {
                all_proxy,
                http_proxy,
                https_proxy,
                no_proxy,
                user_agent,
                headers,
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
        Command::Daemon { listen } => daemon::serve(listen).await?,
        Command::Torrent {
            input,
            output_directory,
        } => torrent::download(input, output_directory).await?,
    }
    Ok(())
}
