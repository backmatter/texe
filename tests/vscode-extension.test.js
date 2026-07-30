const assert = require("node:assert/strict");
const Module = require("node:module");
const path = require("node:path");
const test = require("node:test");

const extensionPath = path.resolve(
  __dirname,
  "../assets/vscode-extension/extension.js"
);

function uri(value) {
  return {
    value,
    toString() {
      return `file://${value}`;
    }
  };
}

function folder(value) {
  return {
    name: path.basename(value),
    uri: uri(value)
  };
}

function request(source = "main.tex", pdf = "main.pdf", id = "request-1") {
  return { source, pdf, request: id };
}

function createHarness({
  folders = [folder("/paper")],
  requests = new Map(),
  existing = [],
  activeFolder,
  failShowingSource = false
} = {}) {
  const files = new Set(existing);
  const state = new Map();
  const calls = {
    activatedExtensions: 0,
    commands: [],
    errors: [],
    information: [],
    openedDocuments: [],
    shownDocuments: [],
    warnings: []
  };
  const handlers = {};

  const watcher = {
    onDidCreate(callback) {
      handlers.created = callback;
      return { dispose() {} };
    },
    onDidChange(callback) {
      handlers.changed = callback;
      return { dispose() {} };
    },
    dispose() {}
  };
  const vscode = {
    ViewColumn: { One: 1, Two: 2 },
    Uri: {
      joinPath(base, relative) {
        return uri(path.posix.join(base.value, relative));
      }
    },
    commands: {
      async executeCommand(...arguments) {
        calls.commands.push(arguments);
      },
      registerCommand(name, callback) {
        handlers.command = callback;
        calls.registeredCommand = name;
        return { dispose() {} };
      }
    },
    extensions: {
      getExtension(name) {
        assert.equal(name, "James-Yu.latex-workshop");
        return {
          isActive: false,
          async activate() {
            calls.activatedExtensions += 1;
          }
        };
      }
    },
    window: {
      activeTextEditor: activeFolder
        ? { document: { uri: uri(path.posix.join(activeFolder.uri.value, "active.tex")) } }
        : undefined,
      async showErrorMessage(message) {
        calls.errors.push(message);
      },
      async showInformationMessage(message) {
        calls.information.push(message);
      },
      async showTextDocument(document, options) {
        if (failShowingSource) {
          throw new Error("simulated editor failure");
        }
        calls.shownDocuments.push([document, options]);
      },
      async showWarningMessage(message) {
        calls.warnings.push(message);
      }
    },
    workspace: {
      fs: {
        async stat(candidate) {
          if (!files.has(candidate.toString())) {
            throw new Error("not found");
          }
          return {};
        }
      },
      workspaceFolders: folders,
      createFileSystemWatcher(pattern) {
        calls.watcherPattern = pattern;
        return watcher;
      },
      getConfiguration(_section, folderUri) {
        return {
          get(key) {
            assert.equal(key, "editor.openPaper");
            return requests.get(folderUri.toString());
          }
        };
      },
      getWorkspaceFolder(candidate) {
        return folders.find((item) =>
          candidate.toString().startsWith(item.uri.toString())
        );
      },
      async openTextDocument(candidate) {
        calls.openedDocuments.push(candidate.toString());
        return { uri: candidate };
      },
      onDidChangeConfiguration(callback) {
        handlers.configuration = callback;
        return { dispose() {} };
      }
    }
  };
  const context = {
    subscriptions: [],
    workspaceState: {
      get(key) {
        return state.get(key);
      },
      async update(key, value) {
        state.set(key, value);
      }
    }
  };

  return {
    calls,
    context,
    files,
    handlers,
    vscode
  };
}

function loadExtension(vscode) {
  const originalLoad = Module._load;
  delete require.cache[extensionPath];
  Module._load = function load(request, parent, isMain) {
    if (request === "vscode") {
      return vscode;
    }
    return originalLoad.call(this, request, parent, isMain);
  };
  try {
    return require(extensionPath);
  } finally {
    Module._load = originalLoad;
  }
}

