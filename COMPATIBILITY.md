# Fetch compatibility matrix

This is the authoritative migration status for user-facing aria2-style
configuration. `Supported` means Fetch parses, validates, and wires the value
to its active implementation. `Planned` means it is intentionally rejected or
absent rather than silently ignored.

| aria2-style option / surface | Status | Fetch equivalent or note |
| --- | --- | --- |
| `max-concurrent-downloads` | Supported | `fetch.conf`; bounds daemon jobs |
| `max-tries`, `retry-wait` | Supported | `fetch.conf`; daemon HTTP-family retry policy |
| `continue` | Supported | controls restart requeueing of safely restorable jobs |
| `all-proxy`, `http-proxy`, `https-proxy`, `no-proxy` | Supported | CLI and `fetch.conf` HTTP(S) controls; no ambient proxy inheritance |
| `header`, `user-agent` | Supported | CLI and `fetch.conf` HTTP(S) controls; repeated config `header` values are supported |
| `timeout` | Supported with Fetch semantics | CLI or `fetch.conf` HTTP(S) total request deadline in seconds; aria2 uses a socket timeout |
| `connect-timeout` | Fetch extension | CLI or `fetch.conf` HTTP(S) TCP/TLS connection-establishment deadline in seconds |
| `max-connection-per-server` (`-x`) | Supported | CLI HTTP(S)/FTP(S)/SFTP and Metalink per-authority concurrent range-request cap |
| RPC `token:` authentication | Supported | `FETCH_RPC_SECRET` or `--rpc-secret` |
| `--input-file` job blocks | Partially supported | multi-URI mirror job lines plus indented relative `out=FILE`; other per-download options fail explicitly |
| `load-cookies` | Supported | explicit Netscape-format cookie file loading for CLI HTTP(S) and Metalink transfers |
| `.netrc`, HTTP auth options | Planned | never silently loaded |
| bandwidth, connection-per-server, file allocation | Planned | no compatibility shim |
| certificate/key and RPC TLS options | Planned | loopback daemon; use TLS proxy |
| FTP/SFTP proxy and SSH key options | Planned | URL credentials only today |
| BitTorrent and Metalink option families | Planned | engine adapters are not full option surfaces |
| XML-RPC | Planned | JSON-RPC/WebSocket only today |

The upstream C++ option handlers remain the source baseline until each row has
fixture-backed Rust behavior and is promoted to Supported.
