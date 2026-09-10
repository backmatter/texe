# texe

texe is a command-line tool for creating and building LaTeX papers. It downloads
the LaTeX tools and packages a project needs and produces the PDF. A separate
TeX Live installation is not required.

## Create your first paper

[Install the CLI](docs/install.md), open Terminal or PowerShell, move into the
folder where you want to keep your papers, and run:

```sh
texe
```

The guided setup creates a basic paper with pdfLaTeX and builds the first PDF.
Git and VS Code are optional. The first build downloads the required tools and
packages; later builds reuse them.

texe prints the source and PDF paths. Edit `main.tex` to write the paper.
If a build fails, the previous successful PDF is kept.

## Keep writing

For an existing paper (including a TeXstudio project):

```sh
texe adopt /path/to/paper --check
texe adopt /path/to/paper
```

The preflight reads your project before setup writes anything. Existing sources
stay in place. See [moving to VS Code](docs/vscode.md) for compatibility checks,
editor conflicts, and familiar shortcuts.

If you chose VS Code during setup, texe opens the source and PDF side by side.
Saving `main.tex` rebuilds and refreshes the PDF. texe installs the required
extensions when they are missing, but does not force-update an existing LaTeX
Workshop installation.

Without VS Code, keep a local browser viewer open with:

```sh
texe watch --view --project my-paper
```

The viewer is available only on your computer. It refreshes after stable saves
and keeps the current page, zoom, and scroll position. A viewer status banner
shows building, failure and disconnection so an older PDF cannot look current. Watch mode waits for
250 ms without input changes before rebuilding. For editors or generators
that save in several steps, increase the quiet period:

```sh
texe watch --view --debounce-ms 1000 --project my-paper
```

`--poll-ms` controls how often inputs are checked (default: 250 ms). Both
intervals accept 50–60,000 ms; changes are detected on polling ticks.

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

The release bundles `texe`, `pqty`, and `pqty-fls`. texe manages the engine and
build passes; [pqty](https://github.com/backmatter/pqty) resolves, locks, and
installs the TeX Live packages. pqty-fls converts engine recorder output into
package traces.

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

Run `texe <command> --help` for options:

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

Run `texe doctor` to check the project and tools; add `--verbose` for details.
See [troubleshooting](docs/install.md#troubleshooting) or open a
[bug report](https://github.com/backmatter/texe/issues/new?template=bug_report.yml)
with the version, platform, command, and a minimal reproduction. Remove private
content and credentials. Report vulnerabilities through [SECURITY.md](SECURITY.md).

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

## License

texe is available under the [MIT License](LICENSE).
