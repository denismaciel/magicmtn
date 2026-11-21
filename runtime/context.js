import { readFileSync } from "node:fs";

// A separate section preserves Pi's prompt, project context, and user addenda.
export default function magicmtnContext(pi) {
  const context = readFileSync(new URL("./context.md", import.meta.url), "utf8");
  pi.on("session_start", (_event, ctx) => {
    if (ctx.hasUI) ctx.ui.setHiddenThinkingLabel("");
  });
  pi.on("before_agent_start", (event) => {
    event.systemPromptOptions.sections.magicmtn = context;
  });
}
