// Project-local bridge for LaTeX Workshop's external build process. All editor
// entry points use the companion's save/queue/cancel/diagnostic behavior.
const net = require("node:net");
const fs = require("node:fs");
const path = require("node:path");
const crypto = require("node:crypto");

async function start(folder, build, cancel = () => {}) {
  const token = crypto.randomBytes(32).toString("hex");
  const sockets = new Set();
  const server = net.createServer((socket) => {
    sockets.add(socket);
    let authorized = false,
      finished = false;
    socket.on("close", () => {
      sockets.delete(socket);
      if (authorized && !finished) cancel();
    });
    socket.on("error", () => {});
    socket.setTimeout(5000, () => socket.destroy());
    let buffer = "";
    let accepted = false;
    socket.on("data", (data) => {
      if (accepted) return;
      buffer += data.toString("utf8");
      if (buffer.length > 4096) return socket.destroy();
      const end = buffer.indexOf("\n");
      if (end < 0) return;
      accepted = true;
      let request;
      try {
        request = JSON.parse(buffer.slice(0, end));
      } catch {
        return socket.destroy();
      }
      if (request.token !== token) return socket.destroy();
      authorized = true;
      socket.setTimeout(0);
      Promise.resolve()
        .then(build)
        .then(
          (report) => {
            finished = true;
            socket.end(
              JSON.stringify({
                ok: !report?.failed && !report?.cancelled,
                cancelled: Boolean(report?.cancelled),
              }) + "\n",
            );
          },
          (error) => {
            finished = true;
            socket.end(
              JSON.stringify({ ok: false, message: error.message }) + "\n",
            );
          },
        );
    });
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  server.on("error", () => {});
  const root = fs.realpathSync(folder.uri.fsPath);
  let directory = root;
  let file;
  try {
    for (const name of [".texe", "editor"]) {
      directory = path.join(directory, name);
      fs.mkdirSync(directory, { recursive: true });
      if (
        fs.lstatSync(directory).isSymbolicLink() ||
        !fs.statSync(directory).isDirectory()
      )
        throw new Error("texe editor state must be a project-local directory");
    }
    file = path.join(directory, "build-bridge.json");
    const temporary = path.join(directory, `bridge-${token}.tmp`);
    try {
      fs.writeFileSync(
        temporary,
        JSON.stringify({ port: server.address().port, token }),
        { mode: 0o600, flag: "wx" },
      );
      fs.renameSync(temporary, file);
    } finally {
      fs.rmSync(temporary, { force: true });
    }
  } catch (error) {
    server.close();
    throw error;
  }
  return {
    dispose() {
      for (const socket of sockets) socket.destroy();
      server.close();
      try {
        if (JSON.parse(fs.readFileSync(file, "utf8")).token === token)
          fs.unlinkSync(file);
      } catch {
        /* A newer extension host may own the bridge now. */
      }
    },
  };
}
module.exports = { start };
