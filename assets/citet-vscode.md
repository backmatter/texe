# Bundled Citet extension

`citet-vscode.vsix` is the self-contained Citet VS Code extension, built from
`citet/apps/vscode`. Its source remains beside `citet/apps/nvim`. texe embeds this
artifact so released command suites do not need a sibling checkout, JavaScript
build tools, a marketplace connection or a Citet server to install the extension.

From the Citet repository, with texe as a sibling:

```bash
bun install
bun run --cwd apps/vscode export:texe
```

For another destination:

```bash
bun run --cwd apps/vscode package -- --output /path/to/citet.vsix
```

The exporter bundles runtime dependencies, PDF.js and its matching worker,
language grammars/configuration, and third-party license notices. It removes
workspace dependency metadata from the installed manifest and uses deterministic
ZIP timestamps. Native tools are resolved independently at runtime from settings,
PATH or checksum-verified official releases.

The VSIX has the extension identity `citet.citet` and its own version. Rust checks
all installed bundle contents, ignoring VS Code's added package metadata, and
writes content-addressed installation artifacts. Update this file together with
the extension whenever the Citet source changes; Rust-only texe builds never
regenerate it implicitly.
