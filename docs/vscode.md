# Move an existing paper to texe + VS Code

texe manages the build. VS Code and LaTeX Workshop supply editing, completion,
outline navigation, PDF viewing and SyncTeX. Your `.tex`, `.bib`, custom classes,
images and TeXstudio files stay where they are.

## Start with the compatibility check

```sh
texe adopt /path/to/paper --check
texe adopt /path/to/paper
```

The check writes nothing and downloads nothing. It reports the entry, engine,
local classes, editor conflicts and signs of external processing. Multiple root
files require `--entry thesis.tex`; texe does not guess even with `--yes`.
`% !TeX program = ...` supplies the engine hint. Without a hint, literal
`fontspec` or `unicode-math` declarations select managed LuaLaTeX. An explicit
incompatible pdfLaTeX choice is reported before setup writes anything. XeLaTeX uses an installed system
TeX distribution; managed pdfLaTeX and LuaLaTeX can download their runtimes.

The static scan is a preflight, not proof that arbitrary TeX will compile. It
follows literal local includes and class/package declarations (up to 256 files
and 8 MiB), ignoring comments and common verbatim forms. Dynamic TeX, fonts and
custom build scripts still need the first build. `--check` prints readable
guidance; add `--json` for the machine-readable report. Existing
`.latexmkrc` and Makefile commands are not executed or imported. External
processing must be configured in `texe.toml` before adopting that workflow.

Use `--no-build` to defer the first build, or `--no-editor` to create only the
manifest. `texe init --entry thesis.tex --yes` can create a manifest around an
existing source for advanced configuration; it preserves the source. An existing
`texe.toml` remains authoritative and is never overwritten by adoption.

## Review editor settings

texe preserves JSONC comments and unrelated settings, including nested file
associations and exclusions. When existing build settings conflict, review them:

```sh
texe editor --preview
texe editor --replace-conflicts
```

For an unadopted folder, interactive `texe adopt` shows the proposed settings
and asks whether to apply them. After reviewing `texe adopt --check`, use
`texe adopt --replace-conflicts` to accept those integration changes directly.
`--no-editor` keeps your editor configuration.
`--replace-conflicts` accepts only texe's integration changes. It does not replace
the entire file. `texe editor --configure-only` updates settings without opening
VS Code or installing extensions.

`texe editor --remove` restores texe-owned values that still match what texe
installed, preserving later edits. Keep `.vscode/texe-integration.json` until you
remove the integration: it contains the original values required for undo. If
nothing was edited after setup, removal restores the original settings bytes.
Old settings created before ownership tracking cannot be automatically undone.

If the first build fails, adoption still installs and opens the editor. Its
failure is handed to the first editor session so Problems and the retained-PDF
badge show what needs fixing. Fix the source and save to retry.

## Work in VS Code

Trust the paper folder, then use the Command Palette:

| Action | Command |
| --- | --- |
| First-use checklist | **texe: Writing Guide** |
| Wrap source, hide generated files and make room for the PDF | **texe: Use Writing Layout** |
| Restore settings changed by Writing Layout | **texe: Restore Previous Writing Settings** |
| Locate the cursor in the PDF | **texe: Source to PDF** |
| Save, build and open source/PDF | **texe: Build and View** |
| Build without changing layout | **texe: Build Paper**, or **Tasks: Run Build Task → texe** |
| Restore the side-by-side view | **texe: Open Paper Side by Side** |
| Find a source error | **texe: Show Problems**, then click the error |
| Inspect engine output | **texe: Open Build Log** or **texe: Show Output** |
| Check installed tools and caches | **texe: Check Setup** (offline) |
| Use a different texe installation | **texe: Choose Executable** |
| Re-enable downloads after choosing offline builds | **texe: Allow Build Downloads** |
| Use F5 for Build and View, F6 for Build | **texe: Enable Familiar Shortcuts** |

Builds on save, tasks, and LaTeX Workshop’s Build button share the companion’s
save/build queue and download policy. LaTeX Workshop uses a project-local,
authenticated loopback bridge to reach that queue; open the trusted project
in VS Code before using its build command. LaTeX Workshop's own
auto-build is disabled to avoid duplicate builds. Set `texe.buildOnSave` to false
for manual builds. Build commands save dirty documents in the selected paper
folder first. In a multi-root workspace, commands select the active file's folder.
The status bar shows the current build phase and elapsed time; click it during
a build to cancel. Cancellation has a neutral status and does not create a new
error toast or PDF error badge. Saving during a build queues the latest changes;
success and recovery notices appear only after that queued build completes.

Editor context is reused between builds and refreshed when the manifest,
executable, or workspace changes.

Replacing or deleting an external figure or data file also triggers a rebuild.
Published PDFs, SyncTeX, locks and private state are excluded to avoid loops.

Problems is replaced by each completed build, so an error fixed in one chapter
does not linger when another chapter fails. Warnings link to the matching build
log line; clicking the warning status opens Problems.

The status bar reports failures; a failed build keeps the previous
PDF, marks its tab with an error badge, and shows an error toast that explicitly
says it is the previous successful build. **Show Error** jumps to the source;
**Open Build Log** opens the log from the failed pass. Identical consecutive
failures are not repeated as new toasts, and notifications never block the next
build. A successful repair clears texe's diagnostics and the PDF badge.

Builds automatically download missing TeX tools and packages. You do not need
an existing TeX installation for managed builds. Set `texe.allowDownloads` to
false to explicitly use offline texe builds; those require cached dependencies.
To resume automatic downloads, use **texe: Allow Build Downloads** and rebuild.
Check Setup remains an offline diagnostic and does not initiate downloads.

The generated executable path works when VS Code is launched from the desktop.
If that absolute path belongs to another machine, the companion tries `texe` on
PATH. Choose Executable updates the folder setting; rerun `texe editor` after
moving an installation to refresh the LaTeX Workshop external-build command.
For Remote SSH, WSL or containers, install texe where the extension host runs.

Saving `texe.toml` refreshes source and PDF paths through texe's manifest parser.
Settings that you changed manually are treated as conflicts, not overwritten.

## TeXstudio habits

Use LaTeX Workshop's **SyncTeX from cursor** for source → PDF. Ctrl+click in its
PDF viewer performs inverse search (Cmd+click on macOS). The optional F5/F6
bindings apply only to LaTeX editors in configured texe folders; they do not
change your global VS Code keybindings.

TeXstudio macros, custom menus, dictionaries and user commands are not imported.
Keep using the original folder while you reproduce those personal conveniences
with VS Code snippets, shortcuts and extensions. Before switching your daily
workflow, build your paper once and check its references, figures and fonts.

Writing Layout is opt-in and saves previous folder settings in VS Code workspace
state. Restore Previous Writing Settings restores values that still match what
it applied, preserving later manual edits. Use it before removing the companion
if you also want to undo the optional layout settings.
