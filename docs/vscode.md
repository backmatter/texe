# VS Code

texe manages the build. VS Code and LaTeX Workshop supply editing, completion,
outline navigation, PDF viewing, and SyncTeX. Your `.tex`, `.bib`, custom
classes, images, and TeXstudio files stay where they are.

## Set up an existing paper

```sh
texe adopt /path/to/paper --check
texe adopt /path/to/paper
```

The check writes nothing and downloads nothing. It reports the entry file,
engine, local classes, editor setting conflicts, and signs of external
processing. Add `--json` for the machine-readable report.

Useful flags:

- `--entry thesis.tex` — required when the paper has several root files. texe
  does not guess, even with `--yes`.
- `--no-build` — defer the first build.
- `--no-editor` — write only `texe.toml` and keep your editor configuration.
- `--replace-conflicts` — accept the editor changes the check listed.

An existing `texe.toml` is authoritative and is never overwritten.

The scan is a preflight, not proof that the paper compiles: dynamic TeX, fonts,
and custom build scripts are only settled by the first build. Existing
`.latexmkrc` and Makefile commands are neither run nor imported, so configure
external processing in `texe.toml` before adopting that workflow.

If the first build fails, adoption still opens the editor and hands the failure
to it. Fix the source and save to retry.

### Engine detection

- `% !TeX program = ...` in the source selects the engine.
- Without that hint, a literal `fontspec` or `unicode-math` declaration selects
  managed LuaLaTeX.
- An explicit pdfLaTeX choice that cannot work is reported before anything is
  written.
- Managed pdfLaTeX and LuaLaTeX download their own runtimes. XeLaTeX needs an
  installed system TeX distribution.

## Editor settings

texe writes only its own keys, preserving JSONC comments and unrelated settings.

```sh
texe editor --preview             # show the merged settings and any conflicts
texe editor --replace-conflicts   # accept texe's integration values
texe editor --configure-only      # write settings only, no extensions, no launch
texe editor --remove              # undo texe's settings
```

Interactive `texe adopt` shows the proposed settings and asks before applying
them. `--replace-conflicts` replaces only the conflicting texe keys, never the
whole file, and settings you later change by hand count as conflicts rather than
being overwritten. `--remove` restores texe-owned values that still match what
texe wrote, so your edits survive; keep `.vscode/texe-integration.json` until
then, since it holds the values needed to undo.

## Everyday commands

Trust the paper folder, then use the Command Palette:

| Action | Command |
| --- | --- |
| First-use checklist | **texe: Writing Guide** |
| Save, build, and open source and PDF | **texe: Build and View** |
| Build without changing the layout | **texe: Build Paper** |
| Restore the side-by-side view | **texe: Open Paper Side by Side** |
| Locate the cursor in the PDF | **texe: Source to PDF** |
| Find a source error | **texe: Show Problems**, then click the error |
| Inspect engine output | **texe: Open Build Log** or **texe: Show Output** |
| Wrap source and make room for the PDF | **texe: Use Writing Layout** |
| Undo Writing Layout | **texe: Restore Previous Writing Settings** |
| Check installed tools and caches, offline | **texe: Check Setup** |
| Use a different texe installation | **texe: Choose Executable** |
| Re-enable downloads after offline builds | **texe: Allow Build Downloads** |
| Use F5 to build and view, F6 to build | **texe: Enable Familiar Shortcuts** |

Writing Layout is opt-in and saves the previous folder settings in VS Code
workspace state. Restoring applies values that still match what it wrote. Use it
before removing the companion if you also want the layout settings back.

## Builds

Builds on save, the build task, and LaTeX Workshop's Build button share one
build queue and one download policy. LaTeX Workshop's own auto-build is disabled
so nothing builds twice.

- Set `texe.buildOnSave` to false for manual builds only.
- Build commands first save unsaved files in the selected paper folder. In a
  multi-root workspace, the active file selects the folder.
- The status bar shows the build phase and elapsed time. Click it during a build
  to cancel; cancelling is neutral and creates no error toast or PDF badge.
- Saving during a build queues the newest changes. Success and recovery notices
  appear once that queued build finishes.
- Replacing or deleting an external figure or data file also triggers a rebuild.
  Published PDFs, SyncTeX files, locks, and private state are excluded so builds
  do not loop.
- Saving `texe.toml` refreshes the source and PDF paths.

Open the trusted project in VS Code before using LaTeX Workshop's build command.

## Errors and warnings

A failed build keeps the previous PDF, marks its tab with an error badge, and
shows a toast saying the PDF is from the last successful build. **Show Error**
jumps to the source; **Open Build Log** opens the log from the failed pass. A
successful repair clears the diagnostics and the badge.

Each completed build replaces Problems, so an error fixed in one chapter does
not linger while another fails. Warnings link to the matching build log line,
and clicking the warning status opens Problems. Repeated identical failures do
not produce new toasts, and notifications never block the next build.

## Downloads

Builds download missing TeX tools and packages automatically; managed builds
need no existing TeX installation. Set `texe.allowDownloads` to false for
offline builds, which then require cached dependencies — **texe: Allow Build
Downloads** and a rebuild resume downloading. Check Setup stays offline and
never downloads.

## Finding the texe executable

The generated path works when VS Code is launched from the desktop; if it
belongs to another machine, the companion falls back to `texe` on `PATH`.
**Choose Executable** updates the folder setting, and rerunning `texe editor`
after moving an installation refreshes LaTeX Workshop's external build command.
For Remote SSH, WSL, and containers, install texe where the extension host runs.

## Coming from TeXstudio

Use LaTeX Workshop's **SyncTeX from cursor** for source → PDF, and Ctrl+click in
its PDF viewer for inverse search (Cmd+click on macOS). The optional F5 and F6
bindings apply only to LaTeX editors in configured texe folders.

TeXstudio macros, custom menus, dictionaries, and user commands are not
imported. Keep using the original folder while you rebuild those conveniences
from VS Code snippets, shortcuts, and extensions. Before switching your daily
workflow, build the paper once and check its references, figures, and fonts.
