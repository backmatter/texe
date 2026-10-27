# Citet integration test fixture

`citet-vscode.vsix` is Citet 0.1.2, built from `citet/apps/vscode` for the
real VS Code workflow tests. It is not embedded in texe or included in the
published crate. `texe editor` installs `backmatter.citet` from the Marketplace;
VS Code installs its standalone tex-ls dependency.

- SHA-256: `0b92f557b91dc685ff4813c628aad077cac46d20e31c51f95faab4cfe522a6db`
- Build: `bun run --cwd apps/vscode export:texe` from the Citet repository.
- This fixture includes the Linux sync binary. The local-only editor tests do
  not start the sync daemon. Platform release packaging belongs to Citet's CI.

Keep this fixture unchanged for native updates that do not change the editor
contract. Refresh it when updating the editor integration tests. Citet releases
remain independent of texe releases.
