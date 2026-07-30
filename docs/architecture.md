# Architecture

`texe` is the user-facing LaTeX workflow. It composes a toolchain provider, an
engine adapter, and pqty's package environment:

```text
project source
      |
      v
texe -> toolchain provider
  |  -> engine adapter
  `----> pqty -> exact package environment
      |
      v
PDF + SyncTeX + texe.lock
```

The dependency direction is deliberate. texe consumes pqty through its CLI and
versioned JSON artifacts; pqty does not depend on texe or any TeX engine.

## Toolchain boundary

A provider resolves an engine request into an executable, immutable runtime
roots, environment variables, and adapter capabilities.

The default `managed` provider selects an embedded recipe from
`toolchains/catalog.toml` and `toolchains/recipes/`. Each recipe owns the
Registry Snapshot identity, engines, formats, bootstrap providers, native
platform containers, and Biber component. Downloads are bounded and accepted
only after their pinned size and digest match. Installed files are hashed at
installation and periodically or explicitly reverified.

The `system` provider is an opt-in boundary. It discovers host executables and
runtime roots. In remote package mode texe creates a private format from the
locked LaTeX/L3 package layer; local mode retains the host format.

See [managed toolchain recipes](toolchain-recipes.md) for adding a snapshot.

## Build pipeline

The kpathsea adapter supports pdfTeX, XeTeX, and LuaTeX. A normal managed build:

1. resolves the requested toolchain;
2. asks pqty to lock and materialize the package environment;
3. generates or reuses a format keyed by toolchain and package fingerprints;
4. runs an engine discovery pass with `-recorder`;
5. translates the `.fls` file with `pqty-fls` and converges missing packages;
6. runs BibTeX, Biber, MakeIndex, or glossaries when their control files
   require it;
7. performs frozen passes until the relevant auxiliary files stabilize;
8. atomically publishes the PDF and SyncTeX file.

The adapter owns engine invocation, pass scheduling, logs, bibliography and
index tools, font maps, and SyncTeX. pqty owns source discovery, package
resolution, package integrity, and the materialized TEXMF tree.

Managed processes receive explicit TeX, Lua, font, bibliography, and command
search paths. Shell escape is disabled unless the project opts in. A failed
build does not replace a previously published PDF.

## pqty boundary

`PqtyClient` first negotiates capabilities and requires the Artifact Protocol
schemas texe consumes:

- `pqty.lock/v1`
- `pqty.env/v1`
- `pqty.trace/v1`
- `pqty.trace-report/v1`
- `pqty.convergence-report/v1`

Every pqty call uses `--no-config`; project, Registry Snapshot, and store
choices come from texe. Package trees use pqty's read-only copy mode by
default. The experimental link modes are explicit.

`suite.lock.toml` pins the pqty tag, version, revision, and source digest used
to build the three-command release suite. Packaging verifies all four before
compilation.

## Persistent and derived state

`texe.lock/v1` is the user-owned reproducibility boundary. It records:

- provider, engine, target, channel, and toolchain fingerprint;
- managed runtime and Registry Snapshot integrity;
- the pinned build timestamp;
- the complete `pqty.lock/v1` object.

A frozen build requires that lock, verifies missing cached content against its
recorded sources, and forbids package convergence.

Everything below `.texe/` is derived. It includes engine output, the internal
pqty lock and TEXMF tree, timing history, and build state. `texe.build-state/v1`
fingerprints the effective manifest, toolchain, lock, source inputs, and
published outputs. When it still matches, an ordinary build can return the
existing artifact without starting the engine. Shell escape disables this
optimization because arbitrary host inputs cannot be fingerprinted.

One operating-system lock serializes builds for a project. `texe watch` uses
the same build path and excludes declared output and state directories from
its source snapshot.

Managed runtimes, components, formats, downloads, pqty package data, and the
editor companion live in named shared roots below `TEXE_HOME`. texe gives pqty
that same cache home on every platform. The data is reproducible from embedded
recipes and locks. Cleanup only traverses those named owned roots;
projects and unknown files in a custom `TEXE_HOME` are never removal targets.

## Reproducibility and trust

Managed builds pin the engine and package bytes, construct isolated search
paths, and provide `SOURCE_DATE_EPOCH` and `FORCE_SOURCE_DATE`. The lock's
timestamp is reused across frozen builds. An inherited `SOURCE_DATE_EPOCH`
overrides one build without repinning the project.

HTTPS is required for normal remote access, but content identity comes from
pinned sizes and cryptographic digests. These controls detect corruption and
drift; they do not authenticate the publisher. Archive entries and
project-owned paths are confined before filesystem access.

The explicit limits are the `system` provider, system fonts, enabled shell
escape, and explicitly permitted unmanaged command overrides. Those inputs
belong to the host or project environment and therefore sit outside the
managed reproducibility claim. Builds crossing one of these command boundaries
do not use the no-op cache. Fully managed cache identity includes the exact
pqty and pqty-fls executable bytes.

## Presentation and integrations

Workflow code returns domain reports and `TexeError`. Presentation code maps
them to focused human output or closed JSON contracts. Progress uses stderr;
one-shot JSON results use stdout; watch mode emits one
`texe.watch-event/v1` JSON object per line.

The local viewer binds only to `127.0.0.1` and serves a fixed set of PDF.js
resources plus the current PDF. It does not own build logic.

Project-local integrations are adapters under `src/integrations/`. Git owns
repository initialization and a marked `.gitignore` block. The VS Code adapter
owns optional project-local settings and a small bundled layout companion. It
does not merge existing settings or create a separate workspace. Keeping editor
behavior behind this boundary allows additional editors without coupling them
to project setup or builds.

## Code map

| Location | Responsibility |
| --- | --- |
| `src/app/` | CLI command orchestration and output selection |
| `src/cli.rs` | Command-line types and parsing |
| `src/config/` | Manifest discovery, validation, and starter projects |
| `src/build/` | Build pipeline, engine passes, processors, traces, and artifacts |
| `src/toolchain/` | Recipe catalog plus managed and system providers |
| `src/progress/` | Progress protocol, rendering, and advisory history |
| `src/package.rs` | The pqty process boundary |
| `src/integrations/` | Git and editor adapters |
| `src/lockfile.rs`, `src/state.rs` | User lock and derived build state |
| `src/diagnostics.rs`, `src/ux.rs` | Diagnostics and presentation contracts |
| `src/viewer.rs` | Optional local PDF viewing |
| `src/clean.rs` | Storage reporting and cleanup |
| `schemas/` | Public machine-readable contracts |
| `toolchains/` | Embedded managed recipe data |

Release and CI behavior is documented in the [release runbook](releasing.md),
not in this architecture.
