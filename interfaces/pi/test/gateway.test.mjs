import test from "node:test";
import assert from "node:assert/strict";
import { connectPiModel, workspaceModels } from "../index.js";

function context(overrides = {}) {
  return {
    model: { provider: "example", id: "chosen-model", api: "openai-completions", baseUrl: "https://api.example.com/v1" },
    ui: { confirm: async () => true },
    modelRegistry: { isUsingOAuth: () => false, getApiKeyAndHeaders: async () => ({ ok: true, apiKey: "test-user-key" }) },
    ...overrides,
  };
}
test("Pi-selected inference is first and independent of the sandbox catalog", () => {
  const models = workspaceModels(context(), { models: [{ id: "managed", provider: "magicmtn" }] });
  assert.equal(models[0].id, "__pi_current__");
  assert.equal(models[1].id, "managed");
});
test("only the selected API-key credential is connected after explicit confirmation", async () => {
  let uploaded;
  const result = await connectPiModel(context(), async (value) => { uploaded = value; return { id: "connection" }; });
  assert.deepEqual(result, { id: "connection" });
  assert.equal(uploaded.model.id, "chosen-model");
  assert.equal(uploaded.api_key, "test-user-key");
  assert.deepEqual(Object.keys(uploaded).sort(), ["api_key", "model"]);
});
test("declining connection never resolves or uploads a credential", async () => {
  let read = false;
  const ctx = context({ ui: { confirm: async () => false }, modelRegistry: { isUsingOAuth: () => false, getApiKeyAndHeaders: () => { read = true; } } });
  await assert.rejects(connectPiModel(ctx, () => assert.fail("uploaded")), /cancelled/);
  assert.equal(read, false);
});
test("OAuth is not silently converted into a copied access token", async () => {
  await assert.rejects(connectPiModel(context({ modelRegistry: { isUsingOAuth: () => true } }), () => assert.fail("uploaded")), /OAuth/);
});
test("extra authentication headers or environment need an explicit adapter", async () => {
  const ctx = context({ modelRegistry: { isUsingOAuth: () => false, getApiKeyAndHeaders: async () => ({ ok: true, apiKey: "key", headers: { "x-extra-secret": "secret" } }) } });
  await assert.rejects(connectPiModel(ctx, () => assert.fail("uploaded")), /No credentials were uploaded/);
});
