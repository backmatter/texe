const vscode = require("vscode");

async function exists(uri) {
  try {
    await vscode.workspace.fs.stat(uri);
    return true;
  } catch {
    return false;
  }
}

function paperRequest(folder) {
  const configuration = vscode.workspace.getConfiguration("texe", folder.uri);
  const request = configuration.get("editor.openPaper");
  if (
    !request ||
    typeof request.source !== "string" ||
    typeof request.pdf !== "string" ||
    typeof request.request !== "string"
  ) {
    return undefined;
  }
  return request;
}

function stateKey(kind, folder) {
  return `${kind}:${folder.uri.toString()}`;
}

async function openPaper(context, folder, force = false) {
  const request = paperRequest(folder);
  if (!request) {
    return "not-configured";
  }

  const layoutKey = stateKey("layout", folder);
  if (!force && context.workspaceState.get(layoutKey) === request.request) {
    return "already-open";
  }

  const source = vscode.Uri.joinPath(folder.uri, request.source);
  const pdf = vscode.Uri.joinPath(folder.uri, request.pdf);
  if (!(await exists(source))) {
    return "source-missing";
  }

  const pdfExists = await exists(pdf);
  const sourceKey = stateKey("source", folder);
  if (
    force ||
    pdfExists ||
    context.workspaceState.get(sourceKey) !== request.request
  ) {
    const document = await vscode.workspace.openTextDocument(source);
    await vscode.window.showTextDocument(document, {
      viewColumn: vscode.ViewColumn.One,
      preserveFocus: false,
      preview: false
    });
    await context.workspaceState.update(sourceKey, request.request);
  }

  if (!pdfExists) {
    return "pdf-missing";
  }

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
  await context.workspaceState.update(layoutKey, request.request);
  return "opened";
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

  async function openChangedPdf(uri) {
    const folders = vscode.workspace.workspaceFolders || [];
    for (const folder of folders) {
      const request = paperRequest(folder);
      if (
        request &&
        vscode.Uri.joinPath(folder.uri, request.pdf).toString() === uri.toString()
      ) {
        try {
          await openPaper(context, folder);
        } catch (error) {
          console.error("texe could not open the completed paper", error);
        }
      }
    }
  }

  async function openPaperManually() {
    const activeUri = vscode.window.activeTextEditor?.document.uri;
    const folder =
      (activeUri && vscode.workspace.getWorkspaceFolder(activeUri)) ||
      vscode.workspace.workspaceFolders?.[0];
    if (!folder) {
      await vscode.window.showInformationMessage(
        "Open a texe project folder before opening its paper."
      );
      return;
    }
    try {
      const result = await openPaper(context, folder, true);
      if (result === "not-configured") {
        await vscode.window.showInformationMessage(
          "Run `texe editor` to configure this project first."
        );
      } else if (result === "source-missing") {
        await vscode.window.showWarningMessage(
          "The configured texe source file no longer exists."
        );
      } else if (result === "pdf-missing") {
        await vscode.window.showInformationMessage(
          "The source is open. Build the paper to add its PDF on the right."
        );
      }
    } catch (error) {
      console.error("texe could not open the paper layout", error);
      await vscode.window.showErrorMessage(
        "texe could not open the paper layout. See the extension host log for details."
      );
    }
  }

  const pdfWatcher = vscode.workspace.createFileSystemWatcher("**/*.pdf");
  context.subscriptions.push(
    vscode.commands.registerCommand("texe.openPaper", openPaperManually),
    pdfWatcher,
    pdfWatcher.onDidCreate((uri) => {
      void openChangedPdf(uri);
    }),
    pdfWatcher.onDidChange((uri) => {
      void openChangedPdf(uri);
    }),
    vscode.workspace.onDidChangeConfiguration((event) => {
      void openConfiguredPapers(event);
    })
  );
  await openConfiguredPapers();
}

function deactivate() {}

module.exports = { activate, deactivate };