async function settle() {
  await new Promise((resolve) => setImmediate(resolve));
}

for (const event of ["created", "changed"]) {
  test(`opens a PDF when the watcher reports it was ${event}`, async () => {
    const project = folder("/paper");
    const projectRequest = request();
    const harness = createHarness({
      folders: [project],
      requests: new Map([[project.uri.toString(), projectRequest]]),
      existing: ["file:///paper/main.tex"]
    });
    const extension = loadExtension(harness.vscode);

    await extension.activate(harness.context);

    assert.equal(harness.calls.watcherPattern, "**/*.pdf");
    assert.deepEqual(harness.calls.openedDocuments, ["file:///paper/main.tex"]);
    assert.equal(harness.calls.commands.length, 0);

    const pdf = uri("/paper/main.pdf");
    harness.files.add(pdf.toString());
    harness.handlers[event](pdf);
    await settle();

    assert.equal(harness.calls.activatedExtensions, 1);
    assert.equal(harness.calls.commands[0][0], "vscode.openWith");
    assert.equal(harness.calls.commands[0][1].toString(), pdf.toString());
    assert.equal(
      harness.calls.commands[1][0],
      "workbench.action.focusLeftGroup"
    );
  });
}

test("the manual command force-reopens the active folder layout", async () => {
  const first = folder("/first");
  const active = folder("/active");
  const activeRequest = request("paper.tex", "paper.pdf", "active-request");
  const harness = createHarness({
    folders: [first, active],
    requests: new Map([[active.uri.toString(), activeRequest]]),
    existing: ["file:///active/paper.tex", "file:///active/paper.pdf"],
    activeFolder: active
  });
  const extension = loadExtension(harness.vscode);

  await extension.activate(harness.context);
  const openedBeforeCommand = harness.calls.openedDocuments.length;
  await harness.handlers.command();

  assert.equal(harness.calls.registeredCommand, "texe.openPaper");
  assert.equal(harness.calls.openedDocuments.length, openedBeforeCommand + 1);
  assert.equal(
    harness.calls.openedDocuments.at(-1),
    "file:///active/paper.tex"
  );
  assert.equal(
    harness.calls.commands.at(-2)[1].toString(),
    "file:///active/paper.pdf"
  );
});

test("manual command explains missing workspace, configuration, source, and PDF", async (t) => {
  await t.test("workspace", async () => {
    const harness = createHarness({ folders: [] });
    await loadExtension(harness.vscode).activate(harness.context);
    await harness.handlers.command();
    assert.match(harness.calls.information[0], /Open a texe project folder/);
  });

  await t.test("configuration", async () => {
    const project = folder("/paper");
    const harness = createHarness({ folders: [project] });
    await loadExtension(harness.vscode).activate(harness.context);
    await harness.handlers.command();
    assert.match(harness.calls.information[0], /texe editor/);
  });

  await t.test("source", async () => {
    const project = folder("/paper");
    const harness = createHarness({
      folders: [project],
      requests: new Map([[project.uri.toString(), request()]])
    });
    await loadExtension(harness.vscode).activate(harness.context);
    await harness.handlers.command();
    assert.match(harness.calls.warnings[0], /source file no longer exists/);
  });

  await t.test("PDF", async () => {
    const project = folder("/paper");
    const harness = createHarness({
      folders: [project],
      requests: new Map([[project.uri.toString(), request()]]),
      existing: ["file:///paper/main.tex"]
    });
    await loadExtension(harness.vscode).activate(harness.context);
    await harness.handlers.command();
    assert.match(harness.calls.information[0], /Build the paper/);
  });
});

test("manual command reports extension-host failures", async () => {
  const project = folder("/paper");
  const harness = createHarness({
    folders: [project],
    requests: new Map([[project.uri.toString(), request()]]),
    existing: ["file:///paper/main.tex", "file:///paper/main.pdf"],
    failShowingSource: true
  });
  const extension = loadExtension(harness.vscode);

  await extension.activate(harness.context);
  await harness.handlers.command();

  assert.match(harness.calls.errors[0], /could not open the paper layout/);
});
