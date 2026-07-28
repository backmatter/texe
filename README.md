# texe

texe is a command-line app for creating and building LaTeX papers. You write
the paper in a `.tex` file; texe installs the LaTeX tools and packages it needs
and produces the PDF. You do not need to install or maintain TeX Live
separately.

## Create your first paper

1. [Install texe](docs/install.md) for your computer.
2. Open **Terminal** on Linux or macOS, or **PowerShell** on Windows.
3. Move into the folder that should contain the new paper. For example, use
   `cd ~/Documents` on Linux or macOS, or `cd "$HOME\Documents"` in
   PowerShell.
4. Run:

   ```sh
   texe
   ```

5. Follow the guided setup. It creates a basic scientific paper with pdfLaTeX
   and builds the first PDF. Git and VS Code are optional; choose **No** if you
   do not use them yet.

The first build needs an internet connection and takes longer because texe
downloads and checks the required LaTeX tools and packages. Later builds reuse
those downloads.

When setup finishes, texe prints the exact paths to the source and PDF, for
example:

```text
Source  my-paper/main.tex
PDF     my-paper/main.pdf
```

Edit `main.tex` to write the paper. If a build fails, the previous successful
PDF is kept.

## Keep writing

If you chose VS Code during setup, texe opens the source and PDF side by side.
Saving `main.tex` rebuilds and refreshes the PDF. texe installs the required
extensions when they are missing, but does not force-update an existing LaTeX
Workshop installation.

Without VS Code, keep a local browser viewer open with:

```sh
texe watch --view --project my-paper
```

The viewer is available only on your computer. It refreshes after stable saves
and keeps the current page, zoom, and scroll position.

Experienced users can create the same starter without prompts:

```sh
texe init my-paper --yes \
  --template basic \
  --title "My Paper" \
  --author "Ada Researcher" \
  --git --vscode
texe build --project my-paper --yes
```

## How texe works

The release contains three compatible commands. Most users interact only with
`texe`; it calls `pqty` and `pqty-fls` to find and prepare the exact TeX Live
packages used by the paper.

```text
paper source
     |
     v
   texe  ----> managed LaTeX engine
     |
     `-------> pqty package environment
     |
     v
PDF + texe.lock
```

The managed setup supports pdfLaTeX and LuaLaTeX on Linux x86-64, Windows
x86-64, and macOS Apple Silicon. Guided setup uses pdfLaTeX. Advanced users
can select LuaLaTeX through `texe init --engine lualatex` or `texe.toml`. See
the [support matrix](docs/support.md) if you are unsure whether your computer
is supported.

On a managed build, texe:

- downloads and verifies the selected LaTeX engine;
- resolves only the TeX Live packages the paper needs;
- runs BibTeX, Biber, MakeIndex, and glossaries when needed;
- repeats LaTeX passes until references and auxiliary files settle;
- publishes the PDF and SyncTeX file beside the source;
- keeps other generated files below the project's `.texe/` directory.

Managed tools, package downloads, and the shared package store live below
`TEXE_HOME`. They never modify an operating system TeX installation.

## Optional project configuration

The guided setup writes `texe.toml`; a first paper does not need any manual
configuration:

```toml
schema = "texe.project/v1"

[project]
entry = "main.tex"

[toolchain]
engine = "pdflatex"
```

The [configuration guide](docs/configuration.md) explains engine selection,
additional input folders, generated files, package storage, command overrides,
and shell escape. The
[project schema](schemas/texe.project.schema.json) is the complete field
reference.

Commit both `texe.toml` and `texe.lock`. The lock records the selected LaTeX
runtime, packages, integrity information, and build timestamp so later builds
can reproduce the same environment.

## Useful commands

```sh
texe                              # guided setup or project menu
texe init                         # create or adopt a project
texe build                        # update the lock and build
texe watch --view                 # rebuild and show the PDF in a local viewer
texe doctor                       # check the project and installed tools
texe storage                      # show project and shared storage
texe clean --dry-run              # show what generated state would be removed
texe clean                        # remove generated project state
```

Run `texe <command> --help` for every option. Common advanced build options are:

```sh
texe build --frozen               # require the existing lock
texe build --offline              # forbid network access
texe build --force                # build even when nothing changed
```

Use `--json` for versioned machine-readable results, `--quiet` to suppress
successful presentation, and `--verbose` for a detailed transcript. The
[machine-readable output guide](docs/machine-readable-output.md) lists every
v1 protocol and its JSON Schema.

## Getting help

Start with the built-in health check from the project folder:

```sh
texe doctor
texe doctor --verbose
```

The [installation troubleshooting guide](docs/install.md#troubleshooting)
covers common setup problems. If the problem remains, open a
[bug report](https://github.com/backmatter/texe/issues/new?template=bug_report.yml)
with the output of `texe --version`, the platform, and the smallest safe
reproduction. Remove confidential paper content, credentials, and private
filesystem paths before posting.

Report vulnerabilities privately by following [SECURITY.md](SECURITY.md).

## Reproducibility, privacy, and trust

A locked managed project with shell escape disabled is designed to rebuild the
same PDF bytes from empty caches on each supported target. Cross-target byte
identity is not yet a compatibility guarantee.

texe isolates TeX, Lua, font, and command lookup; pins build timestamps;
verifies downloaded content; and publishes outputs only after a successful
build. Source, bibliography data, logs, and PDFs stay local. See
[privacy and network behavior](docs/privacy.md) for the exact connections texe
can make.

The system provider, shell escape, and explicitly enabled unmanaged command
overrides allow host or project software to affect the build. texe cannot
promise full reproducibility for those modes.

## Development

The supported Rust toolchain is declared in `Cargo.toml` and pinned by
`rust-toolchain.toml`. The Rust library is an implementation API and may
change between releases. The versioned CLI and JSON schemas are the
compatibility boundary.

See [CONTRIBUTING.md](CONTRIBUTING.md) for fresh-clone setup, test tiers, and
pull-request expectations.

Maintainer references:

- [Architecture](docs/architecture.md)
- [Managed toolchain recipes](docs/toolchain-recipes.md)
- [Release runbook](docs/releasing.md)

Machine-facing contracts are defined by the schemas for
[`texe.lock/v1`](schemas/texe.lock.schema.json),
[`texe.error/v1`](schemas/texe.error.schema.json), and
[`texe.watch-event/v1`](schemas/texe.watch-event.schema.json).

## License

texe is available under the [MIT License](LICENSE).
