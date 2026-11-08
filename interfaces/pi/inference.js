import { lazyStream, openAICompletionsApi } from "@earendil-works/pi-ai/compat";

// Pi owns model selection, rendering, requests, and the agent/tool loop. This
// provider only supplies account models and a short-lived grant for each request.
export function createInferenceProvider(request, { now = Date.now, api = openAICompletionsApi() } = {}) {
  let active = false;
  let revision = 0;
  let models = new Map();
  let catalog = new Map();
  let aliases = new Map();
  const grants = new Map();

  async function authorize(id, signal) {
    id = aliases.get(id) || id;
    if (!active || !catalog.has(id)) throw new Error("Sign in to MagicMtn and select an available model with /magic inference.");
    signal?.throwIfAborted();
    const cached = grants.get(id);
    if (cached && cached.expiresAt > now()) return cached.config;
    const current = revision;
    const config = await request(["pi", "inference", "--model", id], signal);
    signal?.throwIfAborted();
    if (!active || current !== revision) throw new Error("MagicMtn account models changed; select the model again.");
    const definition = config.models?.find((model) => model.id === id);
    if (!definition || config.api !== "openai-completions" || !config.apiKey || !config.baseUrl) {
      throw new Error("Invalid MagicMtn inference configuration");
    }
    const lifetime = Number(config.expiresIn);
    if (!Number.isFinite(lifetime) || lifetime <= 0) throw new Error("Invalid MagicMtn grant lifetime");
    models.set(id, { ...definition, id: models.get(id).id, provider: "magicmtn", api: config.api, baseUrl: config.baseUrl });
    grants.set(id, { config, expiresAt: now() + Math.max(0, lifetime - 60) * 1000 });
    return config;
  }

  const forward = (method) => (model, context, options) => lazyStream(model, async () => {
    const id = aliases.get(model.id) || model.id;
    const config = await authorize(id, options?.signal);
    const resolved = { ...model, ...config.models.find((entry) => entry.id === id), id: model.id, provider: "magicmtn", api: config.api, baseUrl: config.baseUrl };
    // Never reuse another provider's headers or credentials when switching models.
    // Pi's other request options (abort, tools, transport and event hooks) pass through.
    const headers = Object.fromEntries(Object.entries(options?.headers || {}).filter(([key]) => !["authorization", "x-api-key", "api-key"].includes(key.toLowerCase())));
    return api[method](resolved, context, {
      ...options, headers, apiKey: config.apiKey,
      // Pi's footer displays model.id. Keep a readable, stable Pi ID while
      // sending the account's registration UUID required by the scoped gateway.
      async onPayload(payload) {
        const wirePayload = { ...payload, model: id };
        return (await options?.onPayload?.(wirePayload, resolved)) ?? wirePayload;
      },
    });
  });

  return {
    provider: {
      id: "magicmtn",
      name: "MagicMtn",
      auth: { apiKey: {
        name: "MagicMtn account",
        async check() { return active ? { type: "api_key", source: "magic login" } : undefined; },
        async resolve() { return active ? { auth: {}, source: "magic login" } : undefined; },
      } },
      getModels() { return [...models.values()]; },
      stream: forward("stream"),
      streamSimple: forward("streamSimple"),
    },
    load(entries) {
      revision += 1;
      grants.clear();
      catalog = new Map(entries.map((entry) => [entry.id, entry]));
      aliases = new Map();
      models = new Map(entries.map((entry) => {
        const stem = (entry.source_model_id?.split("/").at(-1) || entry.display_name)
          .toLowerCase().replace(/[^a-z0-9._-]+/g, "-").replace(/^-|-$/g, "");
        const id = `${entry.provider}/${stem || "model"}:${entry.id.slice(0, 8)}`;
        aliases.set(id, entry.id);
        return [entry.id, {
          id, name: entry.display_name, provider: "magicmtn", api: "openai-completions",
          // Requests always resolve the real endpoint before reaching Pi's adapter.
          baseUrl: "", reasoning: false, input: ["text"],
          contextWindow: 32768, maxTokens: 4096,
          cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
        }];
      }));
      active = entries.length > 0;
    },
    clear() { active = false; revision += 1; models.clear(); catalog.clear(); aliases.clear(); grants.clear(); },
    model(id) { return catalog.get(aliases.get(id) || id); },
    idFor(id) { return models.get(id)?.id; },
    authorize,
  };
}
