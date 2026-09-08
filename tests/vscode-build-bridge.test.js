const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const net = require("node:net");
const bridge = require("../assets/vscode-extension/build-bridge");

function request(endpoint, token) {
  return new Promise((resolve, reject) => {
    const socket = net.connect(endpoint.port, "127.0.0.1", () =>
      socket.write(JSON.stringify({ token }) + "\n"),
    );
    let result = "";
    socket.on("data", (data) => (result += data));
    socket.on("end", () => resolve(result));
    socket.on("error", reject);
  });
}
test("Workshop bridge requires its token, returns the build result and removes its endpoint", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "texe-bridge-"));
  let builds = 0;
  const server = await bridge.start({ uri: { fsPath: root } }, async () => {
    builds++;
    return { failed: true };
  });
  const file = path.join(root, ".texe/editor/build-bridge.json");
  try {
    const endpoint = JSON.parse(fs.readFileSync(file));
    assert.equal(await request(endpoint, "wrong-token"), "");
    assert.equal(builds, 0);
    assert.equal(JSON.parse(await request(endpoint, endpoint.token)).ok, false);
    assert.equal(builds, 1);
  } finally {
    server.dispose();
    assert.equal(fs.existsSync(file), false);
    fs.rmSync(root, { recursive: true, force: true });
  }
});

test("stopping the Workshop client cancels its queued build", async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "texe-bridge-cancel-"));
  let started;
  const entered = new Promise((resolve) => {
    started = resolve;
  });
  let cancelled;
  const stopped = new Promise((resolve) => {
    cancelled = resolve;
  });
  const server = await bridge.start(
    { uri: { fsPath: root } },
    () => {
      started();
      return new Promise(() => {});
    },
    cancelled,
  );
  try {
    const endpoint = JSON.parse(
      fs.readFileSync(path.join(root, ".texe/editor/build-bridge.json")),
    );
    const socket = net.connect(endpoint.port, "127.0.0.1", () =>
      socket.write(JSON.stringify({ token: endpoint.token }) + "\n"),
    );
    await entered;
    socket.destroy();
    await stopped;
  } finally {
    server.dispose();
    fs.rmSync(root, { recursive: true, force: true });
  }
});
