# Releasing texe

release-plz maintains a release PR and creates the reviewed tag and draft. The
tag workflow publishes through crates.io Trusted Publishing and builds the
native artifacts, checksums, provenance, and GitHub Release.

```text
normal PRs -> release-plz PR -> merge commit -> vX.Y.Z tag
                                                |
                                                +-> trusted cargo publish
                                                +-> tested native suites
                                                +-> GitHub Release
```

release-plz owns version/changelog preparation, the tag, and a draft GitHub
Release. The tag workflow makes the draft public only after crate publication
and every required artifact job succeed.

## Release automation setup

Install the release GitHub App only on `backmatter/texe`, with:

- metadata: read;
- repository contents: read and write;
- pull requests: read and write.

Set repository variable `RELEASE_APP_CLIENT_ID` and secret
`RELEASE_APP_PRIVATE_KEY`. The App-created release PR runs normal CI and its
tag triggers the separate release workflow; no personal access token is
needed.

Also:

1. Configure the crates.io Trusted Publisher for `backmatter/texe`, workflow
   `release.yml`, and environment `crates.io`.
2. Protect `main`, require pull requests and the full CI matrix, and disallow
   force pushes and deletion.
3. Allow squash merges for normal PRs. Merge release-plz PRs with
   **Create a merge commit**, not squash or rebase, so the reviewed release
   commit is the tag target.
4. Protect `v*` tags and allow the release App to create them.
5. Keep the default `GITHUB_TOKEN` read-only; jobs declare narrower elevated
   permissions where required.
6. Enable private vulnerability reporting.

## Normal releases

Use Conventional Commit PR titles. Ordinary PRs are squash-merged, so the title
becomes the commit release-plz evaluates:

```text
fix(store): reject a corrupt object
feat(cli): explain the selected toolchain
docs: clarify offline behavior
feat(protocol)!: introduce an incompatible artifact schema
```

After a push to `main`, release-plz opens or updates one release PR. Review its
Cargo version, lockfile, and changelog. Confirm that the proposed version
matches the compatibility impact described by Semantic Versioning.

Merge the release PR with a merge commit. release-plz creates the tag and draft
release. The tag workflow:

- requires the tag version and commit to match protected `main`;
- dry-runs the crates.io package;
- publishes the crate through Trusted Publishing;
- builds the exact pqty revision from `suite.lock.toml`;
- packages and extracts each native command suite;
- runs `cargo xtask verify contracts` and `cargo xtask verify platform` against
  the installed binaries;
- builds and installs the Debian package;
- tests the Linux, macOS, and Windows portable installers and uninstallers;
- builds `texe-aarch64-macos.dmg` with the AppKit welcome app and private suite;
- builds `texe-x86_64-windows-setup.exe` with the Windows Forms welcome app,
  then tests installation, bundled project setup, and uninstall;
- renders Homebrew and WinGet submissions;
- writes one aggregate `SHA256SUMS`, attests all assets, and publishes the
  draft GitHub Release.

## Native desktop releases

The native welcome apps are built from `desktop/` and bundled with the same
pinned command suite that passed the installed-suite tests. Portable archives
remain available for CLI users. Follow the [desktop acceptance checklist](../desktop/README.md)
on a clean Windows and macOS account before releasing the graphical downloads.

Configure signing credentials on the release hosts as described in the
[desktop signing guide](../desktop/README.md#signing). The package scripts support
Developer ID signing and notarization on macOS, and timestamped Authenticode
signatures on Windows. Hosted CI currently produces unsigned Windows and ad-hoc
signed macOS artifacts unless signing credentials are provisioned. State that
status in release notes; these builds can show OS security prompts and must not
be presented as signed or notarized.

## Updating pqty

Publish pqty first. Then check out the intended public tag and update texe's
suite lock:

```sh
pqty_tag="<public-pqty-tag>"
git -C ../pqty switch --detach "$pqty_tag"
cargo xtask pqty update "$pqty_tag" ../pqty
cargo xtask pqty check ../pqty
cargo xtask verify
```

Commit the suite-lock update through an ordinary texe PR. Third-party release
versions do not affect texe-owned protocol identifiers, which change only when
their formats become incompatible.

## Failure and rollback

If no crate or GitHub Release became public, fix external configuration and
rerun the failed job. If source must change, delete the unpublished tag, merge
the fix, and let release-plz prepare a corrected release.

If a crate version is public, never reuse that version or move its tag. Fix the
pipeline and rerun it, or publish a higher version through the next release PR.
Never replace public asset bytes; yank an unusable crate when appropriate.
