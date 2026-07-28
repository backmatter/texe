# Contributing to texe

Thank you for helping improve texe. Start with an issue before making a change
that alters the manifest, lock or JSON schema contracts, supported platforms,
managed trust boundary, release artifacts, or pqty integration. Small fixes and
documentation improvements can go directly to a pull request.

Use the [issue tracker](https://github.com/backmatter/texe/issues) for public
design discussion. Vulnerabilities follow the private process in
[SECURITY.md](SECURITY.md).

## Fresh-clone setup

Install Git and [rustup](https://rustup.rs/), then clone texe:

```sh
git clone https://github.com/backmatter/texe.git
cd texe
rustup show active-toolchain
```

`rust-toolchain.toml` selects the supported Rust toolchain and installs
Rustfmt and Clippy. The ordinary Rust checks do not require TeX Live or a pqty
checkout.

The complete command-suite and managed-toolchain tests use pqty at the exact
revision in `suite.lock.toml`. Clone it beside texe:

```sh
git clone https://github.com/backmatter/pqty.git ../pqty
pqty_revision="$(sed -n 's/^pqty_revision = "\([^"]*\)"/\1/p' suite.lock.toml)"
git -C ../pqty switch --detach "$pqty_revision"
cargo xtask pqty check ../pqty
```

Set `PQTY_REPO=/another/path/pqty` when the repositories are not siblings.

## Fast checks

Run these while iterating and before every pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RUSTDOCFLAGS="-D warnings -D missing-docs" cargo doc --workspace --no-deps --locked
```

The dependency policy check requires
[`cargo-deny`](https://github.com/EmbarkStudios/cargo-deny):

```sh
cargo install cargo-deny --locked
cargo deny check
```

CI repeats these checks on Linux x86-64, Windows x86-64, and macOS Apple
Silicon.

## Networked acceptance tests

Changes to the build pipeline, managed toolchains, release packaging, or pqty
boundary also need:

```sh
cargo xtask verify
```

This Linux-oriented gate builds the pinned sibling pqty checkout and exercises
pdfLaTeX, LuaLaTeX, BibTeX, Biber, indexes, glossaries, offline operation, and
empty-cache frozen reproduction. It downloads the pinned TeX runtime and
package containers. Install Poppler's `pdffonts` command for the font checks.

Use `cargo xtask verify local` to check the system provider explicitly when a
host `pdflatex` and `kpsewhich` are installed. Hosted CI runs the clean managed
user journey separately on every supported target. Describe any environment
limitation in the pull request when a relevant networked test cannot be run
locally.

## Making changes

Add regression tests for behavior changes. Keep JSON and TOML schemas,
documentation, fixtures, and human error text aligned with the implementation.
Do not weaken path, digest, size, redirect, or command-execution checks to make
a fixture pass.

Public contracts are the CLI, project and lock formats, versioned JSON results,
watch events, and documented trust boundary. The Rust library is an
implementation API and may change between releases.

Pull requests should:

- stay focused and leave unrelated working-tree changes untouched;
- explain user-visible behavior and trust-boundary changes;
- include tests for defects or new behavior;
- use a Conventional Commit title such as `fix(cli): explain a missing lock`;
- update documentation when users need to act differently.

By contributing, you agree that your contribution is licensed under this
repository's MIT License.

Security reports follow [SECURITY.md](SECURITY.md), not the public issue
tracker.
