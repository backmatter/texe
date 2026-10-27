# Citet integration test fixture

`citet-vscode.vsix` is Citet 0.1.2, built from `citet/apps/vscode` for the
real VS Code workflow tests. It is not embedded in texe or included in the
published crate. `texe editor` installs `backmatter.citet` from the Marketplace;
VS Code installs its standalone tex-ls dependency.

- SHA-256: `3c58e3ce59eefd1c66027e6f758e7ceab19853e9e4184ef6544c7a1989bbf64c`
- Build: `bun run --cwd apps/vscode export:texe` from the Citet repository.
- This fixture includes the Linux sync binary. The local-only editor tests do
  not start the sync daemon. Platform release packaging belongs to Citet's CI.

Keep this fixture unchanged for native updates that do not change the editor
contract. Refresh it when updating the editor integration tests. Citet releases
remain independent of texe releases.
