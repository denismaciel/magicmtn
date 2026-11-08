import assert from "node:assert/strict";
import test from "node:test";
import { createAssistantMessageEventStream, createModels } from "@earendil-works/pi-ai";
import { createInferenceProvider } from "../inference.js";

const entries = [
  { id: "alpha", display_name: "Alpha", provider: "fireworks" },
  { id: "beta", display_name: "Beta", provider: "baseten" },
];
function grant(id, suffix = "1") {
  return { baseUrl: "https://gateway.example/v1/harness/agent", api: "openai-completions", apiKey: `scoped-${id}-${suffix}`, expiresIn: 120,
    models: [{ id, name: id, contextWindow: 65536, maxTokens: 8192, compat: { supportsStore: false } }] };
}
function fixture() {
  let time = 1000;
  const requests = [];
  const sent = [];
  const api = Object.fromEntries(["stream", "streamSimple"].map((method) => [method, (model, context, options) => {
    sent.push({ method, model, context, options });
    const stream = createAssistantMessageEventStream();
    stream.push({ type: "done", reason: "stop", message: { role: "assistant", api: model.api, provider: model.provider, model: model.id,
      content: [{ type: "text", text: "ok" }], stopReason: "stop", timestamp: 1000,
      usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } } } });
    return stream;
  }]));
  const inference = createInferenceProvider(async (args, signal) => {
    requests.push({ args, signal });
    return grant(args.at(-1), String(requests.length));
  }, { now: () => time, api });
  inference.load(entries);
  return { inference, requests, sent, advance(ms) { time += ms; } };
}

test("Pi's real model registry lists the catalog without requesting credentials", async () => {
  const f = fixture();
  const models = createModels();
  models.setProvider(f.inference.provider);
  assert.deepEqual((await models.getAvailable()).map((model) => model.id), ["fireworks/alpha:alpha", "baseten/beta:beta"]);
  assert.equal(f.requests.length, 0);
  f.inference.clear();
  assert.deepEqual(await models.getAvailable(), []);
});

test("each selected model uses its own scoped grant and Pi's existing adapter", async () => {
  const f = fixture();
  const context = { messages: [], tools: [] };
  const controller = new AbortController();
  const onPayload = (payload) => ({ ...payload, tracked: true });
  for (const [index, model] of f.inference.provider.getModels().entries()) {
    const method = index === 0 ? "stream" : "streamSimple";
    await f.inference.provider[method](model, context, { signal: controller.signal, onPayload, headers: { Authorization: "Bearer other-model", "X-Trace": "trace" } }).result();
    const sent = f.sent.at(-1);
    assert.equal(sent.method, method);
    assert.equal(sent.model.id, model.id);
    assert.equal(sent.model.baseUrl, grant(model.id).baseUrl);
    assert.equal(sent.model.contextWindow, 65536);
    assert.equal(sent.options.apiKey, `scoped-${entries[index].id}-${index + 1}`);
    assert.equal(sent.options.signal, controller.signal);
    assert.deepEqual(await sent.options.onPayload({ model: model.id }), { model: entries[index].id, tracked: true });
    assert.deepEqual(sent.options.headers, { "X-Trace": "trace" });
    assert.equal(sent.context, context);
  }
  assert.deepEqual(f.requests.map(({ args }) => args), [["pi", "inference", "--model", "alpha"], ["pi", "inference", "--model", "beta"]]);
});

test("grants cache by model and renew before expiry", async () => {
  const f = fixture();
  const model = f.inference.provider.getModels()[0];
  await f.inference.provider.streamSimple(model, {}).result();
  await f.inference.provider.streamSimple(model, {}).result();
  assert.equal(f.requests.length, 1);
  f.advance(61000);
  await f.inference.provider.streamSimple(model, {}).result();
  assert.equal(f.requests.length, 2);
  assert.equal(f.sent.at(-1).options.apiKey, "scoped-alpha-2");
});

test("sign-out during grant acquisition prevents dispatch and caching", async () => {
  let finish;
  let dispatched = false;
  const inference = createInferenceProvider(() => new Promise((resolve) => { finish = resolve; }), { api: { streamSimple() { dispatched = true; } } });
  inference.load(entries);
  const stream = inference.provider.streamSimple(inference.provider.getModels()[0], {});
  inference.clear();
  finish(grant("alpha"));
  const result = await stream.result();
  assert.equal(result.stopReason, "error");
  assert.match(result.errorMessage, /account models changed/);
  assert.equal(dispatched, false);
  assert.equal(await inference.provider.auth.apiKey.resolve(), undefined);
});

test("cancelled grants and unknown models fail before dispatch", async () => {
  const f = fixture();
  const controller = new AbortController();
  controller.abort();
  const result = await f.inference.provider.streamSimple(f.inference.provider.getModels()[0], {}, { signal: controller.signal }).result();
  assert.equal(result.stopReason, "error");
  assert.equal(f.requests.length, 0);
  assert.equal(f.sent.length, 0);
  await assert.rejects(f.inference.authorize("unknown"), /available model/);
});

test("a grant for a different model is rejected", async () => {
  const inference = createInferenceProvider(async () => grant("beta"));
  inference.load(entries);
  await assert.rejects(inference.authorize("alpha"), /Invalid MagicMtn inference configuration/);
});
