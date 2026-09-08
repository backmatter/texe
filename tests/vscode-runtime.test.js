const assert = require("node:assert/strict");
const test = require("node:test");
const { EventEmitter } = require("node:events");
const fs = require("node:fs");
const vm = require("node:vm");
const path = require("node:path");

function harness(outcomes, settings = {}, adoptionError = null) {
  const callbacks = new Map(),
    notifications = [],
    decorations = new Map(),
    markers = new Map(),
    calls = [],
    children = [],
    information = [],
    executed = [],
    labels = [];
  const events = {};
  const status = {
    show() {},
    hide() {},
    dispose() {},
    set text(value) {
      this.value = value;
      labels.push(value);
    },
    get text() {
      return this.value;
    },
  };
  let manifest = "entry = main.tex";
  let executablePath = "texe";
  let editorReport = {
    schema: "texe.editor-context/v1",
    source: "main.tex",
    pdf: "main.pdf",
    log: ".texe/build/output/main.log",
  };
  const disposable = { dispose() {} };
  class VSCodeEmitter {
    listeners = [];
    event = (callback) => {
      this.listeners.push(callback);
      return disposable;
    };
    fire(value) {
      for (const callback of this.listeners) callback(value);
    }
    dispose() {}
  }
  const uri = (file) => ({ fsPath: file, toString: () => `file://${file}` });
  const folder = { uri: uri("/paper"), name: "paper" };
  const config = {
    get: (key, fallback) =>
      ({ "editor.enabled": true, executablePath, ...settings })[key] ??
      fallback,
  };
  const vscode = {
    Uri: { file: uri },
    EventEmitter: VSCodeEmitter,
    ThemeColor: class {},
    StatusBarAlignment: { Left: 1 },
    ViewColumn: { One: 1 },
    DiagnosticSeverity: { Error: 0, Warning: 1 },
    Range: class {
      constructor(line) {
        this.start = { line };
      }
    },
    Diagnostic: class {
      constructor(range, message) {
        this.range = range;
        this.message = message;
      }
    },
    workspace: {
      isTrusted: true,
      workspaceFolders: [folder],
      textDocuments: [],
      getConfiguration: () => config,
      getWorkspaceFolder: () => folder,
      onDidSaveTextDocument: (callback) => {
        events.save = callback;
        return disposable;
      },
      onDidChangeConfiguration: (callback) => {
        events.config = callback;
        return disposable;
      },
      onDidChangeWorkspaceFolders: (callback) => {
        events.folders = callback;
        return disposable;
      },
      createFileSystemWatcher: (pattern) => ({
        dispose() {},
        onDidChange: (callback) => {
          if (pattern === "**/texe.toml") events.manifest = callback;
          else events.file = callback;
          return disposable;
        },
        onDidCreate: () => disposable,
        onDidDelete: () => disposable,
      }),
    },
    window: {
      createOutputChannel: () => ({
        append() {},
        appendLine() {},
        show() {},
        dispose() {},
      }),
      createStatusBarItem: () => status,
      registerFileDecorationProvider: (provider) => {
        decorations.set("provider", provider);
        return disposable;
      },
      onDidChangeActiveTextEditor: () => disposable,
      showErrorMessage: (message, ...actions) =>
        new Promise((resolve) =>
          notifications.push({ message, actions, resolve }),
        ),
      showInformationMessage: (message) => {
        information.push(message);
        return Promise.resolve();
      },
      showTextDocument: () => Promise.resolve(),
    },
    languages: {
      createDiagnosticCollection: () => ({
        set: (uri, value) => markers.set(uri.toString(), value),
        delete: (uri) => markers.delete(uri.toString()),
        forEach: (callback) => {
          for (const key of markers.keys()) callback(uri(key.slice(7)));
        },
        dispose() {},
      }),
    },
    commands: {
      registerCommand: (name, callback) => {
        callbacks.set(name, callback);
        return disposable;
      },
      executeCommand: (name) => {
        executed.push(name);
        return Promise.resolve();
      },
    },
    tasks: { registerTaskProvider: () => disposable },
  };
  function spawn(_program, args) {
    calls.push(args);
    const child = new EventEmitter();
    children.push(child);
    child.stdout = new EventEmitter();
    child.stderr = new EventEmitter();
    child.stdout.setEncoding = () => {};
    child.stderr.setEncoding = () => {};
    child.pid = 123;
    child.kill = () => child.emit("close", null, "SIGTERM");
    child.finish = (report) => {
      child.stdout.emit("data", JSON.stringify(report));
      child.emit("close", report.schema === "texe.error/v1" ? 6 : 0, null);
    };
    setImmediate(() => {
      const report = args[0] === "editor" ? editorReport : outcomes.shift();
      if (typeof report === "function") report(child);
      else child.finish(report);
    });
    return child;
  }
  const module = { exports: {} };
  vm.runInNewContext(
    fs.readFileSync(
      path.join(__dirname, "../assets/vscode-extension/runtime.js"),
      "utf8",
    ),
    {
      module,
      require: (name) =>
        name === "./build-bridge"
          ? { start: async () => disposable }
          : name === "vscode"
            ? vscode
            : name === "node:child_process"
              ? { spawn }
              : name === "node:fs"
                ? {
                    existsSync: () => true,
                    readFileSync: (file) =>
                      file.endsWith("adoption-error.json") && adoptionError
                        ? JSON.stringify(adoptionError)
                        : manifest,
                    unlinkSync: () => {
                      adoptionError = null;
                    },
                    statSync: () => ({
                      isFile: () => true,
                      size: 1,
                      mtimeMs: 1,
                    }),
                  }
                : require(name),
      process: { ...process, kill: () => children.at(-1).kill() },
      setInterval,
      clearInterval,
      setTimeout,
      clearTimeout,
      console,
    },
  );
  const context = { subscriptions: [] };
  const requests = new Map();
  const api = module.exports.activate(context, requests, () =>
    Promise.resolve(),
  );
  return {
    api,
    callbacks,
    notifications,
    decorations,
    markers,
    context,
    calls,
    children,
    information,
    executed,
    labels,
    status,
    events,
    folder,
    uri,
    vscode,
    requests,
    setEditorReport: (value) => {
      editorReport = value;
    },
    setManifest: (value) => {
      manifest = value;
    },
    setExecutable: (value) => {
      executablePath = value;
    },
  };
}
const failure = {
  schema: "texe.error/v1",
  error: {
    message: "build failed",
    diagnostic: {
      message: "Unknown command.",
      explanation: "Check the command spelling.",
      action: "Fix and save.",
      location: { file: "main.tex", line: 3 },
      log: ".texe/build/discovery/main.log",
    },
  },
};
const success = { schema: "texe.build-report/v1", warning_count: 0 };

