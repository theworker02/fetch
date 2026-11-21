# Fetch

![Fetch mark](assets/fetch-mark.svg)

Fetch is the Rust migration track for aria2. It is derived from an untouched
clone of [`aria2/aria2`](https://github.com/aria2/aria2), whose GPL-2.0-only
license and history remain present in this repository. `upstream` is configured
to that source repository.

## Current implementation

The Rust workspace is in `crates/fetch`. It currently provides:

- concurrent ranged HTTP/HTTPS downloads from multiple mirrors;
- persisted completed-range checkpoints for compatible resumed transfers;
- FTP, FTPS, and password-authenticated SFTP downloads through libcurl, with
  certificate validation retained for FTPS;
- atomic `.part` staging and full-file SHA-256 validation;
- Metalink v4 source extraction plus full-file and per-piece SHA-256, SHA-1,
  and MD5 verification for one explicit file entry;
- BitTorrent and magnet downloads through the Rust-native `librqbit` engine;
- a loopback-only JSON-RPC daemon with `fetch.getVersion`, `fetch.addUri`,
  `fetch.addTorrent`, `fetch.tellStatus`, and `fetch.remove`, durable SQLite
  job state, and WebSocket events at `/events`.

The daemon marks active transfers as interrupted after a restart rather than
claiming that a transfer resumed. Transport-specific resume metadata and service
manager integration remain to be implemented.

The repository's ``Fetch Rust`` workflow builds and tests the Rust binary on
Linux, macOS, and Windows, retaining unsigned artifacts for review. It does
not publish releases automatically.

## Run

```powershell
cargo run -p fetch -- get https://example.com/file.bin --output file.bin --splits 8
cargo run -p fetch -- get sftp://user@host.example/path/file.bin --output file.bin
cargo run -p fetch -- torrent 'magnet:?xt=urn:btih:...' --output-directory .\downloads
cargo run -p fetch -- daemon
```

The daemon deliberately refuses non-loopback addresses. Any remote exposure
must use an authenticated TLS proxy and an explicit security design.

## Migration architecture

```text
CLI / JSON-RPC
      |
Scheduler ── Transfer adapters (HTTP(S), FTP(S), BitTorrent)
      |              |
Verification ─── local staging / resume metadata
      |
Persistent daemon state
```

The committed initial adapter is HTTP(S). Adding a transport requires real
integration tests against a local protocol fixture, not a placeholder command.
