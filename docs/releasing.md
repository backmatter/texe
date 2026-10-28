# Release the native suite

texe ships with the exact pqty and pqty-fls sources recorded in
`suite.lock.toml`. Each texe release builds and tests that combination on Linux,
Windows and macOS. Protected local Citet builds currently require Linux x64;
other editor hosts verify language features and reject unsupported local builds. A pqty release becomes available to texe users through a new
texe suite release. Editor extensions have independent release cycles.

## Update pqty

After publishing its stable GitHub release, pqty dispatches `pqty-release` to
this repository. **Update pqty suite** validates the published tag and checks
that its commit belongs to pqty's main history. It records the source revision,
checksum and advertised protocol schemas, then verifies installed-suite contracts.
It prepares a dependency PR for the existing main protection and platform checks.
There is no bypass or automatic merge. After merge, the existing release-plz
process prepares the texe release PR and publishes its release when merged.

The helper's `--if-newer` option ignores older releases and an already pinned
revision. A different revision using the pinned version fails. Never replace an
existing release tag to deliver changed binaries.

To retry a missed or failed handoff:

```sh
gh workflow run pqty-update.yml --repo backmatter/texe --ref main -f tag=v0.1.2
```

The tag must already be a published stable pqty release. Repeated deliveries
reuse an existing dependency PR. A retry also recovers a push interrupted before
PR creation, provided the branch contains the same suite lock. A declined, closed
PR is left closed. A conflicting existing automation branch requires inspection.

For a local update, use a clean pqty checkout at the released tag:

```sh
cargo xtask pqty update v0.1.2 /path/to/pqty --if-newer
PQTY_REPO=/path/to/pqty cargo xtask verify contracts
```

## Configure the handoff

Install this receiver workflow on main before pqty's dispatcher. Both repositories
already use a release GitHub App with `RELEASE_APP_CLIENT_ID` and
`RELEASE_APP_PRIVATE_KEY`. pqty's app must also have an installation on texe with
Contents write permission for repository dispatch. texe's own release app needs
Contents write and Pull requests write permissions for the dependency PR. Keep
repository access limited to the repositories each app needs. An app installed
only on pqty cannot dispatch to texe.

Verify the installation scope and a live dispatch during rollout. Local linting
does not verify GitHub App access or run the protected CI checks. A handoff failure
leaves pqty published and can be recovered through the manual workflow above.

## Editor delivery

Citet installs the official checksummed texe suite as a unit. It checks for a
compatible update during online builds, at most once per day, and offers an
explicit update command. Cached offline builds keep their installed suite.
Tools installed through PATH or a configured executable path remain managed by
their original installer. A pqty or texe patch does not require republishing Citet.

tex-ls publishes its language server and editor packages through its own release
workflows. Change that dependency only when its own source needs a release.
