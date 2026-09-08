const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const {
  runTests,
  downloadAndUnzipVSCode,
  resolveCliArgsFromVSCodeExecutablePath,
} = require("@vscode/test-electron");

(async () => {
  delete process.env.ELECTRON_RUN_AS_NODE;
  const repo = path.resolve(__dirname, "../..");
  const root =
    process.env.TEXE_TEST_REUSE ||
    fs.mkdtempSync(path.join(os.tmpdir(), "texe-vscode-"));
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
  fs.cpSync(path.join(repo, "assets/vscode-extension"), extension, {
    recursive: true,
  });
  const pkg = path.join(extension, "package.json");
  fs.writeFileSync(
    pkg,
    fs.readFileSync(pkg, "utf8").replace("@VERSION@", "0.1.2"),
  );
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
    PATH: path.join(root, "bin") + path.delimiter + process.env.PATH,
  };
  execFileSync(texe, ["editor", "--project", paper, "--configure-only"], {
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
  if (process.env.LATEX_WORKSHOP_PATH) {
    fs.cpSync(
      process.env.LATEX_WORKSHOP_PATH,
      path.join(extensions, "james-yu.latex-workshop-10.17.1"),
      { recursive: true },
    );
  } else {
    const [cli, ...args] = resolveCliArgsFromVSCodeExecutablePath(executable);
    execFileSync(
      cli,
      [
        ...args,
        `--user-data-dir=${profile}`,
        `--extensions-dir=${extensions}`,
        "--install-extension",
        "James-Yu.latex-workshop@10.17.1",
      ],
      { stdio: "inherit" },
    );
  }
  console.log(`Real VS Code test workspace: ${root}`);
  fs.writeFileSync(path.join(os.tmpdir(), "texe-vscode-latest"), root);
  await runTests({
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
      "--remote-debugging-port=9337",
    ],
  });
})().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
