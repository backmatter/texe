# VS Code

texe installs **Citet** (`citet.citet`), a single native VS Code extension maintained
in the Citet repository at `apps/vscode`, beside the Neovim integration. It runs
texe builds, hosts the tex-ls language server, and owns the PDF.js viewer and
SyncTeX navigation. LaTeX Workshop and the separate tex-ls extension are not
required. The texe CLI remains usable on its own.

Open a paper with:

```bash
texe editor
```

The project folder opens directly. Trust the folder to enable local tools. If a
PDF exists, Citet opens it beside the source. New folders can use **Citet:
Configure Local Paper**, or **Citet: Build and View**, to adopt an existing paper
and select its main TeX file.

## Local workflow

Local editing and builds require no account or Citet server. Citet uses
`texe.toml` for the project and the versioned `texe editor --inspect --json` and
`texe build --json` protocols for paths, package roots, build progress and errors.
It does not check for an exact texe or tex-ls version.

- **Citet: Build and View** (`Ctrl+Alt+B`, `Cmd+Alt+B` on macOS) builds and opens the PDF.
- **Citet: Build Paper** builds without opening another tab.
- Saves build configured papers when `citet.buildOnSave` is enabled.
- The **Citet: Build paper** task uses the same build queue.
- **Citet: Cancel Local Build / Stop Waiting for Remote Build** stops the current local process tree and clears queued builds.
- **Citet: Open Build Log** and **Show Build Output** expose diagnostics and progress.

Each project has one build queue. Saves during a build request one subsequent
build with the latest saved sources. Errors appear in Problems, and failed
builds preserve the last successful PDF and mark it out of date. PDF refresh
preserves zoom, scroll position and search. The native viewer also opens existing
PDF files without adopting their parent folder.

Use **Citet: Sync PDF to Cursor** (`Ctrl+Alt+J`) for source → PDF. Ctrl/Cmd+click
or double-click in the PDF jumps back to source. Local SyncTeX uses a bundled
MIT-licensed reader; it does not require Workshop or a host `synctex` executable.
Rebuild changed sources before navigating an out-of-date PDF.

tex-ls supplies completion, hover, navigation, symbols, formatting, code actions
and linting through native VS Code providers. Citet is the default formatter for
LaTeX and BibTeX. Compiler diagnostics come from texe. Project and runtime TEXMF
roots are read from texe's editor context, while `tex-ls.toml` provides reversible
project root and auxiliary-directory settings.

`citet.texePath` and `citet.texLsPath` select executables without shell argument
splitting. Otherwise the extension uses an installed tool or a verified GitHub
release in its own user storage. `citet.allowDownloads: false` disables missing
tool downloads and makes texe builds offline. Cached tools, runtime and packages
continue to work. Versions can change independently; unsupported JSON schemas
produce an actionable error.

## Optional Citet collaboration

Use **Citet: Login** and **Connect Folder to Citet** to connect a local folder to
an existing remote project. Existing remote files must match their local copies;
conflicts are reported before binding or overwriting. Alternatively, **Open
Project Locally** materializes a remote project into a folder. The binding lives
in `.citet/config.json` alongside the existing collaboration cache.

Connected projects still build locally by default. **Citet: Choose Build Backend**
sets `citet.buildBackend` to `local` or `remote` for the folder. There is no silent
fallback. Remote builds wait for attached document commits before queueing a
server build and only accept the requested job's successful artifact. If another
server build supersedes the request, Citet reports that instead of showing it as
the requested result. Stopping a remote wait does not cancel the server job.

The same build, PDF and navigation commands work with either backend. Switching
backends gives each artifact its own viewer tab. Remote citations, collaboration
and Presenter remain available for connected projects. Local bibliography
completion and insertion use the language server. Generated `.texe` files and the
local build PDF are excluded from the collaboration mirror.

## Reversible setup

```bash
texe editor --preview
texe editor --replace-conflicts
texe editor --configure-only
texe editor --remove
```

Settings merge with JSONC comments and unrelated settings preserved. Setup
records its edits, including `tex-ls.toml`; removal restores values still owned
by texe and preserves later user edits. Rerunning setup retires old texe-owned
Workshop/companion settings and replaces the old companion with Citet. Existing
third-party extensions are not globally uninstalled.

The bundled Citet VSIX is built separately from texe and carries its own version.
See [the bundle provenance](../assets/citet-vscode.md) for rebuilding it. No Citet
checkout, Node, Bun or server is required to install it from the texe CLI.
