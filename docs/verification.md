# Verification coverage

Verification is split by cost. Use local fixtures during development; use the
managed journey to qualify downloadable runtimes and platform releases.

## Fast checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings -D missing-docs" cargo doc --workspace --no-deps --locked
node --test tests/*.test.js
```

Run the Rust checks in pqty too; its documentation check uses `-D warnings`.
These checks do not require a TeX installation.

## Local command-suite contracts

```sh
cargo xtask verify contracts
```

This verifies and builds the pinned pqty checkout, then runs the real texe,
pqty, and pqty-fls binaries with a synthetic registry containing one package.
The subprocesses use isolated homes, caches, and explicit offline options.
No engine, registry download, or user paper is needed.

To test already built development or extracted release binaries:

```sh
cargo xtask verify contracts --suite-bin /path/to/suite/bin
```

The directory must contain all three executables. This option tests their
behavior without claiming they match `suite.lock.toml`.

On Linux, test with networking actually disabled after building the tools:

```sh
unshare --user --map-root-user --net \
  target/debug/xtask verify contracts --suite-bin /path/to/suite/bin
```

This requires permission to create user and network namespaces. If the command
fails to create them, network isolation has not been tested. Application-level
`--offline` flags alone are not proof that network access was impossible.

| Promise | Evidence |
| --- | --- |
| Headless setup works | Real CLI initialization in a path containing spaces and Unicode; CLI tests cover quiet and JSON modes |
| Offline failures preserve project files | Empty-cache build fails with a JSON error while source, previous artifact, and lock remain unchanged |
| Package identity is deterministic | Two empty stores produce identical locks and environment JSON for the same local source |
| Ambient configuration cannot alter integrations | An invalid project `pqty.toml` is ignored by explicit `--no-config` calls |
| Recorder and package protocols agree | Real pqty-fls output is accepted by pqty check-trace; a stale environment fingerprint is rejected |
| Cached installation works without the source | The registry is deleted before a second install from the store |
| Corruption cannot replace working packages | A corrupt store object causes offline installation to fail, preserves the installed tree and lock, and retains quarantine evidence |
| Publication failures preserve previous outputs | Unit tests inject a late write failure and verify rollback of replacements, deletions, and new files |
| Watch waits for stable inputs | Deterministic clock tests exercise continuous changes beyond the former retry limit and cancellation of reverted changes |
| Recorder input memory is bounded | pqty-fls tests verify the input boundary and stop reading at the limit plus one byte |

The local contracts run before the managed journey in CI and against extracted
binaries in release jobs.

## Small real-engine check

When a system TeX installation with `pdflatex`, `kpsewhich`, and a local
`texlive.tlpdb` is available:

```sh
cargo xtask verify local --suite-bin /path/to/suite/bin
```

This uses the repository's tiny convergence example. It checks real PDF
production, runtime package discovery, frozen rebuilding, byte identity on
repeated builds, a blocked SyncTeX destination, and recovery with an unwritable
build-cache destination. On Linux it can run under the same `unshare` command.

This is a system-provider check; it does not qualify managed runtime downloads.

## Release qualification

```sh
cargo xtask pqty check
cargo xtask verify
```

The full gate includes the managed engine, bibliography, index, glossary, and
empty-cache frozen reproduction cases. CI additionally runs
`cargo xtask verify platform` on Linux x86-64, Windows x86-64, and macOS Apple
Silicon. Release jobs exercise the packaged binaries and installers.

A passing local check does not establish Windows/macOS installation behavior,
managed download availability, or compatibility with arbitrary publisher
classes and fonts. Those require the corresponding platform and document
coverage. Cross-platform PDF byte identity is not a supported guarantee.

Output publication rolls back ordinary write failures. Individual replacements
are atomic; replacing the PDF, SyncTeX, and lock is not a single transaction
across power loss or process termination.

## Real VS Code workflow

With the command suite built, run a blank-cache managed editor journey:

```sh
TEXE_TEST_MANAGED=1 TEXE_TEST_SUITE=/absolute/path/to/suite npm test --prefix tests/vscode
```

For a local system-provider check with `pdflatex` and `kpsewhich` installed:

```sh
npm ci --prefix tests/vscode --ignore-scripts
TEXE_TEST_SUITE=/absolute/path/to/suite npm test --prefix tests/vscode
```

The test creates a temporary profile, extension directory and paper folder.
It never uses your normal VS Code profile. By default it downloads pinned
VS Code 1.126.0 and LaTeX Workshop 10.17.1. Set `VSCODE_EXECUTABLE` to an existing
Electron executable and `LATEX_WORKSHOP_PATH` to an installed extension directory
to copy those instead. CI runs the managed editor journey on Linux x86-64 (under Xvfb), Windows
x86-64, and macOS Apple Silicon, using optimized binaries.

The harness builds an actual PDF, inspects its rendered text in the real
webview, checks a save-triggered rebuild and viewer refresh, injects a source
error, clicks the actual Problems row, checks previous-PDF retention, repairs the
error, and verifies forward/inverse SyncTeX with LaTeX Workshop. It also checks the
automatic error toast and its navigation action, the F6 shortcut, LaTeX
Workshop’s Build button, and changing the manifest entry. Three warm edits
record save-to-visible-PDF latency, including debounce and viewer refresh. Screenshots and
`results.json` remain in the printed temporary directory. LaTeX Workshop internal
hooks are used only in the pinned navigation tests, never in the shipped companion.

This is a small synthetic paper. Local default runs use the system provider;
`TEXE_TEST_MANAGED=1` uses downloaded managed tools in isolated caches. It verifies editor
interactions without requiring a publisher-sized document corpus; it does not
claim that arbitrary projects or all VS Code/LaTeX Workshop versions work.

The platform journey also adopts `examples/pilot`, a multi-file paper with a
local class, included chapters, bibliography, cross-references, a PNG figure,
and a TeXstudio session file. It checks the first build, frozen offline rebuild,
and preservation of the session file.
