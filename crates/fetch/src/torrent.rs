//! BitTorrent adapter backed by `librqbit`.
//!
//! The engine owns peer connections, trackers, DHT and piece hashes; Fetch
//! owns local job lifecycle and does not pretend a URL parser is a torrent
//! implementation.

use std::path::PathBuf;

use anyhow::{Context, Result};
use librqbit::{AddTorrent, Session};

pub async fn download(input: String, output_directory: PathBuf) -> Result<()> {
    let session = Session::new(output_directory)
        .await
        .context("starting BitTorrent session")?;
    let handle = session
        .add_torrent(AddTorrent::from_url(&input), None)
        .await
        .context("adding torrent or magnet URI")?
        .into_handle()
        .context("torrent was not accepted for download")?;
    handle
        .wait_until_completed()
        .await
        .context("BitTorrent transfer failed")?;
    Ok(())
}
