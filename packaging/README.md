# Fetch packaging

`fetch-release.yml` creates unsigned archives and SHA-256 checksum files for
Windows, macOS, and Linux when a `fetch-v*` tag is pushed or the workflow is
started manually. It uploads artifacts for review only; it does not create a
GitHub Release or claim code signing.

Before public distribution, add platform signing identities, provenance, and a
reviewed publishing step. Historical aria2 packaging remains in its original
directories until equivalent Fetch packaging replaces it.
