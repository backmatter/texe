const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vscode = require("vscode");
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function until(condition, message, timeout = 60000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) {
    const value = await condition();
    if (value) return value;
    await delay(100);
  }
  throw new Error(message);
}
exports.run = async () => {
  const folder = vscode.workspace.workspaceFolders[0];
  const root = folder.uri.fsPath;
  const source = vscode.Uri.file(path.join(root, "paper.v2.tex"));
  const pdf = vscode.Uri.file(path.join(root, "paper.v2.pdf"));
  const companion = vscode.extensions.getExtension(
    "backmatter.texe-paper-layout",
  );
  const api = await companion.activate();
  await api.ready;
  const report = await vscode.commands.executeCommand("texe.buildAndView");
  assert.equal(report.schema, "texe.build-report/v1", JSON.stringify(report));
  assert.ok(fs.statSync(pdf.fsPath).size > 1000);
  assert.ok(fs.existsSync(path.join(root, "paper.v2.synctex.gz")));
  const tabs = vscode.window.tabGroups.all.flatMap((group) => group.tabs);
  assert.ok(
    tabs.some((tab) => tab.input?.uri?.fsPath === pdf.fsPath),
    "real PDF editor opened",
  );
  console.log(
    "PASS real build and PDF tab for dotted filename in path with spaces",
  );

  const { connect } = require("./cdp");
  const page = await connect(
    (target) => target.type === "page" && target.url.startsWith("vscode-file:"),
  );
  const pdfFrame = await until(
    () => connect((target) => target.url.includes("viewer.html?")),
    "PDF webview did not load",
  );
  await until(
    () => pdfFrame.evaluate("document.body.innerText.includes('real paper')"),
    "PDF pages were not rendered",
  );
  await page.screenshot(path.join(process.env.TEXE_TEST_ROOT, "01-built.png"));
  console.log("PASS real PDF webview rendered document text");
  const workshop = vscode.extensions.getExtension("James-Yu.latex-workshop");
  await workshop.activate();
  // Pinned integration-test hooks only: production uses public VS Code commands.
  // Reuse the activated module: Windows drive-letter casing can otherwise
  // cause Node to load a second copy and break Workshop's circular imports.
  const modulePath = path.join(workshop.extensionPath, "out/src/locate/synctex.js");
  const loaded = Object.values(require.cache).find(
    (module) => module.filename && path.relative(module.filename, modulePath) === "",
  );
  assert.ok(loaded, "activated LaTeX Workshop SyncTeX module was not loaded");
  const { synctex } = loaded.exports;
  const forward = await synctex.components.synctexToPDFCombined(
    3,
    0,
    source.fsPath,
    pdf,
    true,
  );
  assert.ok(forward.page > 0);
  console.log("FORWARD", JSON.stringify(forward));
  const reverse = await synctex.components.computeToTeX(
    { page: forward.page, pos: [forward.x, forward.y] },
    pdf,
  );
  assert.ok(reverse?.input.endsWith("paper.v2.tex"), JSON.stringify(reverse));
  await synctex.toTeX({ page: forward.page, pos: [forward.x, forward.y] }, pdf);
  await until(
    () => vscode.window.activeTextEditor?.document.uri.fsPath === source.fsPath,
    "inverse sync did not open source",
  );
  console.log("PASS forward and inverse SyncTeX through LaTeX Workshop");

  const beforeCancel = fs.readFileSync(pdf.fsPath);
  const cancellationDoc = await vscode.workspace.openTextDocument(source);
  const cancellationEdit = new vscode.WorkspaceEdit();
  cancellationEdit.insert(
    source,
    cancellationDoc.positionAt(cancellationDoc.getText().length),
    "\n% cancellation test\n",
  );
  await vscode.workspace.applyEdit(cancellationEdit);
  const cancelling = vscode.commands.executeCommand("texe.build");
  const cancelTarget = await until(
    () =>
      page.evaluate(`(() => {
      const item = [...document.querySelectorAll('.statusbar-item')].find(
        node => /texe:.*[0-9]+\\.[0-9]+s/.test(node.textContent));
      if (!item) return null;
      const rect = item.getBoundingClientRect();
      return { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 };
    })()`),
    "elapsed build progress did not appear in the status bar",
  );
  await page.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    button: "left",
    clickCount: 1,
    ...cancelTarget,
  });
  await page.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    button: "left",
    clickCount: 1,
    ...cancelTarget,
  });
  assert.equal(
    (await cancelling).cancelled,
    true,
    "status-bar click did not cancel the build",
  );
  assert.ok(
    fs.readFileSync(pdf.fsPath).equals(beforeCancel),
    "cancel replaced the PDF",
  );
  assert.equal(
    vscode.languages.getDiagnostics(source).length,
    0,
    "cancel created a source error",
  );
  await until(
    () =>
      page.evaluate(
        "document.querySelector('.statusbar')?.innerText.includes('build cancelled')",
      ),
    "neutral cancellation status did not appear",
  );
  await page.screenshot(
    path.join(process.env.TEXE_TEST_ROOT, "01-cancelled.png"),
  );
  console.log(
    "PASS live elapsed progress, status-bar Cancel and neutral cancellation",
  );

  const before = fs.readFileSync(pdf.fsPath);
  const document = await vscode.workspace.openTextDocument(source);
  const edit = new vscode.WorkspaceEdit();
  edit.insert(source, new vscode.Position(2, 0), "An edit built on save. ");
  await vscode.workspace.applyEdit(edit);
  await document.save();
  await until(
    () => !fs.readFileSync(pdf.fsPath).equals(before),
    "saving did not rebuild PDF",
    120000,
  );
  await until(
    () =>
      pdfFrame
        .evaluate("document.body.innerText.includes('edit built on save')")
        .catch(() => false),
    "PDF viewer did not refresh after save",
  );
  await page.screenshot(
    path.join(process.env.TEXE_TEST_ROOT, "02-refreshed.png"),
  );
  console.log("PASS real save event rebuilt PDF and viewer refreshed");

  const good = fs.readFileSync(pdf.fsPath);
  const invalid = new vscode.WorkspaceEdit();
  invalid.insert(source, new vscode.Position(2, 0), "\\undefinedTexeCommand ");
  await vscode.workspace.applyEdit(invalid);
  await document.save();
  await until(
    () =>
      vscode.languages
        .getDiagnostics(source)
        .some(
          (d) =>
            d.source === "texe" ||
            d.message.includes("Undefined") ||
            d.message.includes("undefined"),
        ),
    "build error did not appear in Problems",
    120000,
  );
  assert.deepEqual(
    fs.readFileSync(pdf.fsPath),
    good,
    "failed build replaced previous PDF",
  );
  const diagnostic = vscode.languages.getDiagnostics(source)[0];
  assert.equal(diagnostic.range.start.line, 2);
  await until(
    () =>
      page.evaluate(
        `document.body.innerText.includes("Showing the PDF from the previous successful build.")`,
      ),
    "automatic build failure did not show an error toast",
  );
  await delay(350); // Wait for the notification slide-in before capturing evidence.
  await page.screenshot(
    path.join(process.env.TEXE_TEST_ROOT, "03-error-toast.png"),
  );
  const toastButton = await page.evaluate(`(() => {
    const button = [...document.querySelectorAll('.notifications-toasts button, .notifications-toasts .monaco-button')].find(button => button.textContent.trim() === 'Show Error');
    if (!button) return null; const rect = button.getBoundingClientRect(); return {x:rect.x+rect.width/2,y:rect.y+rect.height/2};
  })()`);
  assert.ok(toastButton, "toast exposes Show Error");
  await page.send("Input.dispatchMouseEvent", {
    type: "mousePressed",
    button: "left",
    clickCount: 1,
    ...toastButton,
  });
  await page.send("Input.dispatchMouseEvent", {
    type: "mouseReleased",
    button: "left",
    clickCount: 1,
    ...toastButton,
  });
  await until(
    () => vscode.window.activeTextEditor?.selection.active.line === 2,
    "toast action did not navigate to the error",
  );
  console.log(
    "PASS automatic error toast explains previous PDF; clicked Show Error",
  );
  await vscode.commands.executeCommand("texe.showProblems");
  const row = await until(
    () =>
      page.evaluate(`(() => {
    const row = [...document.querySelectorAll('.monaco-list-row')].find(row => row.innerText.includes(${JSON.stringify(diagnostic.message.split("\n")[0])}));
    if (!row) return null;
    row.scrollIntoView({block:'nearest'});
    const rect = row.getBoundingClientRect();
    if (rect.width < 1 || rect.height < 1 || rect.y + 8 >= innerHeight - 22) return null;
    return {x:rect.x + Math.min(180,rect.width/2),y:rect.y+8};
  })()`),
    "error row was not visible",
  );
  for (const clickCount of [1, 2]) {
    await page.send("Input.dispatchMouseEvent", {
      type: "mousePressed",
      button: "left",
      clickCount,
      ...row,
    });
    await page.send("Input.dispatchMouseEvent", {
      type: "mouseReleased",
      button: "left",
      clickCount,
      ...row,
    });
  }
  await until(
    () =>
      vscode.window.activeTextEditor?.document.uri.fsPath === source.fsPath &&
      vscode.window.activeTextEditor.selection.active.line === 2,
    "clicking Problems did not navigate to source",
  );
  await page.screenshot(path.join(process.env.TEXE_TEST_ROOT, "03-error.png"));
  console.log(
    "PASS clicked actual Problems row; navigated to source, previous PDF retained",
  );

  const repair = new vscode.WorkspaceEdit();
  repair.replace(
    source,
    new vscode.Range(2, 0, 2, "\\undefinedTexeCommand ".length),
    "",
  );
  await vscode.workspace.applyEdit(repair);
  await document.save();
  await until(
    () => vscode.languages.getDiagnostics(source).length === 0,
    "successful rebuild did not clear error",
    120000,
  );
  console.log("PASS save after repair clears Problems");
  await vscode.commands.executeCommand("texe.openBuildLog");
  assert.ok(
    vscode.window.activeTextEditor.document.uri.fsPath.endsWith("paper.v2.log"),
  );

  await vscode.window.showTextDocument(document, vscode.ViewColumn.One);
  await vscode.commands.executeCommand("texe.enableFamiliarShortcuts");
  await until(
    () =>
      vscode.workspace
        .getConfiguration("texe", folder.uri)
        .get("editor.familiarShortcuts"),
    "shortcuts were not enabled",
  );
  const shortcutEdit = new vscode.WorkspaceEdit();
  shortcutEdit.insert(
    source,
    document.positionAt(document.getText().length),
    "\n% F6 saves before building\n",
  );
  await vscode.workspace.applyEdit(shortcutEdit);
  const modified = fs.statSync(pdf.fsPath).mtimeMs;
  await page.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "F6",
    code: "F6",
    windowsVirtualKeyCode: 117,
  });
  await page.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    key: "F6",
    code: "F6",
    windowsVirtualKeyCode: 117,
  });
  await until(
    () => fs.statSync(pdf.fsPath).mtimeMs > modified,
    "F6 did not build",
    120000,
  );
  console.log("PASS actual F6 keybinding builds the paper");
  const tasks = await vscode.tasks.fetchTasks({ type: "texe" });
  assert.equal(tasks.length, 1, "texe build task is available");

  const workshopEdit = new vscode.WorkspaceEdit();
  workshopEdit.insert(
    source,
    new vscode.Position(2, 0),
    "Workshop button saves and builds. ",
  );
  await vscode.workspace.applyEdit(workshopEdit);
  await vscode.commands.executeCommand("latex-workshop.build");
  await until(
    () =>
      pdfFrame.evaluate("document.body.innerText.includes('Workshop button')"),
    "Workshop build did not save and use the companion queue",
    120000,
  );
  console.log(
    "PASS LaTeX Workshop Build saves dirty source and uses the companion queue",
  );

  const latencies = [];
  for (let editNumber = 0; editNumber < 3; editNumber++) {
    const token = `RefreshSample${editNumber}`;
    const edit = new vscode.WorkspaceEdit();
    edit.insert(source, new vscode.Position(2, 0), token + " ");
    await vscode.workspace.applyEdit(edit);
    const started = Date.now();
    await document.save();
    await until(
      () => pdfFrame.evaluate(`document.body.innerText.includes('${token}')`),
      "timed save did not render",
      120000,
    );
    latencies.push(Date.now() - started);
  }
  console.log("SAVE_TO_VISIBLE_PDF_MS", JSON.stringify(latencies));

  fs.copyFileSync(source.fsPath, path.join(root, "revised.v3.tex"));
  const manifestUri = vscode.Uri.file(path.join(root, "texe.toml"));
  const manifestDoc = await vscode.workspace.openTextDocument(manifestUri);
  const manifestEdit = new vscode.WorkspaceEdit();
  manifestEdit.replace(
    manifestUri,
    new vscode.Range(
      manifestDoc.positionAt(0),
      manifestDoc.positionAt(manifestDoc.getText().length),
    ),
    manifestDoc.getText().replace("paper.v2.tex", "revised.v3.tex"),
  );
  await vscode.workspace.applyEdit(manifestEdit);
  await manifestDoc.save();
  await until(
    () =>
      vscode.workspace
        .getConfiguration("latex-workshop", folder.uri)
        .get("latex.search.rootFiles.include")?.[0] === "revised.v3.tex",
    "manifest edit did not refresh editor root",
  );
  const revised = await vscode.commands.executeCommand("texe.buildAndView");
  assert.equal(revised.schema, "texe.build-report/v1");
  assert.ok(fs.existsSync(path.join(root, "revised.v3.pdf")));
  console.log(
    "PASS manifest root change updates configuration and builds the new PDF",
  );
  await page.screenshot(
    path.join(process.env.TEXE_TEST_ROOT, "04-recovered.png"),
  );
  await vscode.commands.executeCommand("texe.gettingStarted");
  await until(
    () =>
      page.evaluate(
        "document.body.innerText.includes('Write your paper with texe')",
      ),
    "Writing Guide walkthrough did not open",
  );
  await page.screenshot(
    path.join(process.env.TEXE_TEST_ROOT, "05-writing-guide.png"),
  );
  const writingSettings = () =>
    vscode.workspace.getConfiguration(undefined, folder.uri);
  await writingSettings().update(
    "editor.wordWrap",
    "off",
    vscode.ConfigurationTarget.Workspace,
  );
  await writingSettings().update(
    "files.exclude",
    { "**/keep-private": true },
    vscode.ConfigurationTarget.Workspace,
  );
  const originalWrap = writingSettings().get("editor.wordWrap");
  const originalExcludes = writingSettings().get("files.exclude");
  const layoutResult =
    await vscode.commands.executeCommand("texe.writingLayout");
  assert.ok(!layoutResult?.failed, "Writing Layout command failed");
  assert.equal(writingSettings().get("editor.wordWrap"), "on");
  assert.equal(writingSettings().get("files.exclude")["**/.texe"], true);
  await page.screenshot(
    path.join(process.env.TEXE_TEST_ROOT, "05-writing-layout.png"),
  );
  await writingSettings().update(
    "files.exclude",
    { ...writingSettings().get("files.exclude"), "**/later-user-edit": true },
    vscode.ConfigurationTarget.Workspace,
  );
  await vscode.commands.executeCommand("texe.restoreWritingLayout");
  assert.equal(writingSettings().get("editor.wordWrap"), originalWrap);
  assert.deepEqual(writingSettings().get("files.exclude"), {
    ...originalExcludes,
    "**/later-user-edit": true,
  });
  console.log(
    "PASS optional writing layout and restoration of previous folder settings",
  );
  pdfFrame.close();
  page.close();
  fs.writeFileSync(
    path.join(process.env.TEXE_TEST_ROOT, "results.json"),
    JSON.stringify(
      {
        build: true,
        workshopBuild: true,
        writingLayout: true,
        saveToVisiblePdfMillis: latencies,
        cancellation: true,
        liveProgress: true,
        save: true,
        errors: true,
        recovery: true,
        forwardSync: true,
        inverseSync: true,
        log: true,
        shortcuts: true,
        manifestChange: true,
        renderedRefresh: true,
        clickedProblems: true,
        errorToast: true,
      },
      null,
      2,
    ),
  );
};
