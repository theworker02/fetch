# Fetch migration contract

Fetch is an in-place Rust rewrite of aria2, not a wrapper and not a parallel
product.  This document makes the remaining work measurable against the
upstream implementation before a C++ subsystem is removed.

## Upstream baseline

The imported aria2 tree currently contains 938 C++ headers and implementation
files, 214 C++ test files, and 211 registered option preferences.  A Rust
replacement must preserve observable behavior, documented security boundaries,
and compatible import/export formats before the matching C++ area is retired.

## Migration tracks

| Track | Upstream responsibility | Rust destination | Completion evidence |
| --- | --- | --- | --- |
| CLI and configuration | option handlers, config files, URI input files | `crates/fetch/src/config` | typed, fail-closed initial config subset; every supported option parsed, validated, documented, and fixture-tested |
| Scheduler | request groups, piece selection, retries, bandwidth, resume | `crates/fetch/src/engine` | deterministic queue/retry tests and resumable transfer fixtures |
| HTTP(S) | HTTP/1.1, authentication, proxies, cookies, TLS | `crates/fetch/src/http` | local HTTP/proxy/TLS integration suite |
| FTP/FTPS/SFTP | authentication, active/passive modes, TLS, proxying | `crates/fetch/src/ftp` and `src/sftp` | local protocol-server integration suite |
| BitTorrent | bencode, peers, DHT, trackers, WebSeed, piece state | `crates/fetch/src/torrent` | magnet and `.torrent` interoperability fixtures |
| Metalink | v3/v4 parsing, mirrors, signatures, chunk hashes | `crates/fetch/src/metalink` | RFC-derived fixtures and checksum failure tests |
| RPC and daemon | JSON-RPC, WebSocket, sessions, lifecycle, auth | `crates/fetch/src/daemon` | initial `aria2.*` aliases and bounded queue; compatibility requests, restart, and authorization tests |
| Persistence | session files, checkpoints, cookies, DHT state | `crates/fetch/src/store` | restart and corruption-recovery tests |
| Packaging | platform releases, completion, man pages | `packaging/` | reproducible platform build verification |

## Retirement rule

No C++ file or historical build path may be deleted merely to alter language
classification.  It is removed only after its Rust replacement has passed the
matching compatibility suite and is listed as complete here.

## Current completed foundation

The initial Rust implementation has real HTTP(S), FTP/FTPS/SFTP, BitTorrent,
Metalink checksum, local JSON-RPC, WebSocket event, and SQLite state paths.
These provide a foundation, not compatibility parity.  In particular, option
coverage, HTTP proxy/cookie compatibility, FTP-family proxy support, `.netrc`,
resumable daemon sessions, and the full aria2 RPC method set are still pending.
