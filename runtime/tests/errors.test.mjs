import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { createServer } from "node:http";
import { once } from "node:events";
import { test } from "node:test";
import { install } from "../errors.js";
import { submit } from "../error-worker.js";

function harness() {
  const hooks = new Map();
  const worker = new EventEmitter();
  const events = [];
  worker.unref = () => {};
  worker.terminate = async () => {};
  worker.postMessage = event => events.push(event);
  install({ on: (name, handler) => hooks.set(name, handler) }, () => worker);
  return { hooks, worker, events };
}

test("Pi hooks enqueue only structured failures without awaiting delivery", () => {
  const { hooks, events } = harness();
  const message = { role: "assistant", stopReason: "error", errorMessage: "secret", content: "private prompt" };
  assert.equal(hooks.get("message_end")({ message }), undefined);
  hooks.get("tool_execution_end")({ isError: true, args: "secret", result: "private output" });
  hooks.get("session_compact_failed")({ aborted: false, errorMessage: "private" });
  hooks.get("message_end")({ message: { ...message, stopReason: "aborted" } });
  hooks.get("tool_execution_end")({ isError: false });
  hooks.get("session_compact_failed")({ aborted: true });
  assert.deepEqual(events, [{ kind: "model" }, { kind: "tool" }, { kind: "compaction" }]);
});

test("unavailable delivery stays bounded and worker failure never escapes a hook", () => {
  const { hooks, worker, events } = harness();
  for (let i = 0; i < 1000; i++) hooks.get("tool_execution_end")({ isError: true });
  assert.equal(events.length, 32);
  worker.emit("message", "done");
  hooks.get("tool_execution_end")({ isError: true });
  assert.equal(events.length, 33);
  worker.emit("error", new Error("offline"));
  assert.doesNotThrow(() => hooks.get("tool_execution_end")({ isError: true }));
  assert.equal(events.length, 33);
});

test("worker authenticates to the relay and does not follow redirects", async () => {
  const received = [];
  const server = createServer(async (req, res) => {
    let body = "";
    for await (const part of req) body += part;
    received.push({ path: req.url, auth: req.headers.authorization, body: JSON.parse(body) });
    res.writeHead(302, { location: "/do-not-follow" }).end();
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  try {
    const status = await submit({ origin: `http://127.0.0.1:${server.address().port}`, token: "scoped" }, { kind: "model" });
    assert.equal(status, 302);
    assert.deepEqual(received, [{ path: "/v1/harness/agent/errors", auth: "Bearer scoped", body: { kind: "model" } }]);
  } finally { server.closeAllConnections(); server.close(); }
});


test("a stalled relay times out without blocking other work", async () => {
  const server = createServer(() => {});
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  let ticks = 0;
  const timer = setInterval(() => { ticks++; }, 10);
  try {
    await assert.rejects(submit({ origin: `http://127.0.0.1:${server.address().port}`, token: "scoped" }, { kind: "tool" }), /report timeout/);
    assert.ok(ticks > 20, "other work must continue during the request");
  } finally { clearInterval(timer); server.closeAllConnections(); server.close(); }
});

test("Pi reload releases the worker without waiting for delivery", () => {
  const { hooks, worker, events } = harness();
  let terminated = false;
  worker.terminate = async () => { terminated = true; };
  hooks.get("tool_execution_end")({ isError: true });
  assert.equal(hooks.get("session_shutdown")({ reason: "reload" }), undefined);
  assert.equal(terminated, true);
  hooks.get("tool_execution_end")({ isError: true });
  assert.equal(events.length, 1);
});
