# Privacy and network access

texe has no telemetry or accounts. Paper sources, bibliography data, logs, and
PDFs stay on your computer.

Managed builds download tools and packages from TeX Live mirrors and verify
their sizes and cryptographic digests. `texe build --offline` disables these
downloads and requires all dependencies to be cached.

VS Code setup can download VS Code and installs Citet from the Marketplace.
VS Code installs and updates its standalone tex-ls dependency.
Language processing, builds, PDF viewing and SyncTeX run locally without an
account or server. Citet can download checksum-verified texe releases and
missing TeX dependencies unless `citet.allowDownloads` is false. Sources are
synchronized to a Citet server only after connecting a project; remote builds are
selected explicitly with `citet.buildBackend`.

The terminal browser preview listens only on your computer. It serves the PDF
and bundled viewer assets, not project sources or a directory listing. The VS
Code viewer uses a webview directly and has no local build-bridge server.

The system provider, shell escape, and unmanaged command overrides allow host
software to affect builds. See [configuration](configuration.md) before enabling
them.
