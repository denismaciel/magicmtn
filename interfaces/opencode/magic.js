import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { executable } from "./config.js";

export { executable };
const run = promisify(execFile);

async function request(args, { input, timeout = 90000 } = {}) {
  const pending = run(executable, args, { timeout, maxBuffer: 4 * 1024 * 1024 });
  const stdin = pending.child.stdin;
  stdin.on("error", () => {});
  stdin.end(input);
  const { stdout } = await pending;
  return JSON.parse(stdout);
}

export function magic(args) {
  return request(args);
}

export async function sandboxTool(name, args = {}) {
  const result = await request(["--local", "mcp", "call", name], {
    input: JSON.stringify(args), timeout: 35000,
  });
  if (result.isError) throw new Error(result.content[0].text);
  return JSON.parse(result.content[0].text);
}
