use std::path::Path;

use anyhow::{Context, Result, bail};
use quick_xml::de::from_str;
use serde::Deserialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tokio::{fs, io::AsyncReadExt};
use url::Url;

#[derive(Debug, Clone)]
pub struct MetalinkEntry {
    pub sources: Vec<Url>,
    pub sha256: Option<String>,
    pub piece_length: Option<usize>,
    pub piece_hashes: Vec<PieceHash>,
}

#[derive(Debug, Clone)]
pub struct PieceHash {
    pub algorithm: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
struct MetalinkDocument {
    #[serde(rename = "file")]
    files: Vec<MetalinkFile>,
}

#[derive(Debug, Deserialize)]
struct MetalinkFile {
    #[serde(default)]
    resources: Resources,
    #[serde(default)]
    verification: Verification,
}

#[derive(Debug, Default, Deserialize)]
struct Resources {
    #[serde(rename = "url", default)]
    urls: Vec<UrlValue>,
}

#[derive(Debug, Deserialize)]
struct UrlValue {
    #[serde(rename = "$text")]
    value: String,
}

#[derive(Debug, Default, Deserialize)]
struct Verification {
    #[serde(rename = "hash", default)]
    hashes: Vec<HashValue>,
    #[serde(rename = "pieces", default)]
    pieces: Option<Pieces>,
}

#[derive(Debug, Deserialize)]
struct Pieces {
    #[serde(rename = "@length")]
    length: usize,
    #[serde(rename = "hash", default)]
    hashes: Vec<HashValue>,
}

#[derive(Debug, Deserialize)]
struct HashValue {
    #[serde(rename = "@type")]
    algorithm: String,
    #[serde(rename = "$text")]
    value: String,
}

/// Reads the first selected file from a Metalink v4 XML document.
///
/// Multi-file selection intentionally belongs in the CLI/API layer; silently
/// downloading every entry is surprising and unsafe for a command-line tool.
pub async fn read(path: &Path) -> Result<MetalinkEntry> {
    let xml = fs::read_to_string(path)
        .await
        .context("reading Metalink manifest")?;
    let document: MetalinkDocument = from_str(&xml).context("parsing Metalink XML")?;
    let file = document
        .files
        .into_iter()
        .next()
        .context("Metalink contains no file entries")?;
    let sources = file
        .resources
        .urls
        .into_iter()
        .map(|candidate| Url::parse(candidate.value.trim()).context("invalid Metalink URL"))
        .collect::<Result<Vec<_>>>()?;
    if sources.is_empty() {
        bail!("Metalink file has no sources");
    }
    let sha256 = file
        .verification
        .hashes
        .into_iter()
        .find(|hash| {
            hash.algorithm.eq_ignore_ascii_case("sha-256")
                || hash.algorithm.eq_ignore_ascii_case("sha256")
        })
        .map(|hash| hash.value.trim().to_owned());
    let (piece_length, piece_hashes) = match file.verification.pieces {
        Some(pieces) => (
            Some(pieces.length),
            pieces
                .hashes
                .into_iter()
                .map(|hash| PieceHash {
                    algorithm: hash.algorithm,
                    value: hash.value.trim().to_owned(),
                })
                .collect(),
        ),
        None => (None, Vec::new()),
    };
    Ok(MetalinkEntry {
        sources,
        sha256,
        piece_length,
        piece_hashes,
    })
}

/// Validates every Metalink piece after download.  It fails closed for unknown
/// algorithms or a manifest whose hash count does not exactly match the file.
pub async fn verify_pieces(path: &Path, entry: &MetalinkEntry) -> Result<()> {
    let Some(piece_length) = entry.piece_length else {
        return Ok(());
    };
    if piece_length == 0 || entry.piece_hashes.is_empty() {
        bail!("Metalink pieces declaration is incomplete");
    }
    let mut input = fs::File::open(path).await?;
    let mut index = 0;
    loop {
        let mut piece = vec![0_u8; piece_length];
        let read = input.read(&mut piece).await?;
        if read == 0 {
            break;
        }
        let expected = entry
            .piece_hashes
            .get(index)
            .context("Metalink has fewer piece hashes than file pieces")?;
        let actual = digest_piece(&expected.algorithm, &piece[..read])?;
        if !actual.eq_ignore_ascii_case(expected.value.trim()) {
            bail!("Metalink piece {index} {} mismatch", expected.algorithm);
        }
        index += 1;
    }
    if index != entry.piece_hashes.len() {
        bail!("Metalink has more piece hashes than file pieces");
    }
    Ok(())
}

fn digest_piece(algorithm: &str, piece: &[u8]) -> Result<String> {
    let algorithm = algorithm.to_ascii_lowercase();
    let result = match algorithm.as_str() {
        "sha-256" | "sha256" => hex::encode(Sha256::digest(piece)),
        "sha-1" | "sha1" => hex::encode(Sha1::digest(piece)),
        "md5" | "md-5" => hex::encode(md5::Md5::digest(piece)),
        _ => bail!("unsupported Metalink piece hash algorithm: {algorithm}"),
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn schema_accepts_hashes_and_sources() {
        let xml = r#"<metalink><file name="x"><resources><url>https://example.test/x</url></resources><verification><hash type="sha-256">abc</hash></verification></file></metalink>"#;
        let parsed: MetalinkDocument = from_str(xml).unwrap();
        assert_eq!(parsed.files[0].resources.urls.len(), 1);
    }
}
