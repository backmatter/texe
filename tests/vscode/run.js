const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync, spawn } = require("node:child_process");
const {
  runTests,
  downloadAndUnzipVSCode,
} = require("@vscode/test-electron");

(async () => {
  delete process.env.ELECTRON_RUN_AS_NODE;
  const repo = path.resolve(__dirname, "../..");
  const scratch =
    process.env.TEXE_TEST_REUSE ||
    fs.mkdtempSync(path.join(os.tmpdir(), "texe-vscode-"));
  // Windows TEMP can use DOS 8.3 names, and macOS /var can be an alias.
  // Give the editor the same canonical directory that the TeX engine records.
  fs.mkdirSync(scratch, { recursive: true });
  const root = fs.realpathSync.native(scratch);
  if (process.env.GITHUB_ENV)
    fs.appendFileSync(process.env.GITHUB_ENV, `TEXE_EDITOR_EVIDENCE=${root}\n`);
  const paper = path.join(root, "paper with spaces");
  const extension = path.join(root, "extension");
  const extensions = path.join(root, "extensions");
  const profile = path.join(root, "profile");
  for (const dir of [
    paper,
    extensions,
    path.join(profile, "User"),
    path.join(root, "bin"),
  ])
    fs.mkdirSync(dir, { recursive: true });
  const { unzipSync } = require("fflate");
  const files = unzipSync(fs.readFileSync(path.join(repo, "assets/citet-vscode.vsix")));
  for (const [name, bytes] of Object.entries(files)) {
    if (!name.startsWith("extension/")) continue;
    const target = path.join(extension, name.slice("extension/".length));
    fs.mkdirSync(path.dirname(target), { recursive: true }); fs.writeFileSync(target, bytes);
  }
  const suite = process.env.TEXE_TEST_SUITE || path.join(repo, "target/debug");
  for (const name of ["texe", "pqty", "pqty-fls"]) {
    const binary = name + (process.platform === "win32" ? ".exe" : "");
    fs.copyFileSync(path.join(suite, binary), path.join(root, "bin", binary));
  }
  fs.writeFileSync(
    path.join(paper, "texe.toml"),
    process.env.TEXE_TEST_MANAGED === "1"
      ? 'schema = "texe.project/v1"\n[project]\nentry = "paper.v2.tex"\n[toolchain]\nengine = "pdflatex"\n'
      : 'schema = "texe.project/v1"\n[project]\nentry = "paper.v2.tex"\n[toolchain]\nprovider = "system"\nengine = "pdflatex"\n[packages]\nremote = false\n',
  );
  fs.writeFileSync(
    path.join(paper, "paper.v2.tex"),
    "\\documentclass{article}\n\\begin{document}\nA real paper for editor verification.\n\\end{document}\n",
  );
  const texe = path.join(
    root,
    "bin",
    process.platform === "win32" ? "texe.exe" : "texe",
  );
  const environment = {
    ...process.env,
    TEXE_HOME: path.join(root, "texe-home"),
    XDG_CACHE_HOME: path.join(root, "cache"),
    PATH: process.env.TEXE_TEST_DOWNLOAD_TOOLS === "1" ? process.env.PATH : path.join(root, "bin") + path.delimiter + process.env.PATH,
  };
  execFileSync(texe, ["editor", "--project", paper, "--configure-only", "--replace-conflicts"], {
    env: environment,
  });
  fs.writeFileSync(
    path.join(profile, "User/settings.json"),
    JSON.stringify({
      "workbench.startupEditor": "none",
      "telemetry.telemetryLevel": "off",
      "update.mode": "none",
      "extensions.autoUpdate": false,
      "security.workspace.trust.enabled": false,
    }),
  );
  const executable =
    process.env.VSCODE_EXECUTABLE || (await downloadAndUnzipVSCode("1.126.0"));
  if (process.env.TEXE_TEST_DOWNLOAD_TOOLS === "1" || process.env.TEX_LS_BINARY) {
    const settings = JSON.parse(fs.readFileSync(path.join(paper, ".vscode/settings.json")));
    if (process.env.TEXE_TEST_DOWNLOAD_TOOLS === "1") { settings["citet.texePath"] = ""; settings["citet.texLsPath"] = ""; }
    else settings["citet.texLsPath"] = process.env.TEX_LS_BINARY;
    fs.writeFileSync(path.join(paper, ".vscode/settings.json"), JSON.stringify(settings, null, 2));
  }
  console.log(`Real VS Code test workspace: ${root}`);
  fs.writeFileSync(path.join(os.tmpdir(), "texe-vscode-latest"), root);
  const options = {
    vscodeExecutablePath: executable,
    extensionDevelopmentPath: extension,
    extensionTestsPath: path.join(__dirname, "suite.js"),
    extensionTestsEnv: { ...environment, TEXE_TEST_ROOT: root },
    launchArgs: [
      paper,
      `--user-data-dir=${profile}`,
      `--extensions-dir=${extensions}`,
      "--disable-workspace-trust",
      "--skip-welcome",
      "--skip-release-notes",
      "--disable-gpu",
    ],
  };
  if (process.platform === "win32") {
    // test-electron 2.x uses a shell on Windows and splits workspace paths
    // containing spaces. Code.exe accepts an ordinary argument array.
    await new Promise((resolve, reject) => {
      const child = spawn(
        executable,
        [
          ...options.launchArgs,
          "--no-sandbox",
          "--disable-gpu-sandbox",
          "--disable-updates",
          `--extensionDevelopmentPath=${options.extensionDevelopmentPath}`,
          `--extensionTestsPath=${options.extensionTestsPath}`,
        ],
        { env: options.extensionTestsEnv, stdio: "inherit" },
      );
      child.once("error", reject);
      child.once("exit", (code) => {
        if (code === 0) resolve();
        else reject(new Error(`VS Code tests exited with code ${code}`));
      });
    });
  } else {
    await runTests(options);
  }
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
