# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-07-28

### Added

- Create new papers through guided or non-interactive setup, with basic and
  empty starter documents.
- Build managed pdfLaTeX and LuaLaTeX projects on Linux x86-64, Windows
  x86-64, and macOS Apple Silicon.
- Resolve exact TeX Live packages through the pinned pqty 0.1.0 command suite.
- Run BibTeX, Biber, MakeIndex, glossaries, convergence passes, and SyncTeX
  publication when the document requires them.
- Reproduce committed `texe.lock` environments from empty caches with
  `--frozen`, and forbid network use with `--offline`.
- Watch source files, serve the loopback-only PDF.js viewer, and configure an
  optional project-local VS Code workflow.
- Inspect and clean derived project and shared storage without deleting paper
  sources, locks, or published PDFs.
- Provide closed v1 JSON Schemas for project, lock, command-result, error, and
  watch-event protocols.
- Publish compatible `texe`, `pqty`, and `pqty-fls` binaries as portable
  archives, a Debian package, and generated Homebrew and WinGet metadata.

[0.1.0]: https://github.com/backmatter/texe/releases/tag/v0.1.0
