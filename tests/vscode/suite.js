const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vscode = require("vscode");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(condition, message, timeout = 60000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { const value = await condition(); if (value) return value; await delay(100); }
  throw new Error(message);
}
exports.run = async () => {
  const root = vscode.workspace.workspaceFolders[0].uri.fsPath;
  const source = vscode.Uri.file(path.join(root, "paper.v2.tex"));
  const pdf = path.join(root, "paper.v2.pdf");
  const document = await vscode.workspace.openTextDocument(source);
  const editor = await vscode.window.showTextDocument(document, vscode.ViewColumn.One);
  const extension = vscode.extensions.getExtension("backmatter.citet");
  assert.ok(extension, "unified extension is available");
  assert.equal(vscode.extensions.getExtension("James-Yu.latex-workshop"), undefined);
  assert.ok(vscode.extensions.getExtension("backmatter.tex-ls"));
  const api = await extension.activate();
  assert.ok(["auto", "local"].includes(vscode.workspace.getConfiguration("citet", source).get("buildBackend")));
  if (process.platform !== "linux") {
    const before = document.getText(), messages = [];
    const original = vscode.window.showErrorMessage;
    vscode.window.showErrorMessage = async message => { messages.push(message); return undefined; };
    try { await vscode.commands.executeCommand("citet.buildAndView"); }
    finally { vscode.window.showErrorMessage = original; }
    assert.ok(messages.some(message => /protected local builds.*Linux/i.test(message)), messages.join("\n"));
    assert.equal(document.getText(), before, "unsupported local builds preserve the source");
    assert.equal(fs.existsSync(pdf), false, "unsupported local builds do not publish a PDF");
    await until(async () => Array.isArray(await vscode.commands.executeCommand("vscode.executeFormatDocumentProvider", source, {tabSize: 2, insertSpaces: true})), "standalone tex-ls formatter did not start");
    fs.writeFileSync(path.join(process.env.TEXE_TEST_ROOT, "results.json"), JSON.stringify({extension: extension.id, formatting: true, localBuild: false, unsupportedLocalBuildRejected: true, sourcePreserved: true}, null, 2));
    console.log("PASS editor language features and unsupported protected-build preservation");
    return;
  }
  await vscode.commands.executeCommand("citet.buildAndView");
  assert.ok(fs.statSync(pdf).size > 1000);
  assert.ok(fs.existsSync(pdf.replace(/\.pdf$/, ".synctex.gz")));
  await until(() => vscode.window.tabGroups.all.some((group) => group.tabs.some((tab) => tab.input instanceof vscode.TabInputCustom && tab.input.viewType === "citet.pdf")), "native PDF tab did not open");
  assert.equal(vscode.workspace.getConfiguration("editor", { uri: source, languageId: "latex" }).get("defaultFormatter"), "backmatter.tex-ls");
  const state = [...api.pdfViewer.panels.values()][0];
  assert.ok(state, "PDF viewer is attached to the local backend");
  await until(() => state.documentPages > 0, "native PDF viewer did not render a page");
  const mapped = await state.target.backend.sourceToPdf(source.fsPath, 3, 1);
  assert.equal(mapped.page, 1);
  assert.ok(mapped.width > 0 && mapped.height > 0);
  const inverse = await state.target.backend.pdfToSource(mapped.page, mapped.x + 10, mapped.y + mapped.height / 2);
  assert.equal(fs.realpathSync(inverse.filePath), fs.realpathSync(source.fsPath));
  assert.ok(inverse.line >= 2 && inverse.line <= 4);
  // The standalone tex-ls extension supplies formatting.
  const probe = vscode.Uri.file(path.join(root, ".texe", "editor", "language-probe.tex"));
  fs.mkdirSync(path.dirname(probe.fsPath), { recursive: true });
  fs.writeFileSync(probe.fsPath, "\\begin{itemize}\n\\item hello world\n\\end{itemize}\n");
  const probeDocument = await vscode.workspace.openTextDocument(probe);
  const probeEditor = await vscode.window.showTextDocument(probeDocument, vscode.ViewColumn.One);
  probeEditor.options = { tabSize: 2, insertSpaces: true };
  await until(async () => { await vscode.commands.executeCommand("editor.action.formatDocument"); return probeDocument.getText().includes("  \\item hello world"); }, "native LSP did not format the document");
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
  fs.unlinkSync(probe.fsPath);
  const liveEditor = await vscode.window.showTextDocument(document, vscode.ViewColumn.One);
  const before = fs.readFileSync(pdf);
  await liveEditor.edit((edit) => edit.insert(new vscode.Position(2, 0), "\\anUndefinedCommandForCutover\n"));
  await document.save();
  await until(() => vscode.languages.getDiagnostics(source).some((item) => item.source === "texe" && item.severity === vscode.DiagnosticSeverity.Error), "failed build did not publish a source diagnostic");
  assert.deepEqual(fs.readFileSync(pdf), before, "failed builds preserve the successful PDF");
  assert.equal(state.stale, true, "previous PDF is marked stale");
  await liveEditor.edit((edit) => edit.delete(new vscode.Range(2, 0, 3, 0)));
  await vscode.commands.executeCommand("citet.compile");
  await until(() => !vscode.languages.getDiagnostics(source).some((item) => item.source === "texe" && item.severity === vscode.DiagnosticSeverity.Error) && !state.stale, "successful rebuild did not clear the previous error");
  // Cancel a real engine loop, then prove the same project can build again.
  await liveEditor.edit((edit) => edit.insert(new vscode.Position(2, 0), "\\loop\\iftrue\\repeat\n"));
  await document.save();
  await until(() => [...api.paper.states.values()].some((state) => state.running && /Checking document requirements|Typesetting|Building PDF/.test(state.phase)), "saved loop did not start the TeX engine");
  await delay(500);
  await vscode.commands.executeCommand("citet.cancelBuild");
  await until(() => [...api.paper.states.values()].every((state) => !state.running), "local process tree did not stop after cancellation");
  await liveEditor.edit((edit) => edit.delete(new vscode.Range(2, 0, 3, 0)));
  await vscode.commands.executeCommand("citet.compile");
  await until(() => !state.stale, "building after cancellation did not recover");
  fs.writeFileSync(path.join(process.env.TEXE_TEST_ROOT, "results.json"), JSON.stringify({ extension: extension.id, localBuild: true, nativePdf: true, forwardSync: mapped, inverseSync: inverse, formatting: true, previousPdfPreserved: true, errorRecovery: true, cancellation: true, renderedPages: state.documentPages }, null, 2));
  console.log("PASS Citet and standalone tex-ls: local build, PDF tab, SyncTeX, language features and failure recovery");
};
