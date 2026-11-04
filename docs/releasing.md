# Release the native suite

texe ships with the exact pqty and pqty-fls sources recorded in
`suite.lock.toml`. Each texe release builds and tests that combination on Linux,
Windows and macOS. Protected local Citet builds currently require Linux x64;
other editor hosts verify language features and reject unsupported local builds. A pqty release becomes available to texe users through a new
texe suite release. Editor extensions have independent release cycles.

## Update pqty

After a pqty release, pin it by hand and open a normal pull request:

```sh
git -C ../pqty fetch --tags
git -C ../pqty switch --detach v0.1.4
cargo xtask pqty update v0.1.4 ../pqty
cargo xtask verify contracts
```

The update records the release's Git revision, version and capabilities schema
in `suite.lock.toml`. The pull request runs the usual platform checks; after
merge, release-plz prepares the texe release PR.

## Editor delivery

Citet installs the official checksummed texe suite as a unit. It checks for a
compatible update during online builds, at most once per day, and offers an
explicit update command. Cached offline builds keep their installed suite.
Tools installed through PATH or a configured executable path remain managed by
their original installer. A pqty or texe patch does not require republishing Citet.

tex-ls publishes its language server and editor packages through its own release
workflows. Change that dependency only when its own source needs a release.
