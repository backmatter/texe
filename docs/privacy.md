# Privacy and network access

texe has no telemetry or accounts. Paper sources, bibliography data, logs, and
PDFs stay on your computer.

Managed builds download tools and packages from TeX Live mirrors and verify
their sizes and cryptographic digests. `texe build --offline` disables these
downloads and requires all dependencies to be cached.

VS Code setup can download VS Code and install LaTeX Workshop. The companion
extension downloads missing TeX dependencies during builds unless
`texe.allowDownloads` is false.

The browser preview and VS Code build connection listen only on your computer.
The preview serves the PDF and bundled viewer assets, not your project sources
or a directory listing. Clicking links in a PDF can open external sites.

The system provider, shell escape, and unmanaged command overrides allow host
software to affect builds. See [configuration](configuration.md) before enabling
them.
