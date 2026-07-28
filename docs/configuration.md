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
Unknown fields are rejected so misspelled settings do not silently change a
build.

The [project schema](../schemas/texe.project.schema.json) is the complete field
reference. This guide explains the settings that most projects are likely to
change.

## Toolchain

The toolchain is the LaTeX engine and runtime used to build the paper. The
default managed provider lets texe download and isolate a verified toolchain
instead of relying on a system TeX installation.

Guided setup uses pdfLaTeX. To use LuaLaTeX instead, run
`texe init --engine lualatex` when creating the project or change the engine
before the first build:

```toml
[toolchain]
engine = "lualatex"
channel = "stable"
```

The default managed provider selects the current embedded `stable` recipe.
`stable` may select a newer reviewed recipe in a later texe release. The
[latest release notes](https://github.com/backmatter/texe/releases/latest)
name the exact recipe ID behind that alias. Use that ID as `channel` when a
project must keep the same runtime independently of future texe upgrades.

Use an existing TeX installation explicitly:

```toml
[toolchain]
provider = "system"
engine = "xelatex"
```

The system provider is outside texe's managed reproducibility boundary. Host
executables, formats, packages, and fonts may affect its output.

## Project inputs

Declare additional project folders that contain classes, styles, images, or
nested input files:

```toml
[inputs]
roots = ["styles", "figures/shared"]
```

texe adds these folders to LaTeX's input search, keeps them below the project
directory, asks pqty to scan them, and includes them in the lock and build
fingerprint.

A project can also declare a small generated input by exact content:

```toml
[[project.generated]]
path = "BuildInfo.tex"
content = "\\newcommand{\\BuildLabel}{review-copy}\n"
```

texe materializes generated inputs only in its private build directories. It
does not execute a generator or overwrite a source-tree file.

## Package storage

Copy mode is the supported package-tree mode and the default:

```toml
[packages]
link = "copy"
```

`experimental-symlink` and `experimental-hardlink` trade isolation for links
into pqty's shared store.

By default, texe keeps pqty's registry data, package downloads, and shared
store below `TEXE_HOME/pqty` together with its other managed data. This makes
`texe storage` and `texe clean --all` report or remove the
same owned storage on every platform.

Set a project-local store when package bytes must live with the project:

```toml
[packages]
store = ".texe/package-store"
```

The path contains generated, replaceable data and should not be committed.

## Bibliography and indexes

BibTeX, Biber, MakeIndex, and MakeIndex-backed glossaries are detected
automatically. Additional bibliography search roots can be declared without
overriding a command:

```toml
[bibliography]
roots = ["bibliography/styles"]
```

Command overrides are available for projects that deliberately use their own
tools:

```toml
[toolchain]
engine = "pdflatex"
allow_unmanaged_commands = true

[bibliography]
biber = "tools/biber"

[index]
makeindex = "tools/makeindex"
```

Managed mode rejects command overrides unless
`allow_unmanaged_commands = true` is set. Opted-out builds warn on every run,
can execute project or host software, and do not use the no-op build cache.

## Shell escape

Shell escape is disabled by default. Enable it only for a document that
intentionally runs external commands, such as one using `minted`:

```toml
[toolchain]
engine = "pdflatex"
shell_escape = true
```

Enabling shell escape exposes the host `PATH`, permits arbitrary command
execution selected by the document, disables the no-op build cache, and ends
the full reproducibility guarantee.

## Private build paths

`project.build_dir`, `packages.lock`, and `packages.texmf` can be relocated only
below `.texe/`. The defaults are suitable for normal projects:

```toml
[project]
entry = "main.tex"
build_dir = ".texe/build"

[packages]
lock = ".texe/state/pqty.lock"
texmf = ".texe/texmf"
```

Everything below `.texe/` is generated and can be recreated. Commit
`texe.toml` and `texe.lock`, but do not commit `.texe/`.
