# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.2](https://github.com/backmatter/texe/compare/v0.1.1...v0.1.2) - 2026-07-30

### Fixed

- reuse stable auxiliary outputs ([#12](https://github.com/backmatter/texe/pull/12))
- preserve auxiliary pass budget ([#11](https://github.com/backmatter/texe/pull/11))
- restore VS Code paper layouts ([#13](https://github.com/backmatter/texe/pull/13))
- estimate whole build duration ([#10](https://github.com/backmatter/texe/pull/10))
- focus fatal engine errors ([#9](https://github.com/backmatter/texe/pull/9))
- preserve runtime lock requirements ([#8](https://github.com/backmatter/texe/pull/8))

## [0.1.1] - 2026-07-29

### Fixed

- Support the system Bash version when downloading and installing the command
  suite on macOS.

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

[0.1.1]: https://github.com/backmatter/texe/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/backmatter/texe/releases/tag/v0.1.0
