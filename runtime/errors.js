// Pi hooks only enqueue allowlisted error kinds. A worker owns all delivery.
import { Worker } from "node:worker_threads";

export function install(pi, makeWorker = () => new Worker(new URL("./error-worker.js", import.meta.url))) {
  let worker;
  let pending = 0;
  let failed = false;
  let count = 0;
  let windowStart = Date.now();
  function enqueue(kind) {
    if (failed || pending >= 32) return;
    const now = Date.now();
    if (now - windowStart >= 60_000) { windowStart = now; count = 0; }
    if (count >= 60) return;
    try {
      if (!worker) {
        worker = makeWorker();
        worker.on("message", () => { pending = Math.max(0, pending - 1); });
        worker.on("error", () => { failed = true; });
        worker.on("exit", () => { failed = true; });
        worker.unref();
      }
      worker.postMessage({ kind });
      pending++;
      count++;
    } catch { failed = true; }
  }
  pi.on("session_shutdown", () => {
    failed = true;
    if (worker) void worker.terminate().catch(() => {});
  });
  pi.on("message_end", event => {
    if (event.message?.role === "assistant" && event.message.stopReason === "error") enqueue("model");
  });
  pi.on("tool_execution_end", event => {
    if (event.isError) enqueue("tool");
  });
  pi.on("session_compact_failed", event => {
    if (!event.aborted) enqueue("compaction");
  });
}

export default install;
