import { Model, Plugin, Provider } from "@opencode/plugin";
import { executable, magic } from "./magic.js";

const providerID = Provider.ID.make("magicmtn");
const refreshInterval = 60_000;

export default Plugin.define({
  id: "magicmtn",
  async setup(ctx) {
    let account;
    let models = [];
    let scope;
    const grants = new Map();

    async function refresh() {
      const previous = JSON.stringify([account, scope, models]);
      try {
        account = await magic(["opencode", "account"]);
        const catalog = account.signed_in && account.mode !== "local"
          ? await magic(["opencode", "catalog"]) : undefined;
        scope = catalog?.scope;
        models = catalog?.models || [];
      } catch {
        account = undefined;
        scope = undefined;
        models = [];
      }
      if (previous !== JSON.stringify([account, scope, models])) grants.clear();
    }
    await refresh();

    await ctx.mcp.transform((editor) => {
      if (account?.mode === "local") {
        editor.set("magicmtn", {
          type: "local", command: [executable, "--local", "mcp", "serve"], codemode: false,
        });
      }
    });

    await ctx.provider.transform((editor) => {
      if (!models.length) return;
      editor.add({
        info: {
          ...Provider.Info.empty(providerID),
          name: "MagicMtn",
          activation: "enabled",
          package: "@opencode/ai/providers/openai-compatible",
          settings: { baseURL: "http://127.0.0.1:1", apiKey: "magicmtn-scoped" },
        },
        models: models.map((model) => {
          const definition = Model.Info.default(providerID, Model.ID.make(model.id));
          const context = model.context_window || 32768;
          return {
            ...definition,
            name: model.display_name,
            capabilities: { ...definition.capabilities, toolcall: true },
            limit: { context, output: Math.min(model.max_output_tokens || 4096, context) },
          };
        }),
      });
    });

    await ctx.session.hook("http.request", async (event) => {
      const id = event.model.id;
      if (!models.some((model) => model.id === id)) throw new Error("Select a MagicMtn model from your signed-in account");
      let grant = grants.get(id);
      if (!grant || grant.expiresAt <= Date.now()) {
        grant = {
          expiresAt: Infinity,
          pending: magic(["opencode", "inference", "--model", id]).then((value) => {
            const base = new URL(value.baseUrl);
            const loopback = ["localhost", "127.0.0.1", "[::1]"].includes(base.hostname);
            if (!(base.protocol === "https:" || (loopback && base.protocol === "http:"))
                || base.username || base.password || base.search || base.hash
                || value.api !== "openai-completions" || !value.apiKey
                || !value.models?.some((model) => model.id === id) || !(value.expiresIn > 0)) {
              throw new Error("Invalid MagicMtn inference grant");
            }
            grant.expiresAt = Date.now() + Math.max(0, value.expiresIn - 30) * 1000;
            return value;
          }),
        };
        grants.set(id, grant);
      }
      try {
        const config = await grant.pending;
        if (grants.get(id) !== grant) throw new Error("MagicMtn account changed. Select the model again.");
        const base = new URL(config.baseUrl);
        base.pathname = base.pathname.replace(/\/$/, "") + "/chat/completions";
        const request = new Request(base, event.request);
        request.headers.set("authorization", `Bearer ${config.apiKey}`);
        event.request = new Request(request, { redirect: "error" });
      } catch (error) {
        if (grants.get(id) === grant) grants.delete(id);
        throw error;
      }
    }, { providerID });

    let updating = false;
    let registeredMode = account?.mode;
    let registeredModels = JSON.stringify(models);
    const timer = setInterval(async () => {
      if (updating) return;
      updating = true;
      try {
        await refresh();
        if (registeredMode !== account?.mode) {
          await ctx.mcp.reload();
          registeredMode = account?.mode;
        }
        if (registeredModels !== JSON.stringify(models)) {
          await ctx.provider.reload();
          registeredModels = JSON.stringify(models);
        }
      } catch {
        // The next refresh retries reconciliation.
      } finally {
        updating = false;
      }
    }, refreshInterval);
    timer.unref?.();
    return () => clearInterval(timer);
  },
});
