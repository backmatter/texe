const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const source = fs
  .readFileSync(
    require("node:path").join(__dirname, "../src/viewer.rs"),
    "utf8",
  )
  .match(/const TEXE_BRIDGE_JS: &str = r##"([\s\S]*?)"##;/)[1];

test("the browser marks failed builds and disconnection without replacing the previous PDF", async () => {
  let status = { generation: 1, state: "ready" };
  let tick, banner;
  let opens = 0;
  const app = {
    initializedPromise: Promise.resolve(),
    pdfDocument: {},
    open: () => {
      opens++;
    },
  };
  const document = {
    documentElement: { dataset: {} },
    getElementById: () => banner,
    createElement: () => ({ style: {}, setAttribute() {} }),
    body: {
      appendChild: (node) => {
        banner = node;
      },
    },
  };
  vm.runInNewContext(source, {
    document,
    window: { PDFViewerApplication: app },
    setInterval: (callback) => {
      tick = callback;
    },
    fetch: async () => {
      if (!status) throw new Error("closed");
      return { ok: true, json: async () => status };
    },
    AbortSignal,
    console: { warn() {} },
  });
  await new Promise((resolve) => setImmediate(resolve));
  assert.match(banner.textContent, /up to date/);
  status = { generation: 1, state: "failed" };
  await tick();
  assert.match(banner.textContent, /previous successful build/);
  status = null;
  await tick();
  assert.match(banner.textContent, /disconnected/);
  assert.equal(opens, 0);
});
