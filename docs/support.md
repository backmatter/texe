# Supported computers and tools

texe supports recent 64-bit Intel/AMD Linux and Windows computers, plus Apple
Silicon Macs.

## Computers

| Computer | How to check | Recommended installation |
| --- | --- | --- |
| Linux x86-64 | `uname -m` prints `x86_64` | Debian package on Debian/Ubuntu; portable installer elsewhere |
| Windows x86-64 | Settings → System → About says x64-based processor | WinGet or direct PowerShell installer |
| macOS Apple Silicon | About This Mac shows an Apple M-series chip | Homebrew or one-off installer |

The managed pdfLaTeX and LuaLaTeX engines are available on all three supported
computers.

texe does not currently support:

- Intel Macs;
- ARM Linux computers;
- Windows on ARM, including Snapdragon-based Windows computers;
- iPhone or iPad.

An advanced user may run texe's `system` provider on another computer when a
compatible TeX installation is already present, but that is not a first-class
or reproducible configuration.

## Using another LaTeX engine

Guided setup uses **pdfLaTeX** automatically. It has the smallest first
download and is broadly compatible with journal templates.

Advanced users can choose **LuaLaTeX** for documents that need modern Unicode
or OpenType font workflows. Its initial setup is larger. Run
`texe init --engine lualatex` or change `toolchain.engine` in `texe.toml`
before building.

Managed XeLaTeX is not currently included. Advanced users can select
`provider = "system"` with an existing XeLaTeX installation.

## Editors and PDF viewing

VS Code with LaTeX Workshop is the first supported editor integration. Setup is
optional. It creates `.vscode/settings.json` when absent and asks before
replacing an existing file. Settings are never merged. VS Code opens the
project folder directly.

Older texe versions created a workspace below `.texe/`. It can be removed
without changing project settings with:

```sh
texe editor --remove
```

You do not need VS Code. From the project folder, this command rebuilds after
saves and opens a local PDF viewer in the browser:

```sh
texe watch --view
```

The generated PDF remains an ordinary file that can be opened in any PDF
application.

## Advanced trust boundaries

Shell escape is disabled by default because it allows a document to run host
commands. Enabling it explicitly ends the full reproducibility guarantee.

Managed command overrides are also rejected by default. Enabling
`allow_unmanaged_commands` permits project or host executables, shows a warning,
and disables the no-op build cache.

Managed builds do not silently substitute proprietary or host-only fonts. See
the [configuration guide](configuration.md) before changing these boundaries.
