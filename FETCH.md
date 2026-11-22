# Fetch

![Fetch mark](assets/fetch-mark.svg)

Fetch is the Rust migration track for aria2. It is derived from an untouched
clone of [`aria2/aria2`](https://github.com/aria2/aria2), whose GPL-2.0-only
license and history remain present in this repository. `upstream` is configured
to that source repository.

## Current implementation

The Rust workspace is in `crates/fetch`. It currently provides:

- concurrent ranged HTTP/HTTPS downloads from multiple mirrors;
- validated HTTP(S) proxy selection through `--all-proxy`, `--http-proxy`,
  `--https-proxy`, and `--no-proxy`, plus repeatable `--header` and
  `--user-agent` controls;
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

The daemon also accepts the matching initial `aria2.addUri`,
`aria2.addTorrent`, `aria2.tellStatus`, `aria2.remove`, `aria2.forceRemove`,
`aria2.getVersion`, `aria2.tellActive`, `aria2.tellWaiting`, and
`aria2.tellStopped` method names. These are compatibility entry points, not a
claim of full aria2 RPC parity.

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

## Typed configuration

Pass `--conf-path path/to/fetch.conf` to validate the current configuration
subset before starting a command. The supported keys are
`max-concurrent-downloads`, `max-tries`, `retry-wait`, `continue`,
`user-agent`, repeatable `header`, `timeout`, and `connect-timeout`. Unknown
aria2 options fail explicitly rather than being silently ignored. At present,
`max-concurrent-downloads` controls the daemon's bounded active-job queue.
`max-tries` and `retry-wait` now govern retry attempts for daemon HTTP-family
jobs. Removing an active or waiting daemon job aborts its in-process task and
marks it removed.

For HTTP(S)/FTP(S)/SFTP jobs whose persisted, redacted source can be safely
parsed again, daemon startup requeues interrupted work and reuses the staged
range checkpoint. `fetch.pause`/`aria2.pause` and `fetch.unpause`/
`aria2.unpause` are available for those jobs. Credentials embedded only in a
URI are intentionally not persisted, so such jobs are never silently resumed.

## HTTP(S) compatibility options

The `get` command accepts an aria2-familiar subset of network controls:

```powershell
fetch get https://mirror.example/file.iso --output file.iso `
  --all-proxy http://proxy.example:8080 --no-proxy localhost,.internal `
  --header 'Authorization: Bearer example' --user-agent 'fetch/0.1'
```

Proxy routing and custom headers currently apply to HTTP(S) only. FTP/FTPS/SFTP
proxy support, cookie jars, `.netrc` authentication, retry/backoff settings,
and the rest of aria2's option catalogue are intentionally rejected or absent;
they are not silently emulated.

The daemon deliberately refuses non-loopback addresses. Any remote exposure
must use an authenticated TLS proxy and an explicit security design.

Set `FETCH_RPC_SECRET` (or pass `--rpc-secret`) to require control-plane
authentication. Fetch accepts `Authorization: Bearer <secret>` and the
aria2-compatible first RPC parameter `token:<secret>`. The secret is runtime
only and is never written to SQLite.

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

The HTTP(S) adapter is the most complete current transport. Adding a transport requires real
integration tests against a local protocol fixture, not a placeholder command.
