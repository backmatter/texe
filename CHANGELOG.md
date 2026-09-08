# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Adopt existing papers with a read-only compatibility preflight and a guided
  VS Code setup path.
- Merge and preview JSONC editor settings, track ownership, and undo texe's
  changes while preserving later user edits.
- VS Code build-on-save, tasks, executable selection, setup checks, clickable
  diagnostics, error toasts, retained-PDF badges, and optional F5/F6 shortcuts.
- Real VS Code and LaTeX Workshop tests for rendered PDF refresh, source errors,
  recovery, SyncTeX and manifest changes.

- `cargo xtask verify contracts` checks real command-suite binaries against
  isolated local fixtures without downloading TeX or packages.

- `texe watch --debounce-ms` configures the quiet period before rebuilding
  (default: 250 ms). Continuous changes postpone the build until inputs settle.

### Fixed

- Publish absolute SyncTeX input paths with corrected byte anchors so Windows
  source/PDF navigation works independently of the editor's working directory.
- Recognize actual package declarations during adoption, follow literal local
  includes, choose LuaLaTeX for Unicode packages without an engine hint, and
  provide readable compatibility checks and direct editor-conflict review.
- Open the editor after a failed first build and hand its diagnostic to Problems.
- Route LaTeX Workshop builds through the companion queue, replace stale errors,
  link warnings to build logs, and rebuild externally changed figures/data.
- Preserve saves made during terminal builds and display browser build/failure
  and disconnected states.
- Add an optional reversible writing layout, first-use guide, representative
  adoption paper, and managed editor verification on all three platforms.

- Allow editor builds to download missing TeX tools and packages by default;
  offline texe builds remain available through `texe.allowDownloads = false`.
- Distinguish cancelled editor builds from failures, suppress stale queued-build
  success/error notices, and show live phase/elapsed status with click-to-cancel.
- Reuse inspected editor context with manifest, executable and workspace
  invalidation; report full build duration including preparation.
- Query system TeX directories in one Kpathsea invocation per build.
- Preserve dynamically discovered package files across lock refreshes so warm
  edits avoid repeated package convergence and TeX discovery passes.
- Preserve dotted job names in artifact lookup, glossary outputs, editor opening
  and the browser viewer.
- Keep wrapped fatal summaries from hiding the useful source error in long paths.
- Refresh editor paths after manifest edits and balance the paper's editor panes.

- Publish the project lock, SyncTeX, and PDF with rollback on write failures;
  stale SyncTeX is removed, and derived-cache failures no longer fail a
  completed build.
- Read release-suite revisions through the validated TOML parser instead of
  an over-escaped shell expression in CI and release workflows.

- Watch mode excludes the published PDF and SyncTeX files by exact name while
  continuing to track similarly named input archives.

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
