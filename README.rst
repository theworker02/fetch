.. image:: assets/fetch-mark.svg
   :width: 96
   :alt: Fetch logo

Fetch
=====

Fetch is a Rust-native, local-first multi-source download engine and a
GPL-2.0-only fork of `aria2 <https://github.com/aria2/aria2>`_.  The fork
retains aria2's history, source tree, and upstream remote while replacing its
implementation incrementally in Rust.

Status
------

The Rust workspace is in ``crates/fetch``.  The current migration foundation
provides:

* concurrent segmented HTTP/HTTPS downloads from multiple mirrors;
* FTP, FTPS, and password-authenticated SFTP downloads, with TLS certificate
  validation for FTPS;
* final-file SHA-256 verification;
* resumable ranged transfers with a local, atomic ``.fetch.resume.json``
  checkpoint beside the staged file;
* Metalink source resolution and complete per-piece SHA-256, SHA-1, and MD5
  verification for a selected file;
* BitTorrent and magnet downloads through ``librqbit``;
* a loopback-only JSON-RPC daemon with WebSocket job events and local SQLite
  job history.

The C++ implementation is still present while the full in-place migration is
underway.  Do not treat Fetch as a drop-in, release-ready replacement for
aria2 yet: persistent transfer resume, complete protocol compatibility,
release packaging, and a comprehensive Rust test suite remain migration work.

Build verification runs on Linux, macOS, and Windows and uploads unsigned
artifacts only.  Fetch does not publish a GitHub release automatically.

Quick start
-----------

Requires a current Rust toolchain.

.. code-block:: powershell

   cargo run -p fetch -- get https://example.com/file.bin --output file.bin --splits 8
   cargo run -p fetch -- daemon

The daemon listens only on ``127.0.0.1`` by default.  Place an authenticated
TLS proxy in front of it if remote access is required.

Repository layout
-----------------

``crates/fetch``
  Rust CLI, download engine, Metalink verifier, BitTorrent adapter, and daemon.

``assets/fetch-mark.svg``
  Original Fetch brand mark.

``doc/`` and historical source directories
  Upstream aria2 documentation and implementation retained during migration.
  Their historical references use the aria2 name until each component is
  replaced and its documentation is migrated.

``MIGRATION.md``
  The subsystem-by-subsystem parity contract and retirement criteria for the
  C++ implementation.

License and attribution
-----------------------

Fetch is derived from aria2 and remains licensed under GPL-2.0-only.  See
``COPYING`` and ``AUTHORS`` for upstream licensing and attribution.

