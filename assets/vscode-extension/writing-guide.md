# Your writing workflow

Edit your source and save. texe installs missing TeX tools and packages,
compiles the paper, and refreshes the PDF.

| Action | Where to find it |
| --- | --- |
| Build and view | Play button above the source, or F5 after enabling familiar shortcuts |
| Build | F6 after enabling familiar shortcuts, or LaTeX Workshop's Build button |
| Locate the cursor in the PDF | **texe: Source to PDF** |
| Return to source | Ctrl+click the PDF; Cmd+click on macOS |
| Fix a source error | Click its row in **Problems** |
| Inspect a warning | Click the warning count to open **Problems**, then its build log row |

A failed build keeps the last successful PDF and marks it as previous output.
Fix the source and save to retry. Missing dependencies download automatically.

**Use Writing Layout** wraps long source lines, hides generated files, opens
the outline and makes room for the PDF. **Restore Previous Writing Settings**
undoes its folder settings while preserving later manual changes.
