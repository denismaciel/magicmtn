import { Plugin } from "@opencode/plugin";
import { appendFile, readFile } from "node:fs/promises";
import { brokerTools } from "./broker.js";
import { submit } from "./error-worker.js";

const errorKinds = {
  "session.step.failed": "model",
  "session.tool.failed": "tool",
  "session.compaction.failed": "compaction",
};

export default Plugin.define({
  id: "magicmtn-runtime",
  async setup(ctx) {
    const config = JSON.parse(await readFile(new URL("./config.json", import.meta.url), "utf8"));
    const integrations = process.env.MAGICMTN_INTEGRATIONS_CONFIG;
    if (integrations) {
      await ctx.tool.transform((editor) => {
        for (const tool of brokerTools(integrations)) {
          editor.add({
            name: tool.name,
            description: tool.description,
            input: tool.parameters,
            options: { codemode: false },
            async execute(input, context) {
              const result = await tool.execute(input, context.signal);
              const content = result.content.map((part) => part.text).join("\n");
              if (result.isError) throw new Error(content);
              return { content };
            },
          });
        }
      });
    }

    await ctx.session.hook("context", (event) => {
      event.system.push({ type: "text", text: [
        "You are running OpenCode inside a MagicMtn sandbox. Shell and file tools operate on the sandbox workspace.",
        "MagicMtn synchronizes the managed conversation. Keep work products in workspace files or Git.",
        "The terminal can detach while the workspace continues running.",
        integrations ? "Discover account MCP services with mcp_catalog and call their tools with mcp_call." : "",
        "Use ordinary GitHub repository URLs and gh for GitHub operations.",
      ].filter(Boolean).join("\n") });
    });

    const controller = new AbortController();
    let reportWindow = 0;
    let reportCount = 0;
    let pendingReports = 0;

    void (async () => {
      for await (const event of ctx.event.subscribe({ signal: controller.signal })) {
        if (!event?.durable || event.data?.sessionID !== config.session_id) continue;
        await appendFile(config.journal, JSON.stringify(event) + "\n", { mode: 0o600 });
        const kind = errorKinds[event.type];
        if (!kind || config.error_reporting === false || pendingReports >= 8) continue;
        if (Date.now() - reportWindow >= 60000) {
          reportWindow = Date.now();
          reportCount = 0;
        }
        if (reportCount >= 60) continue;
        reportCount++;
        pendingReports++;
        void submit(config, { kind }).catch(() => {}).finally(() => { pendingReports--; });
      }
    })().catch(() => {});
    return () => controller.abort();
  },
});