test("error toast does not block recovery, deduplicates repeats and marks the retained PDF", async () => {
  const h = harness([failure, failure, success, failure]);
  await h.api.ready;
  assert.equal((await h.callbacks.get("texe.build")()).failed, true);
  assert.equal(h.notifications.length, 1);
  assert.match(h.notifications[0].message, /previous successful build/);
  assert.deepEqual(h.notifications[0].actions, [
    "Show Error",
    "Open Build Log",
  ]);
  assert.equal(
    h.decorations
      .get("provider")
      .provideFileDecoration({ toString: () => "file:///paper/main.pdf" })
      .badge,
    "!",
  );
  await h.callbacks.get("texe.build")();
  assert.equal(
    h.notifications.length,
    1,
    "same failure should not stack toasts",
  );
  await h.callbacks.get("texe.build")();
  assert.equal(h.markers.size, 0);
  assert.equal(
    h.decorations
      .get("provider")
      .provideFileDecoration({ toString: () => "file:///paper/main.pdf" }),
    undefined,
  );
  await h.callbacks.get("texe.build")();
  assert.equal(
    h.notifications.length,
    2,
    "a failure after recovery needs a new toast",
  );
  h.context.subscriptions.forEach((item) => item.dispose());
});

const tick = () =>
  new Promise((resolve) => setImmediate(() => setImmediate(resolve)));
const dispose = (h) =>
  h.context.subscriptions.forEach((item) => item.dispose());

test("editor builds download missing dependencies by default and honor explicit offline settings", async () => {
  const manifest = JSON.parse(
    fs.readFileSync(
      path.join(__dirname, "../assets/vscode-extension/package.json"),
      "utf8",
    ),
  );
  assert.equal(
    manifest.contributes.configuration.properties["texe.allowDownloads"]
      .default,
    true,
  );
  for (const [settings, expected, absent] of [
    [{}, "--yes", "--offline"],
    [{ allowDownloads: false }, "--offline", "--yes"],
  ]) {
    const h = harness([success], settings);
    try {
      await h.api.ready;
      await h.callbacks.get("texe.build")();
      const args = h.calls.find((args) => args[0] === "build");
      assert.ok(args.includes(expected));
      assert.ok(!args.includes(absent));
    } finally {
      dispose(h);
    }
  }
});

