// This thread owns configuration I/O, bounded retries, and network timeouts.
import { parentPort } from "node:worker_threads";
import { readFile } from "node:fs/promises";
import { request } from "node:http";
import { request as secureRequest } from "node:https";

const queue = [];
let running = false;
let config;

export function submit(config, event) {
  return new Promise((resolve, reject) => {
    const url = new URL("/v1/harness/agent/errors", config.origin);
    const data = JSON.stringify(event);
    const send = url.protocol === "https:" ? secureRequest : request;
    const req = send(url, {
      method: "POST", ca: config.ca_certificate || undefined,
      headers: { authorization: `Bearer ${config.token}`, "content-type": "application/json", "content-length": Buffer.byteLength(data) },
    }, response => {
      response.resume();
      resolve(response.statusCode);
    });
    const deadline = setTimeout(() => req.destroy(new Error("report timeout")), 2000);
    req.on("close", () => clearTimeout(deadline));
    req.on("error", reject);
    req.end(data);
  });
}

async function drain() {
  if (running) return;
  running = true;
  try {
    config ??= JSON.parse(await readFile(new URL("./config.json", import.meta.url), "utf8"));
    while (queue.length) {
      const event = queue.shift();
      for (let attempt = 0; attempt < 3; attempt++) {
        try {
          const status = await submit(config, event);
          if (status < 500) break; // Never follow redirects or retry a rejected grant.
        } catch { /* Reporting never changes Pi behavior. */ }
        if (attempt < 2) await new Promise(resolve => setTimeout(resolve, 250 * (attempt + 1)));
      }
      parentPort.postMessage("done");
    }
  } catch {
    while (queue.length) { queue.shift(); parentPort.postMessage("done"); }
  } finally { running = false; }
}

parentPort?.on("message", event => {
  if (!["model", "tool", "compaction"].includes(event.kind) || queue.length >= 32) {
    parentPort.postMessage("done");
    return;
  }
  queue.push({ kind: event.kind });
  void drain();
});
