const vscode = require("vscode");
const { spawn } = require("node:child_process");
const path = require("node:path");
const fs = require("node:fs");
const bridge = require("./build-bridge");

function isWithin(root, file) {
  const relative = path.relative(root, file);
  return (
    Boolean(relative) &&
    !path.isAbsolute(relative) &&
    relative !== ".." &&
    !relative.startsWith(".." + path.sep)
  );
}

function inside(root, relative) {
  if (typeof relative !== "string" || !relative || path.isAbsolute(relative)) {
    throw new Error("texe returned an invalid project-relative path");
  }
  const resolved = path.resolve(root, relative);
  if (!isWithin(root, resolved))
    throw new Error("texe path escaped the project folder");
  return resolved;
}

function activate(context, requests, openPaper) {
  const output = vscode.window.createOutputChannel("texe");
  const diagnostics = vscode.languages.createDiagnosticCollection("texe");
  const status = vscode.window.createStatusBarItem(
    vscode.StatusBarAlignment.Left,
    20,
  );
  status.command = "texe.buildAndView";
  const states = new Map();
  const stalePdfs = new Map();
  const decorationChanged = new vscode.EventEmitter();
  context.subscriptions.push(
    decorationChanged,
    vscode.window.registerFileDecorationProvider({
      onDidChangeFileDecorations: decorationChanged.event,
      provideFileDecoration(uri) {
        return stalePdfs.get(uri.toString());
      },
    }),
  );
  let disposed = false;
  const configuration = (folder) =>
    vscode.workspace.getConfiguration("texe", folder.uri);
  function state(folder) {
    if (!states.has(folder.uri.toString()))
      states.set(folder.uri.toString(), {
        folder,
        label: "$(file-pdf) texe",
        running: null,
      });
    return states.get(folder.uri.toString());
  }
  function currentFolder() {
    return (
      vscode.workspace.getWorkspaceFolder(
        vscode.window.activeTextEditor?.document.uri ||
          vscode.window.tabGroups?.activeTabGroup?.activeTab?.input?.uri ||
          vscode.Uri.file("/"),
      ) || vscode.workspace.workspaceFolders?.[0]
    );
  }
  function render(folder) {
    if (folder && !configuration(folder).get("editor.enabled", false)) {
      status.hide();
      return;
    }
    if (folder && currentFolder()?.uri.toString() === folder.uri.toString()) {
      const active = state(folder).running;
      status.text = state(folder).label;
      status.command = active
        ? "texe.cancelBuild"
        : state(folder).action || "texe.buildAndView";
      status.tooltip = active
        ? `${folder.name}: ${state(folder).label}. Click to cancel this build.`
        : state(folder).action === "texe.showProblems"
          ? `${folder.name}: Click to inspect build warnings in Problems.`
          : state(folder).action === "texe.showError"
            ? `${folder.name}: Click to inspect the latest build error.`
            : `${folder.name}: Build and view paper. See texe output for details.`;
      status.show();
    }
  }
  function label(folder, value) {
    state(folder).label = value;
    render(folder);
  }
  function executable(folder) {
    const configured = configuration(folder).get("executablePath", "texe");
    // A checked-in absolute path may belong to another machine. Try PATH there.
    return path.isAbsolute(configured) && !fs.existsSync(configured)
      ? "texe"
      : configured;
  }
  function run(folder, args, track = false) {
    if (!vscode.workspace.isTrusted)
      return Promise.reject(
        new Error("Trust this paper folder before running texe."),
      );
    const s = state(folder);
    return new Promise((resolve, reject) => {
      const program = executable(folder);
      output.appendLine(`\n[${folder.name}] ${program} ${args.join(" ")}`);
      const child = spawn(
        program,
        [...args, "--project", folder.uri.fsPath, "--json"],
        {
          cwd: folder.uri.fsPath,
          shell: false,
          windowsHide: true,
          detached: track && process.platform !== "win32",
        },
      );
      if (track) s.child = child;
      let stdout = "";
      let exceeded = false;
      let progressBuffer = "";
      child.stdout.setEncoding("utf8");
      child.stdout.on("data", (data) => {
        stdout += data;
        if (stdout.length > 8 * 1024 * 1024) {
          exceeded = true;
          stdout = "";
          child.kill();
        }
      });
      child.stderr.setEncoding("utf8");
      child.stderr.on("data", (data) => {
        output.append(data);
        if (!track || s.cancelled) return;
        progressBuffer += data;
        let end;
        while ((end = progressBuffer.indexOf("\n")) !== -1) {
          const line = progressBuffer.slice(0, end);
          progressBuffer = progressBuffer.slice(end + 1);
          try {
            const event = JSON.parse(line);
            if (
              event.schema === "texe.build-progress/v1" &&
              phases[event.phase]
            ) {
              s.phase = phases[event.phase];
              renderBuild(folder);
            }
          } catch {
            /* Human output and engine diagnostics remain in Output. */
          }
        }
        if (progressBuffer.length > 65536) progressBuffer = "";
      });
      child.on("error", (error) =>
        reject(
          new Error(
            `Cannot run texe: ${error.message}. Use texe: Choose Executable.`,
          ),
        ),
      );
      child.on("close", (code, signal) => {
        if (track && s.child === child) s.child = null;
        if (track && s.cancelled) {
          reject(cancelledError());
          return;
        }
        if (signal || exceeded) {
          reject(
            new Error(
              exceeded
                ? "texe output exceeded 8 MiB"
                : `texe stopped unexpectedly (${signal})`,
            ),
          );
          return;
        }
        try {
          const report = JSON.parse(stdout);
          output.appendLine(stdout.trim());
          if (code === 0) resolve(report);
          else {
            const error = new Error(
              report.message ||
                report.error?.message ||
                "texe failed; see output",
            );
            error.report = report;
            reject(error);
          }
        } catch (error) {
          reject(
            new Error(
              `texe returned invalid JSON (exit ${code}): ${error.message}`,
            ),
          );
        }
      });
    });
  }
  function inspectionKey(folder) {
    const program = executable(folder);
    const candidates = path.isAbsolute(program)
      ? [program]
      : program.includes(path.sep) || program.includes("/")
        ? [path.resolve(folder.uri.fsPath, program)]
        : (process.env.PATH || "")
            .split(path.delimiter)
            .flatMap((dir) =>
              [
                program,
                ...(process.platform === "win32" ? [program + ".exe"] : []),
              ].map((name) => path.resolve(folder.uri.fsPath, dir, name)),
            );
    let identity = null;
    for (const candidate of candidates) {
      try {
        const stat = fs.statSync(candidate);
        if (stat.isFile()) {
          identity = [
            candidate,
            stat.size,
            stat.mtimeMs,
            stat.ctimeMs,
            stat.ino,
          ];
          break;
        }
      } catch {
        /* An absent PATH candidate is not the selected executable. */
      }
    }
    let manifest = null;
    try {
      manifest = fs.readFileSync(
        path.join(folder.uri.fsPath, "texe.toml"),
        "utf8",
      );
    } catch {
      /* Let editor --inspect produce the actionable missing-file error. */
    }
    return JSON.stringify([program, identity, manifest]);
  }
  async function inspect(folder, sync = false) {
    const s = state(folder);
    const key = inspectionKey(folder);
    if (!sync && s.infoKey === key && s.info) return s.info;
    if (s.inspection?.key === key) {
      await s.inspection.promise;
      if (!sync) return inspect(folder);
    }
    requests.set(folder.uri.toString(), null);
    const promise = (async () => {
      const info = await run(folder, ["editor", "--inspect"]);
      if (info.schema !== "texe.editor-context/v1")
        throw new Error(
          "Update texe: this executable does not support editor context v1.",
        );
      for (const field of ["source", "pdf", "log"])
        inside(folder.uri.fsPath, info[field]);
      // A concurrent manifest/configuration edit must not publish old paths.
      if (
        disposed ||
        !vscode.workspace.workspaceFolders?.some(
          (item) => item.uri.toString() === folder.uri.toString(),
        )
      )
        throw cancelledError();
      if (inspectionKey(folder) !== key) return inspect(folder, sync);
      if (sync) await run(folder, ["editor", "--configure-only"]);
      if (inspectionKey(folder) !== key) return inspect(folder, sync);
      s.info = info;
      s.infoKey = key;
      requests.set(folder.uri.toString(), {
        source: info.source,
        pdf: info.pdf,
        request: info.source,
      });
      return info;
    })();
    s.inspection = { key, promise };
    try {
      return await promise;
    } finally {
      if (s.inspection?.promise === promise) s.inspection = null;
    }
  }
  function cancelledError() {
    const error = new Error("Build cancelled");
    error.cancelled = true;
    return error;
  }
  const phases = {
    toolchain: "Preparing LaTeX",
    packages: "Checking packages",
    format: "Preparing LaTeX format",
    "engine-discovery": "Checking document requirements",
    "engine-final": "Typesetting",
    bibliography: "Updating bibliography",
    index: "Updating index",
  };
  function renderBuild(folder) {
    const s = state(folder);
    const elapsed = ((Date.now() - s.started) / 1000).toFixed(1);
    const message = s.cancelled
      ? "Cancelling"
      : s.queued
        ? "Building latest changes"
        : s.phase;
    label(folder, `$(sync~spin) texe: ${message} · ${elapsed}s`);
  }
  async function failure(folder, error) {
    if (error.cancelled || disposed) return;
    clearDiagnostics(folder);
    state(folder).action = "texe.showError";
    output.appendLine(error.message);
    const report = error.report;
    const diagnostic = report?.diagnostic || report?.error?.diagnostic;
    const s = state(folder);
    let errorLocation;
    if (diagnostic?.location) {
      const location = diagnostic.location;
      const file = path.isAbsolute(location.file)
        ? location.file
        : path.resolve(folder.uri.fsPath, location.file);
      if (isWithin(folder.uri.fsPath, file)) {
        const line = Math.max(0, Number(location.line || 1) - 1);
        const item = new vscode.Diagnostic(
          new vscode.Range(line, 0, line, 1000),
          [diagnostic.message, diagnostic.explanation, diagnostic.action]
            .filter(Boolean)
            .join("\n"),
          vscode.DiagnosticSeverity.Error,
        );
        errorLocation = { uri: vscode.Uri.file(file), line };
        item.source = "texe";
        diagnostics.set(vscode.Uri.file(file), [item]);
      }
    }
    const info = state(folder).info;
    const previous = info && fs.existsSync(inside(folder.uri.fsPath, info.pdf));
    label(
      folder,
      previous
        ? "$(error) texe: failed · previous PDF"
        : "$(error) texe: failed",
    );
    if (previous) {
      const pdf = vscode.Uri.file(inside(folder.uri.fsPath, info.pdf));
      stalePdfs.set(pdf.toString(), {
        badge: "!",
        tooltip:
          "Build failed. This PDF is from the previous successful build.",
        color: new vscode.ThemeColor("errorForeground"),
      });
      s.stalePdf = pdf;
      decorationChanged.fire(pdf);
    }
    const signature = JSON.stringify([
      diagnostic?.message || error.message,
      diagnostic?.location?.file,
      diagnostic?.location?.line,
      diagnostic?.explanation,
    ]);
    s.lastError = { location: errorLocation, log: diagnostic?.log };
    if (s.lastFailure !== signature) {
      s.lastFailure = signature;
      const where = diagnostic?.location
        ? ` (${diagnostic.location.file}:${diagnostic.location.line})`
        : "";
      const summary = diagnostic?.message || error.message.split("\n")[0];
      const message = `texe failed${where}: ${summary} ${previous ? "Showing the PDF from the previous successful build." : "No new PDF was produced."}`;
      const actions = error.message.includes("Cannot run texe")
        ? ["Choose Executable", "Show Output"]
        : [errorLocation ? "Show Error" : "Show Output", "Open Build Log"];
      // Never hold the build queue open while a notification awaits a click.
      void vscode.window
        .showErrorMessage(message, ...actions)
        .then(async (action) => {
          if (!action) return;
          if (s.lastFailure !== signature) {
            void vscode.window.showInformationMessage(
              "The paper has changed since this error; check the current texe status.",
            );
            return;
          }
          if (action === "Show Error") await showError(folder);
          if (action === "Open Build Log") await openBuildLog(folder);
          if (action === "Show Output") output.show(true);
          if (action === "Choose Executable") await chooseExecutable(folder);
        })
        .catch((error) => output.appendLine(error.message));
    }
  }
  async function showError(folder) {
    await vscode.commands.executeCommand("workbench.action.problems.focus");
    const location = state(folder).lastError?.location;
    if (location)
      await vscode.window.showTextDocument(location.uri, {
        viewColumn: vscode.ViewColumn.One,
        selection: new vscode.Range(location.line, 0, location.line, 0),
        preview: false,
      });
  }
  async function openBuildLog(folder) {
    const log = state(folder).lastError?.log || (await inspect(folder)).log;
    const file = path.isAbsolute(log) ? log : inside(folder.uri.fsPath, log);
    if (!isWithin(folder.uri.fsPath, file))
      throw new Error("Build log is outside the paper folder.");
    await vscode.window.showTextDocument(vscode.Uri.file(file));
  }

  function clearDiagnostics(folder) {
    const remove = [];
    diagnostics.forEach((uri) => {
      if (isWithin(folder.uri.fsPath, uri.fsPath)) remove.push(uri);
    });
    remove.forEach((uri) => diagnostics.delete(uri));
  }
  function publishWarnings(folder, report) {
    clearDiagnostics(folder);
    const info = state(folder).info;
    if (!info || !report.warnings?.length) return;
    const log = inside(folder.uri.fsPath, info.log);
    let lines = [];
    try {
      lines = fs.readFileSync(log, "utf8").split(/\r?\n/);
    } catch {
      /* No log available. */
    }
    const items = report.warnings.map((warning) => {
      const line = Math.max(
        0,
        lines.findIndex((text) => text.trim() === warning.message.trim()),
      );
      const item = new vscode.Diagnostic(
        new vscode.Range(line, 0, line, 1000),
        warning.message,
        vscode.DiagnosticSeverity.Warning,
      );
      item.source = "texe";
      item.code = warning.kind;
      return item;
    });
    diagnostics.set(vscode.Uri.file(log), items);
  }

  async function ensureBridge(folder) {
    const s = state(folder);
    if (s.bridge) return;
    s.bridge = await bridge.start(
      folder,
      () => manualBuild(folder),
      () => cancel(folder),
    );
    if (disposed) {
      s.bridge.dispose();
      return;
    }
    context.subscriptions.push(s.bridge);
  }

  async function build(folder, view = false) {
    const s = state(folder);
    if (disposed) return { cancelled: true };
    if (s.running) {
      if (!s.cancelled) {
        s.queued = true;
        s.view ||= view;
        renderBuild(folder);
      }
      return s.running;
    }
    s.view = view;
    s.cancelled = false;
    s.started = Date.now();
    s.phase = "Preparing build";
    s.running = (async () => {
      let result;
      do {
        s.queued = false;
        s.phase = "Preparing build";
        renderBuild(folder);
        try {
          if (s.savingPromise) await s.savingPromise;
          await inspect(folder);
          if (s.cancelled || disposed) throw cancelledError();
          const args = ["build"];
          if (configuration(folder).get("allowDownloads", true))
            args.push("--yes");
          else args.push("--offline");
          result = await run(folder, args, true);
          if (s.cancelled || disposed) throw cancelledError();
          // Suppress intermediate success, recovery notices and PDF refreshes.
          if (s.queued) continue;
          if (s.view) await openPaper(folder, true);
          if (s.queued) continue;
          await vscode.commands.executeCommand("latex-workshop.refresh-viewer");
          if (s.cancelled || disposed) throw cancelledError();
          if (s.queued) continue;
          publishWarnings(folder, result);
          s.action = result.warning_count
            ? "texe.showProblems"
            : "texe.buildAndView";
          const recovered = Boolean(s.lastFailure);
          s.lastFailure = null;
          s.lastError = null;
          if (s.stalePdf) {
            stalePdfs.delete(s.stalePdf.toString());
            decorationChanged.fire(s.stalePdf);
            s.stalePdf = null;
          }
          label(
            folder,
            result.warning_count
              ? `$(warning) texe: ${result.warning_count} warnings`
              : `$(check) texe: ${result.cached ? "up to date" : "built"}`,
          );
          if (recovered)
            void vscode.window.showInformationMessage(
              "texe: Build succeeded. The PDF is now up to date.",
            );
        } catch (error) {
          if (s.cancelled || error.cancelled || disposed) {
            label(folder, "$(circle-slash) texe: build cancelled");
            result = { cancelled: true };
            break;
          }
          if (!s.queued) await failure(folder, error);
          result = { failed: true };
        }
      } while (s.queued && !disposed && !s.cancelled);
      return result;
    })().finally(() => {
      clearInterval(s.progressTimer);
      s.running = null;
      render(folder);
    });
    s.progressTimer = setInterval(() => renderBuild(folder), 100);
    s.progressTimer.unref?.();
    renderBuild(folder);
    return s.running;
  }
  async function manualBuild(folder, view = false) {
    const s = state(folder);
    const active = s.running;
    clearTimeout(s.timer);
    if (active) {
      if (!s.cancelled) {
        s.queued = true;
        s.view ||= view;
        renderBuild(folder);
      }
    } else s.cancelled = false;
    if (!s.savingPromise) {
      s.saving = true;
      s.savingPromise = (async () => {
        for (const doc of vscode.workspace.textDocuments) {
          if (
            doc.isDirty &&
            vscode.workspace.getWorkspaceFolder(doc.uri)?.uri.toString() ===
              folder.uri.toString()
          ) {
            if (!(await doc.save()))
              throw new Error("Save the paper before building.");
          }
        }
      })().finally(() => {
        s.saving = false;
        s.savingPromise = null;
      });
    }
    await s.savingPromise;
    if (s.cancelled) return active || { cancelled: true };
    // The active loop already queued this request and waits for saving above.
    if (active && s.running === active) return active;
    return build(folder, view);
  }
  function cancel(folder) {
    const s = state(folder);
    s.queued = false;
    s.cancelled = true;
    clearTimeout(s.timer);
    if (s.running) renderBuild(folder);
    if (s.child?.pid) {
      if (process.platform === "win32")
        spawn("taskkill", ["/pid", String(s.child.pid), "/T", "/F"], {
          windowsHide: true,
        });
      else {
        try {
          process.kill(-s.child.pid, "SIGTERM");
        } catch {
          s.child.kill();
        }
      }
    }
  }
  async function chooseExecutable(folder) {
    const files = await vscode.window.showOpenDialog({
      canSelectMany: false,
      openLabel: "Use texe executable",
    });
    if (files?.[0]) {
      await configuration(folder).update(
        "executablePath",
        files[0].fsPath,
        vscode.ConfigurationTarget.WorkspaceFolder,
      );
      await inspect(folder, true);
    }
  }
  function command(id, action) {
    context.subscriptions.push(
      vscode.commands.registerCommand(id, async () => {
        const folder = currentFolder();
        if (!folder)
          return vscode.window.showInformationMessage(
            "Open your paper folder first.",
          );
        try {
          return await action(folder);
        } catch (error) {
          await failure(folder, error);
          return { failed: true };
        }
      }),
    );
  }
  async function writingLayout(folder, restore = false) {
    const key = `texe.writingLayout:${folder.uri.toString()}`;
    const settings = vscode.workspace.getConfiguration(undefined, folder.uri);
    const target = vscode.workspace.workspaceFile
      ? vscode.ConfigurationTarget.WorkspaceFolder
      : vscode.ConfigurationTarget.Workspace;
    const scoped = (key) => {
      const values = settings.inspect(key);
      return vscode.workspace.workspaceFile
        ? values?.workspaceFolderValue
        : values?.workspaceValue;
    };
    const hidden = ["**/.texe", "**/.vscode", "**/*.synctex.gz"];
    if (restore) {
      for (const edit of context.workspaceState.get(key, [])) {
        if (edit.key === "files.exclude") {
          const current = { ...scoped(edit.key) };
          for (const pattern of hidden) {
            if (current[pattern] !== edit.after[pattern]) continue;
            if (Object.hasOwn(edit.before || {}, pattern))
              current[pattern] = edit.before[pattern];
            else delete current[pattern];
          }
          await settings.update(
            edit.key,
            Object.keys(current).length
              ? current
              : edit.before === undefined
                ? undefined
                : {},
            target,
          );
        } else if (
          JSON.stringify(scoped(edit.key)) === JSON.stringify(edit.after)
        ) {
          await settings.update(edit.key, edit.before, target);
        }
      }
      await context.workspaceState.update(key, undefined);
      return;
    }
    if (!context.workspaceState.get(key)) {
      const changes = [
        ["editor.wordWrap", "on"],
        [
          "files.exclude",
          {
            ...scoped("files.exclude"),
            ...Object.fromEntries(hidden.map((pattern) => [pattern, true])),
          },
        ],
      ];
      const edits = changes.map(([key, after]) => ({
        key,
        before: scoped(key),
        after,
      }));
      await context.workspaceState.update(key, edits);
      for (const edit of edits)
        await settings.update(edit.key, edit.after, target);
    }
    await vscode.commands.executeCommand("workbench.action.closeAuxiliaryBar");
    await vscode.commands.executeCommand("outline.focus");
    await openPaper(folder, true);
  }
  command("texe.writingLayout", (folder) => writingLayout(folder));
  command("texe.restoreWritingLayout", (folder) => writingLayout(folder, true));
  command("texe.forwardSync", () =>
    vscode.commands.executeCommand("latex-workshop.synctex"),
  );
  command("texe.gettingStarted", () =>
    vscode.commands.executeCommand(
      "workbench.action.openWalkthrough",
      "backmatter.texe-paper-layout#texe.startWriting",
      false,
    ),
  );
  command("texe.build", (folder) => manualBuild(folder));
  command("texe.buildAndView", (folder) => manualBuild(folder, true));
  command("texe.cancelBuild", async (folder) => cancel(folder));
  command("texe.showProblems", () =>
    vscode.commands.executeCommand("workbench.action.problems.focus"),
  );
  command("texe.showError", showError);
  command("texe.showOutput", async () => output.show(true));
  command("texe.chooseExecutable", chooseExecutable);
  command("texe.checkSetup", async (folder) => {
    await inspect(folder, true);
    output.show(true);
    const report = await run(folder, ["doctor", "--offline"]);
    await vscode.window.showInformationMessage(
      "texe setup is ready (offline check passed).",
    );
    return report;
  });
  command("texe.openBuildLog", openBuildLog);
  command("texe.enableDownloads", async (folder) => {
    const choice = await vscode.window.showInformationMessage(
      "Allow texe builds in this folder to download the managed TeX runtime and required packages?",
      { modal: true },
      "Allow Downloads",
    );
    if (choice === "Allow Downloads")
      await configuration(folder).update(
        "allowDownloads",
        true,
        vscode.ConfigurationTarget.WorkspaceFolder,
      );
  });
  command("texe.enableFamiliarShortcuts", (folder) =>
    configuration(folder).update(
      "editor.familiarShortcuts",
      true,
      vscode.ConfigurationTarget.WorkspaceFolder,
    ),
  );
  context.subscriptions.push(
    output,
    diagnostics,
    status,
    vscode.window.onDidChangeActiveTextEditor(() => {
      const folder = currentFolder();
      if (folder) render(folder);
      else status.hide();
    }),
    vscode.workspace.onDidSaveTextDocument((document) =>
      inputChanged(document.uri),
    ),
    {
      dispose() {
        disposed = true;
        for (const s of states.values()) {
          cancel(s.folder);
        }
      },
    },
  );
  const inputStamps = new Map();
  function inputChanged(uri) {
    const folder = vscode.workspace.getWorkspaceFolder(uri);
    if (!folder || !configuration(folder).get("editor.enabled", false)) return;
    const relative = path.relative(folder.uri.fsPath, uri.fsPath);
    if (
      !relative ||
      relative.startsWith(".." + path.sep) ||
      path.isAbsolute(relative)
    )
      return;
    if (
      [".texe", ".git", ".vscode", "node_modules", "target"].includes(
        relative.split(path.sep)[0],
      ) ||
      relative === "texe.lock"
    )
      return;
    const info = state(folder).info;
    if (
      info &&
      [
        info.pdf,
        info.pdf.replace(/\.pdf$/, ".synctex.gz"),
        info.pdf.replace(/\.pdf$/, ".dvi"),
      ].includes(relative.split(path.sep).join("/"))
    )
      return;
    let stamp = "deleted";
    try {
      const stat = fs.statSync(uri.fsPath);
      if (!stat.isFile()) return;
      stamp = JSON.stringify([stat.size, stat.mtimeMs, stat.ctimeMs, stat.ino]);
    } catch {
      /* File deletion is an input change too. */
    }
    const key = uri.toString();
    if (inputStamps.get(key) === stamp) return;
    if (inputStamps.size > 4096) inputStamps.clear();
    inputStamps.set(key, stamp);
    if (relative === "texe.toml") {
      refreshManifest(uri);
      return;
    }
    if (state(folder).saving || !configuration(folder).get("buildOnSave", true))
      return;
    if (state(folder).running) {
      void build(folder);
      return;
    }
    clearTimeout(state(folder).timer);
    state(folder).timer = setTimeout(() => {
      void build(folder);
    }, 250);
  }
  const inputWatcher = vscode.workspace.createFileSystemWatcher("**/*");
  context.subscriptions.push(
    inputWatcher,
    inputWatcher.onDidChange(inputChanged),
    inputWatcher.onDidCreate(inputChanged),
    inputWatcher.onDidDelete(inputChanged),
  );

  const manifestWatcher =
    vscode.workspace.createFileSystemWatcher("**/texe.toml");
  function refreshManifest(uri) {
    const folder = vscode.workspace.getWorkspaceFolder(uri);
    if (
      !folder ||
      uri.fsPath !== path.join(folder.uri.fsPath, "texe.toml") ||
      !configuration(folder).get("editor.enabled", false)
    )
      return;
    state(folder).infoKey = null;
    if (state(folder).running) void build(folder);
    clearTimeout(state(folder).manifestTimer);
    state(folder).manifestTimer = setTimeout(() => {
      void inspect(folder, true)
        .then(() => openPaper(folder, true))
        .catch((error) => failure(folder, error));
    }, 250);
  }
  context.subscriptions.push(
    manifestWatcher,
    manifestWatcher.onDidChange(refreshManifest),
    manifestWatcher.onDidCreate(refreshManifest),
    manifestWatcher.onDidDelete(refreshManifest),
    {
      dispose() {
        for (const s of states.values()) clearTimeout(s.manifestTimer);
      },
    },
  );
  function buildTask(folder) {
    const task = new vscode.Task(
      { type: "texe" },
      folder,
      "Build Paper",
      "texe",
      new vscode.CustomExecution(async () => {
        const write = new vscode.EventEmitter();
        const close = new vscode.EventEmitter();
        return {
          onDidWrite: write.event,
          onDidClose: close.event,
          open() {
            write.fire(
              "Building paper. Progress and diagnostics appear in texe Output and Problems.\r\n",
            );
            void manualBuild(folder)
              .then((report) =>
                close.fire(report?.cancelled ? 130 : report?.failed ? 1 : 0),
              )
              .catch((error) => {
                write.fire(error.message + "\r\n");
                close.fire(1);
              });
          },
          close() {
            cancel(folder);
            write.dispose();
            close.dispose();
          },
        };
      }),
    );
    task.group = vscode.TaskGroup.Build;
    return task;
  }
  context.subscriptions.push(
    vscode.tasks.registerTaskProvider("texe", {
      provideTasks() {
        return (vscode.workspace.workspaceFolders || [])
          .filter((folder) =>
            configuration(folder).get("editor.enabled", false),
          )
          .map(buildTask);
      },
      resolveTask(task) {
        const folder = task.scope;
        return folder &&
          typeof folder === "object" &&
          configuration(folder).get("editor.enabled", false)
          ? buildTask(folder)
          : undefined;
      },
    }),
  );
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      for (const folder of vscode.workspace.workspaceFolders || []) {
        if (event.affectsConfiguration("texe.executablePath", folder.uri)) {
          state(folder).infoKey = null;
          if (state(folder).running) void build(folder);
        }
      }
    }),
    vscode.workspace.onDidChangeWorkspaceFolders((event) => {
      for (const folder of event.removed) {
        const s = state(folder);
        cancel(folder);
        s.bridge?.dispose();
        clearTimeout(s.manifestTimer);
        clearDiagnostics(folder);
        if (s.stalePdf) {
          stalePdfs.delete(s.stalePdf.toString());
          decorationChanged.fire(s.stalePdf);
        }
        requests.delete(folder.uri.toString());
        states.delete(folder.uri.toString());
      }
      for (const folder of event.added) {
        if (configuration(folder).get("editor.enabled", false))
          void inspect(folder)
            .then(() => ensureBridge(folder))
            .then(() => render(folder))
            .catch((error) => failure(folder, error));
      }
    }),
  );
  const ready = Promise.all(
    (vscode.workspace.workspaceFolders || [])
      .filter((folder) => configuration(folder).get("editor.enabled", false))
      .map(async (folder) => {
        try {
          await inspect(folder);
          await ensureBridge(folder);
          render(folder);
          const initialError = path.join(
            folder.uri.fsPath,
            ".texe/editor/adoption-error.json",
          );
          try {
            if (fs.statSync(initialError).size <= 1024 * 1024) {
              const report = JSON.parse(fs.readFileSync(initialError, "utf8"));
              if (report.schema === "texe.error/v1") {
                fs.unlinkSync(initialError);
                const error = new Error(
                  report.error?.message || "The first build failed",
                );
                error.report = report;
                await failure(folder, error);
              }
            }
          } catch {
            /* No adoption error is pending. */
          }
        } catch (error) {
          await failure(folder, error);
        }
      }),
  );
  void ready.then(() => {
    const folder = currentFolder();
    if (
      !disposed &&
      folder &&
      configuration(folder).get("editor.enabled", false) &&
      context.workspaceState &&
      !context.workspaceState.get("texe.welcome.v1")
    ) {
      void context.workspaceState.update("texe.welcome.v1", true);
      void vscode.window
        .showInformationMessage(
          "Your paper is ready to edit. Save to build; texe installs missing TeX tools and packages automatically.",
          "Show Writing Guide",
        )
        .then((action) => {
          if (action)
            return vscode.commands.executeCommand("texe.gettingStarted");
        });
    }
  });
  return { ready, inspect, build, reportFailure: failure };
}
module.exports = { activate, inside, isWithin };