test("cancel is neutral even when termination has an exit code instead of a signal", async () => {
  let child;
  const h = harness([
    (value) => {
      child = value;
    },
    success,
  ]);
  await h.api.ready;
  const pending = h.callbacks.get("texe.build")();
  await tick();
  assert.equal(h.status.command, "texe.cancelBuild");
  child.kill = () => child.emit("close", 1, null);
  await h.callbacks.get("texe.cancelBuild")();
  assert.equal((await pending).cancelled, true);
  assert.equal(h.notifications.length, 0);
  assert.equal(h.markers.size, 0);
  assert.match(h.status.text, /cancelled/);
  assert.equal(h.status.command, "texe.buildAndView");
  assert.equal(
    h.decorations
      .get("provider")
      .provideFileDecoration(h.uri("/paper/main.pdf")),
    undefined,
  );
  await h.callbacks.get("texe.build")();
  assert.match(h.status.text, /built/);
  dispose(h);
});

test("unexpected process termination remains an error", async () => {
  const h = harness([(child) => child.emit("close", null, "SIGTERM")]);
  await h.api.ready;
  assert.equal((await h.callbacks.get("texe.build")()).failed, true);
  assert.equal(h.notifications.length, 1);
  assert.match(h.notifications[0].message, /unexpectedly/);
  dispose(h);
});

test("queued saves suppress intermediate success, recovery and viewer refresh", async () => {
  let first, latest;
  const h = harness([
    failure,
    (child) => {
      first = child;
    },
    (child) => {
      latest = child;
    },
  ]);
  await h.api.ready;
  await h.callbacks.get("texe.build")();
  h.labels.length = 0;
  const pending = h.callbacks.get("texe.build")();
  await tick();
  h.events.save({ uri: h.uri("/paper/main.tex") });
  assert.match(h.status.text, /Building latest changes/);
  first.finish(success);
  await tick();
  assert.equal(h.information.length, 0);
  assert.equal(h.executed.includes("latex-workshop.refresh-viewer"), false);
  assert.equal(
    h.labels.some((value) => /texe: built/.test(value)),
    false,
  );
  assert.equal(
    h.markers.size,
    1,
    "retain prior diagnostics until latest build completes",
  );
  latest.finish(success);
  await pending;
  assert.equal(h.information.length, 1);
  assert.equal(h.markers.size, 0);
  dispose(h);
});

test("an obsolete failure does not flash an error before a queued successful build", async () => {
  let child;
  const h = harness([
    (value) => {
      child = value;
    },
    success,
  ]);
  await h.api.ready;
  const pending = h.callbacks.get("texe.build")();
  await tick();
  const queued = h.callbacks.get("texe.build")();
  child.finish(failure);
  await Promise.all([pending, queued]);
  assert.equal(h.notifications.length, 0);
  assert.match(h.status.text, /built/);
  dispose(h);
});

test("editor context is reused and invalidated by manifest and executable changes", async () => {
  const h = harness([success, success, success, success, success]);
  await h.api.ready;
  const inspections = () =>
    h.calls.filter((args) => args.includes("--inspect")).length;
  await h.callbacks.get("texe.build")();
  await h.callbacks.get("texe.build")();
  assert.equal(inspections(), 1);
  h.setManifest("entry = revised.tex");
  await h.callbacks.get("texe.build")();
  assert.equal(inspections(), 2);
  h.setExecutable("/tools/new-texe");
  h.events.config({ affectsConfiguration: () => true });
  await h.callbacks.get("texe.build")();
  assert.equal(inspections(), 3);
  h.events.folders({ removed: [h.folder], added: [] });
  h.events.folders({ removed: [], added: [h.folder] });
  await tick();
  await h.callbacks.get("texe.build")();
  assert.equal(inspections(), 4);
  dispose(h);
});

test("a changed manifest cannot publish an in-flight stale inspection", async () => {
  const h = harness([]);
  await h.api.ready;
  let held;
  h.setManifest("intermediate");
  h.setEditorReport((child) => {
    held = child;
  });
  const pending = h.api.inspect(h.folder);
  await tick();
  h.setManifest("latest");
  const latest = {
    schema: "texe.editor-context/v1",
    source: "latest.tex",
    pdf: "latest.pdf",
    log: ".texe/latest.log",
  };
  h.setEditorReport(latest);
  held.finish({ ...latest, source: "old.tex", pdf: "old.pdf" });
  assert.equal((await pending).source, "latest.tex");
  assert.equal(h.requests.get(h.folder.uri.toString()).source, "latest.tex");
  dispose(h);
});

test("cancelling during inspection prevents the compiler from starting", async () => {
  const h = harness([]);
  await h.api.ready;
  let held;
  h.setManifest("changed");
  h.setEditorReport((child) => {
    held = child;
  });
  const pending = h.callbacks.get("texe.build")();
  await tick();
  await h.callbacks.get("texe.cancelBuild")();
  held.finish({
    schema: "texe.editor-context/v1",
    source: "main.tex",
    pdf: "main.pdf",
    log: ".texe/main.log",
  });
  assert.equal((await pending).cancelled, true);
  assert.equal(
    h.calls.some((args) => args[0] === "build"),
    false,
  );
  assert.equal(h.notifications.length, 0);
  dispose(h);
});

