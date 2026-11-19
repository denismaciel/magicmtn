// Pi extension: dynamically discovers account MCPs through the trusted broker.
// Only the scoped MagicMtn session credential is available here.
import { brokerTools } from "./broker.js";
import { Container, Text, truncateToWidth } from "@earendil-works/pi-tui";

const empty = () => new Container();
const oneLine = value => String(value).replace(/[\x00-\x1f\x7f-\x9f]/g, " ").replace(/\s+/g, " ").trim();
function statusLine(text, theme, color) {
  return {
    invalidate() {},
    render(width) { return [theme.fg(color, truncateToWidth(oneLine(text), width))]; }
  };
}

function renderers(catalog = false) {
  function describe(args = {}) {
    if (catalog) {
      if (args.tool) return `inspected ${args.tool} schema`;
      return args.service ? `listed ${args.service} tools` : args.connection_id ? "listed MCP service tools" : "listed MCP services";
    }
    const target = args.arguments?.id ?? args.arguments?.query ?? args.arguments?.name;
    return `${args.tool || "MCP tool"}${typeof target === "string" && target ? ` for ${target}` : ""}`;
  }
  return {
    renderCall(args, theme, context) {
      if (!context.isPartial) return empty();
      return statusLine(`${catalog ? "MCP:" : "Calling"} ${describe(args)}…`, theme, "toolTitle");
    },
    renderResult(result, {expanded, isPartial}, theme, context) {
      if (isPartial) return empty();
      const failed = context.isError || result.isError;
      const summary = `${failed ? "Failed:" : catalog ? "MCP:" : "Called"} ${describe(context.args)}`;
      if (!expanded) return statusLine(summary, theme, failed ? "error" : "toolTitle");
      const text = result.content.filter(block => block.type === "text").map(block => block.text).join("\n");
      return new Text(`${summary}\n${text}`, 0, 0);
    }
  };
}

export default function mcpCatalog(pi) {
  const configPath = process.env.MAGICMTN_INTEGRATIONS_CONFIG;
  if (!configPath) return;
  for (const { execute, ...tool } of brokerTools(configPath)) {
    pi.registerTool({
      ...tool,
      ...renderers(tool.name === "mcp_catalog"),
      execute: (_id, params, signal) => execute(params, signal),
    });
  }
}
