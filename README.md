# texe

texe creates and builds LaTeX papers. It downloads the tools and packages each
project needs, so you do not have to install TeX Live.

## Get started

Install texe on macOS or Linux:

```sh
curl -LsSf https://github.com/backmatter/texe/releases/latest/download/install.sh | bash
```

On Windows, run this in PowerShell:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://github.com/backmatter/texe/releases/latest/download/install.ps1 | iex"
```

Other options, including the Debian package and offline installs, are in the
[install guide](docs/install.md). Then run texe in the folder where you keep
your papers:

```sh
texe
```

Answer the setup prompts and edit `main.tex`. The first build downloads what the
paper needs; later builds reuse it.

For a paper you already have, check it before adopting it. Your sources are
preserved either way:

```sh
texe adopt /path/to/paper --check   # report problems, change nothing
texe adopt /path/to/paper
```

## Build and preview

```sh
texe build
texe watch --view      # rebuild on save, refresh a browser preview
```

A failed build keeps the last successful PDF. For editing, PDF viewing, and
rebuilds on save in VS Code, see the [VS Code guide](docs/vscode.md).

## Everyday commands

```sh
texe build --frozen    # build using the existing lock
texe build --offline   # build without downloads; requires cached dependencies
texe build --force     # rebuild even when inputs are unchanged
texe doctor            # check the project and tools
texe storage           # show project and shared storage
texe clean --dry-run   # preview removal of generated files
texe clean             # remove generated project state
```

Add `--help` to any command for its options, `--quiet` to suppress progress and
success output, or `--verbose` for detail.

## Project files

Setup writes `texe.toml`. Commit it together with `texe.lock` to pin both the
configuration and the exact tools and packages. `.texe/` holds generated files;
do not commit it.

Managed builds run pdfLaTeX or LuaLaTeX on Linux x86-64, Windows x86-64, and
Apple Silicon macOS.

## Documentation

- [Install](docs/install.md) — installing, upgrading, troubleshooting
- [VS Code](docs/vscode.md) — adopting a paper, editing, preview, build on save
- [Configuration](docs/configuration.md) — engines, input folders, bibliography,
  shell escape
- [JSON output](docs/json-output.md) — scripting against texe
- [Privacy](docs/privacy.md) — what texe downloads, what stays on your computer

## Help

Report bugs in the [issue tracker](https://github.com/backmatter/texe/issues)
and vulnerabilities through [SECURITY.md](SECURITY.md). To work on texe itself,
see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE).
