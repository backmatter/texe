# VS Code

texe installs **Citet** (`backmatter.citet`) from the VS Code Marketplace.
VS Code installs its standalone `backmatter.tex-ls` dependency for language
features. Citet owns texe builds, the PDF.js viewer and SyncTeX navigation.
VS Code manages editor updates independently of the native command suite.
The texe CLI remains usable on its own.

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
and linting through native VS Code providers. The standalone tex-ls extension
is the default formatter for LaTeX and BibTeX. Compiler diagnostics come from texe. Project and runtime TEXMF
roots are read from texe's editor context, while `tex-ls.toml` provides reversible
project root and auxiliary-directory settings.

`citet.texePath` selects the native build executable. Otherwise Citet uses texe
on PATH or its own checksum-verified compatible texe suite. Managed online builds
check for tool updates at most once per day, without an update timer or a startup
check. `citet.allowDownloads: false` disables checks and downloads and makes
local builds offline. Cached tools, runtimes and packages continue to work.
Use **Citet: Update Local Build Tools** to check immediately. PATH and configured
installations remain managed by their installer or package manager.

Use `tex-ls.*` settings and commands for language features. VS Code updates that
extension independently of texe and Citet.

## Optional Citet collaboration

Use **Citet: Login** and **Connect Folder to Citet** to connect a local folder to
an existing remote project. Existing remote files must match their local copies;
conflicts are reported before binding or overwriting. Alternatively, **Open
Project Locally** materializes a remote project into a folder. The binding lives
in `.citet/config.json` alongside the existing collaboration cache.

Connected projects use protected local builds on Linux x64 by default and hosted
builds on other hosts. **Citet: Choose Build Backend**
sets `citet.buildBackend` to `local` or `remote` for the folder. There is no silent
fallback. Remote builds wait for attached document commits before queueing a
server build and only accept the requested job's successful artifact. If another
server build supersedes the request, Citet reports that instead of showing it as
the requested result. Cancelling your online build aborts its worker request and
prevents it from publishing success, while retaining the last successful PDF.
A collaborator's separate build continues.

Connected papers automatically prepare a verified package, font and runtime
environment while online. Local builds retain the chosen engine and work
offline once prepared. Adding a missing dependency while online refreshes the
environment and retries the build. Protected local compilation currently
requires Linux x64 with Landlock V3; unsupported platforms stop before running
the source. Standalone managed adoption supports pdfLaTeX, XeLaTeX and LuaLaTeX;
prepared Citet environments also support Shipout and Tectonic. Shipout Biber
papers require another engine.

Protected builds bound the process group's resident memory to 2 GiB and private
compiler output to 4 GiB and 100,000 entries. The output budget also adapts to
available disk space, preserving a 512 MiB reserve; existing writable caches
count toward the aggregate cap. Checks run before launch, after exit, and about
every 100 ms while compiling, including open deleted temporary files. This is a
sampled watchdog rather than a filesystem quota, so writes during a check can
overshoot the threshold. A stopped build keeps saved sources and the last PDF;
`texe clean` removes derived build files if a disk warning requires space.

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

Each `texe editor` setup asks VS Code to install or update `backmatter.citet`
from the Marketplace. No extension version is pinned or bundled. VS Code installs
Citet's dependencies from the store and manages their updates. Installing a missing
extension or updating it requires network access; an already installed extension
remains usable offline, and texe reports a store failure without blocking the CLI.
