# texe for VS Code

Build and view a texe paper with LaTeX Workshop. The companion provides queued
builds on save, a build status indicator, clickable Problems diagnostics, setup
checks and source/PDF layout. The texe CLI remains responsible for the build.

Run **texe: Build and View**, **texe: Check Setup**, or **texe: Show Output** from
the Command Palette. **texe: Enable Familiar Shortcuts** enables F5 for Build and
View and F6 for Build in this folder. tex-ls provides completion and outline
navigation. LaTeX Workshop provides PDF refresh and forward/inverse SyncTeX.

Builds automatically download missing TeX tools and packages; no existing TeX
installation is needed. Set `texe.allowDownloads` to false for offline texe
builds using cached dependencies. **texe: Allow Build Downloads** re-enables
downloads for the folder. A failed build retains the last PDF and reports that
it is showing the previous output.

Run `texe adopt --check` before adopting an existing paper. `texe editor` installs
this companion and configures the folder. **texe: Choose Executable** repairs a
missing CLI path. The companion runs only in trusted workspaces and collects no
telemetry.

[VS Code setup and settings removal](https://github.com/backmatter/texe/blob/main/docs/vscode.md)

Language features require **tex-ls 0.1.2 or newer**. `texe editor` installs it when
missing and configures it for the project's packages and build outputs. LaTeX
Workshop remains the PDF viewer and SyncTeX provider. **texe: Check Setup** reports
whether tex-ls needs installing or updating. Formatting uses tex-ls; formatting on
save is optional.
