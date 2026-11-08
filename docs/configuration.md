# Project configuration

Most users do not need to edit configuration for their first paper. The guided
setup creates `texe.toml` in the project folder with safe defaults.

The smallest manifest is:

```toml
schema = "texe.project/v1"

[project]
entry = "main.tex"

[toolchain]
engine = "pdflatex"
```

Paths use forward slashes, are relative to the project, and cannot escape it.
Unknown fields are rejected, so a misspelled setting fails instead of silently
changing the build.

This guide covers the settings projects actually tend to change. The
[project schema](../schemas/texe.project.schema.json) is the complete
reference.

## Toolchain

By default texe downloads and isolates a verified toolchain instead of using a
system TeX installation. Guided setup picks pdfLaTeX; for LuaLaTeX, run
`texe init --engine lualatex`, or change the engine before the first build:

```toml
[toolchain]
engine = "lualatex"
channel = "stable"
```

`stable` may point at a newer reviewed recipe in a later texe release. The
[latest release notes](https://github.com/backmatter/texe/releases/latest) name
the exact recipe ID behind the alias; use that ID as `channel` to keep one
runtime across texe upgrades.

To use an existing TeX installation instead:

```toml
[toolchain]
provider = "system"
engine = "xelatex"
```

Protected builds expose locked fonts to native family lookup through a private
Fontconfig configuration. Type 1 faces are excluded from that lookup: duplicate
legacy faces can shadow OpenType fonts with the same name and lack their Unicode
glyphs. Type 1 metrics and outlines remain available to TeX through its usual
locked font maps. This does not pin ambient native font libraries or promise
identical shaping across different library versions.

The system provider is outside texe's reproducibility guarantee: host
executables, formats, packages, and fonts can all affect the output.

To build with [Shipout](https://github.com/backmatter/shipout), put the
`shipout` executable on your `PATH` and select it:

```toml
[toolchain]
provider = "system"
engine = "shipout"
```

Shipout carries its own LaTeX kernel and base fonts, so it needs no TeX
installation. It runs the passes, BibTeX, and index stages a document needs
itself. Packages still come from `texe.lock`. `texe watch` keeps one Shipout
process running, so an edit resumes from the engine state just before the
change instead of starting over.

## Project inputs

Declare additional project folders that contain classes, styles, images, or
nested input files:

```toml
[inputs]
roots = ["styles", "figures/shared"]
```

texe searches these folders for source files and package dependencies.

A project can also declare a small generated input by its exact content:

```toml
[[project.generated]]
path = "BuildInfo.tex"
content = "\\newcommand{\\BuildLabel}{review-copy}\n"
```

Generated inputs are written only inside texe's private build directories. texe
never runs a generator or overwrites a file in your source tree.

## Package storage

Packages are copied into the project's package tree. This is the default and the
supported mode; `experimental-symlink` and `experimental-hardlink` trade
isolation for links into the shared store.

Registry data, downloads, and the shared store live below `TEXE_HOME/pqty`, so
`texe storage` and `texe clean --all` cover the same storage on every platform.
Set a project-local store when the package bytes must live with the project:

```toml
[packages]
store = ".texe/package-store"
```

That path holds generated, replaceable data; do not commit it.

## Bibliography and indexes

BibTeX, Biber, MakeIndex, and MakeIndex-backed glossaries are detected
automatically. Extra bibliography search roots need no command override:

```toml
[bibliography]
roots = ["bibliography/styles"]
```

Projects that deliberately use their own tools can override the commands:

```toml
[toolchain]
engine = "pdflatex"
allow_unmanaged_commands = true

[bibliography]
biber = "tools/biber"

[index]
makeindex = "tools/makeindex"
```

Managed mode rejects overrides unless `allow_unmanaged_commands = true` is set.
Opted-out builds warn on every run, can execute project or host software, and
skip the no-op build cache.

## Shell escape

Shell escape is off by default. Enable it only for a document that intentionally
runs external commands, such as one using `minted`:

```toml
[toolchain]
engine = "pdflatex"
shell_escape = true
```

Shell escape exposes the host `PATH`, lets the document run arbitrary commands,
disables the no-op build cache, and ends the reproducibility guarantee.

## What to commit

Commit `texe.toml` and `texe.lock`. Everything below `.texe/` is generated.
Protected builds also publish the Paper lock at the project root; compilers
cannot modify it.

`texe.lock` keeps one exact pqty package selection and a `runtimes` map keyed by
portable target names (for example, `linux-x64` and `macos-arm64`). An unlocked
target is added by a non-frozen build using the existing packages. A frozen
build requires an identity for the current target and checks its fingerprint
and artifact hashes. Changing packages clears other targets' identities until
they have been verified against the new selection.

Run `texe fetch --project .` to restore and verify the locked archives and
materialize their package tree without executing TeX. It requires an existing
Paper lock, keeps its package selection, and can add the current target's
verified runtime identity. Use `--frozen` to require that identity already,
and `--offline` to use populated caches only. A Paper without a lock needs an
initial `texe build` to discover and resolve its dependencies.

A Paper manifest always owns its build settings. Connected environment bundles
populate verified runtime and package caches; `.texe/environment.json` cannot
override `texe.toml` or `texe.lock`.

Hosted operators can set `TEXE_HOSTED=1` to require managed tools (or the
opt-in Shipout runtime), reject shell escape and external source roots, and
set `TEXE_HOST_SECCOMP_PATH` to an operator-owned Linux BPF filter. The filter
is intersected with texe's compiler policy for every source interpreter.
Fetch can use an explicit proxy; protected offline builds never need it.

Locked formats retain Knuth's English patterns as language zero and enable
additional languages whose registry declarations and complete pattern files are
already in the package closure. The declarations come from the exact pqty lock,
so frozen offline builds do not fetch language metadata. texe generates the
TeX and Lua language configuration from that metadata and preserves declared
synonyms, minima and Lua-specific restrictions. This does not infer or install
arbitrary languages from document text.

Each generated format records the files initex read (from `-recorder`) with
their SHA-256, relative to the format staging, the locked package tree or the
managed runtime. A format is reused instead of regenerated whenever every one
of those files is byte-identical in the current build, so adding a package
that the format never reads (a font provider found during discovery) keeps it.
Protected builds also offer their formats to other build directories through
`TEXE_HOME/formats/by-inputs`; a build takes one only after hashing its own copy
of every recorded input. A format that read anything else, such as a project
file through a `.` search path, is neither reused nor shared. Protected builds
unpack the pinned Biber runtime once per executable into `TEXE_HOME`, which
compilers can read and execute but not modify, and give each run its own view
of it.
