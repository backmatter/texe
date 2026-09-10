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

The system provider is outside texe's reproducibility guarantee: host
executables, formats, packages, and fonts can all affect the output.

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