test("progress handles split JSON and human output and exposes elapsed time and Cancel", async () => {
  let child;
  const h = harness([
    (value) => {
      child = value;
    },
  ]);
  await h.api.ready;
  const pending = h.callbacks.get("texe.build")();
  await tick();
  child.stderr.emit(
    "data",
    'human output\n{"schema":"texe.build-progress/v1","phase":"pack',
  );
  child.stderr.emit("data", 'ages","elapsed_millis":42}\n');
  assert.match(h.status.text, /Checking packages · [\d.]+s/);
  assert.match(h.status.tooltip, /cancel/i);
  child.stderr.emit(
    "data",
    '{"schema":"texe.build-progress/v1","phase":"engine-final","elapsed_millis":50}\n',
  );
  assert.match(h.status.text, /Typesetting/);
  child.finish(success);
  await pending;
  dispose(h);
});

test("a manual queued build waits for dirty documents to save without an extra rebuild", async () => {
  let first, latest, finishSave;
  const h = harness([
    (child) => {
      first = child;
    },
    (child) => {
      latest = child;
    },
  ]);
  await h.api.ready;
  const pending = h.callbacks.get("texe.build")();
  await tick();
  h.vscode.workspace.textDocuments = [
    {
      isDirty: true,
      uri: h.uri("/paper/main.tex"),
      save: () =>
        new Promise((resolve) => {
          finishSave = resolve;
        }),
    },
  ];
  const queued = h.callbacks.get("texe.build")();
  first.finish(success);
  await tick();
  assert.equal(h.calls.filter((args) => args[0] === "build").length, 1);
  assert.equal(
    h.labels.some((value) => /texe: built/.test(value)),
    false,
  );
  finishSave(true);
  await tick();
  latest.finish(success);
  await Promise.all([pending, queued]);
  assert.equal(h.calls.filter((args) => args[0] === "build").length, 2);
  dispose(h);
});

test("removing a failed workspace clears its diagnostics and PDF decoration", async () => {
  const h = harness([failure]);
  await h.api.ready;
  await h.callbacks.get("texe.build")();
  assert.equal(h.markers.size, 1);
  h.events.folders({ removed: [h.folder], added: [] });
  assert.equal(h.markers.size, 0);
  assert.equal(
    h.decorations
      .get("provider")
      .provideFileDecoration(h.uri("/paper/main.pdf")),
    undefined,
  );
  dispose(h);
});

test("a completed failed build replaces diagnostics from the previous chapter", async () => {
  const other = JSON.parse(JSON.stringify(failure));
  other.error.diagnostic.location.file = "chapter2.tex";
  const h = harness([failure, other]);
  try {
    await h.api.ready;
    await h.api.build(h.folder);
    await h.api.build(h.folder);
    assert.deepEqual([...h.markers.keys()], ["file:///paper/chapter2.tex"]);
    assert.equal(h.status.command, "texe.showError");
  } finally {
    dispose(h);
  }
});

test("warnings appear in Problems with a log destination and the status opens Problems", async () => {
  const h = harness([
    {
      ...success,
      warning_count: 1,
      warnings: [
        { kind: "unresolved-citation", message: "Citation missing undefined" },
      ],
    },
  ]);
  try {
    await h.api.ready;
    await h.api.build(h.folder);
    assert.equal(
      h.markers.get("file:///paper/.texe/build/output/main.log")[0].message,
      "Citation missing undefined",
    );
    assert.equal(h.status.command, "texe.showProblems");
  } finally {
    dispose(h);
  }
});

test("external figure changes rebuild, but published artifacts and private state do not", async () => {
  const h = harness([success]);
  try {
    await h.api.ready;
    for (const file of [
      "main.pdf",
      "main.synctex.gz",
      "texe.lock",
      ".texe/build/output/main.log",
      ".vscode/settings.json",
    ])
      h.events.file(h.uri("/paper/" + file));
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.equal(h.calls.filter((args) => args[0] === "build").length, 0);
    h.events.file(h.uri("/paper/figures/result.png"));
    await new Promise((resolve) => setTimeout(resolve, 300));
    assert.equal(h.calls.filter((args) => args[0] === "build").length, 1);
  } finally {
    dispose(h);
  }
});

test("the first editor session displays the adoption failure and a repair clears it", async () => {
  const h = harness([success], {}, failure);
  try {
    await h.api.ready;
    assert.equal(h.markers.size, 1);
    assert.match(h.notifications[0].message, /previous successful build/);
    await h.api.build(h.folder);
    assert.equal(h.markers.size, 0);
  } finally {
    dispose(h);
  }
});
