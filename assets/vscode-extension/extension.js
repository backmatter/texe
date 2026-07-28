const vscode = require("vscode");

async function exists(uri) {
  try {
    await vscode.workspace.fs.stat(uri);
    return true;
  } catch {
    return false;
  }
}

async function openPaper(context, folder) {
  const configuration = vscode.workspace.getConfiguration("texe", folder.uri);
  const request = configuration.get("editor.openPaper");
  if (
    !request ||
    typeof request.source !== "string" ||
    typeof request.pdf !== "string" ||
    typeof request.request !== "string"
  ) {
    return;
  }

  const stateKey = `opened:${folder.uri.toString()}:${request.request}`;
  if (context.workspaceState.get(stateKey)) {
    return;
  }

  const source = vscode.Uri.joinPath(folder.uri, request.source);
  const pdf = vscode.Uri.joinPath(folder.uri, request.pdf);
  if (!(await exists(source))) {
    return;
  }

  const document = await vscode.workspace.openTextDocument(source);
  await vscode.window.showTextDocument(document, {
    viewColumn: vscode.ViewColumn.One,
    preserveFocus: false,
    preview: false
  });

  if (await exists(pdf)) {
    const workshop = vscode.extensions.getExtension("James-Yu.latex-workshop");
    if (workshop && !workshop.isActive) {
      await workshop.activate();
    }
    await vscode.commands.executeCommand(
      "vscode.openWith",
      pdf,
      "latex-workshop-pdf-hook",
      {
        viewColumn: vscode.ViewColumn.Two,
        preserveFocus: false,
        preview: false
      }
    );
    await vscode.commands.executeCommand("workbench.action.focusLeftGroup");
  }

  await context.workspaceState.update(stateKey, true);
}

async function activate(context) {
  async function openConfiguredPapers(event) {
    const folders = vscode.workspace.workspaceFolders || [];
    for (const folder of folders) {
      if (
        event &&
        !event.affectsConfiguration("texe.editor.openPaper", folder.uri)
      ) {
        continue;
      }
      try {
        await openPaper(context, folder);
      } catch (error) {
        console.error("texe could not open the paper layout", error);
      }
    }
  }

  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      void openConfiguredPapers(event);
    })
  );
  await openConfiguredPapers();
}

function deactivate() {}

module.exports = { activate, deactivate };
