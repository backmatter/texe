# Contributing

Discuss changes to public formats, supported platforms, or build security in an
issue first; small fixes can go straight to a pull request. Report
vulnerabilities through [SECURITY.md](SECURITY.md).

## Set up a checkout

Install Git and [rustup](https://rustup.rs/). `rust-toolchain.toml` selects the
Rust version.

```sh
git clone https://github.com/backmatter/texe.git
cd texe
```

Integration tests also need the pinned pqty checkout:

```sh
git clone https://github.com/backmatter/pqty.git ../pqty
pqty_revision="$(cargo xtask pqty revision)"
git -C ../pqty switch --detach "$pqty_revision"
cargo xtask pqty check ../pqty
```

Set `PQTY_REPO` if pqty lives elsewhere.

## Run the checks

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings -D missing-docs" cargo doc --workspace --no-deps --locked
node --test tests/*.test.js
```

Dependency policy needs `cargo-deny`. Install it with
`cargo install cargo-deny --locked`, then run `cargo deny check`.

## Verify builds end to end

```sh
cargo xtask verify contracts   # the three commands against a local registry
cargo xtask verify local       # build papers with a system TeX Live
cargo xtask verify platform    # managed install and build checks per platform
cargo xtask verify             # Rust checks, contracts, and managed build cases
```

- `verify contracts` builds the pinned pqty checkout and tests texe, pqty, and
  pqty-fls against an isolated local package registry. It needs no TeX
  installation and no downloads.
- `verify local` needs system `pdflatex`, `kpsewhich`, and a local
  `texlive.tlpdb`. It covers PDF production, package discovery, frozen
  rebuilds, repeatable output, and recovery from publication failures.
- Plain `cargo xtask verify` reruns the Rust checks, then downloads the pinned
  tools and packages to check pdfLaTeX, LuaLaTeX, bibliographies, indexes, and
  glossaries with empty caches. Install Poppler's `pdffonts` for the font
  checks. It does not include the `local` or `platform` cases.

Add `--suite-bin /path/to/suite/bin` to any single case to reuse existing
binaries; the directory must hold all three executables. On Linux you can also
run a case with networking disabled, which needs permission to create user and
network namespaces:

```sh
unshare --user --map-root-user --net \
  target/debug/xtask verify contracts --suite-bin /path/to/suite/bin
```

## Test the VS Code integration

```sh
npm ci --prefix tests/vscode --ignore-scripts
TEXE_TEST_MANAGED=1 TEXE_TEST_SUITE=/absolute/path/to/suite npm test --prefix tests/vscode
```

Omit `TEXE_TEST_MANAGED=1` to use system `pdflatex` and `kpsewhich`. The tests
run against a temporary VS Code profile, extension directory, and paper,
covering PDF refresh, diagnostics, failure recovery, SyncTeX, build commands,
and manifest changes. Linux needs a display server or Xvfb. Screenshots and
`results.json` land in the temporary directory the run prints.

The harness downloads the VS Code and LaTeX Workshop versions pinned in
[`tests/vscode/run.js`](tests/vscode/run.js). To use local copies, set
`VSCODE_EXECUTABLE` to an Electron executable and `LATEX_WORKSHOP_PATH` to an
installed extension directory.

## Pull requests

Explain the behavior change and how you tested it. Include regression tests for
bugs and new behavior, and update affected schemas and user documentation. Use a
Conventional Commit title, such as `fix(cli): explain a missing lock`.

The title becomes the changelog entry, so write it for a reader of the release
notes. `feat`, `fix`, and `docs` titles are published; `chore`, `ci`, `build`,
`refactor`, `style`, and `test` are omitted. Add a `changelog: ignore` footer to
leave out a commit that would otherwise appear.

The CLI and versioned data formats are public contracts. The Rust library is an
internal API and may change between releases.

## Releases

release-plz opens a release PR that bumps the version, rewrites `CHANGELOG.md`
from the commit titles, and tags and drafts the GitHub release once merged. The
release body repeats the changelog entry. Do not edit `CHANGELOG.md` by hand;
correct a wrong entry by amending the commit title before it is released.

Contributions are licensed under the [MIT License](LICENSE).
