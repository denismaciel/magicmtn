// Pi extension: dynamically discovers account MCPs through the trusted broker.
// Only the scoped MagicMtn session credential is available here.
import { readFileSync } from "node:fs";

export default function mcpCatalog(pi) {
  const configPath = process.env.MAGICMTN_INTEGRATIONS_CONFIG;
  if (!configPath) return;
  const toolCount = count => `${count} ${count === 1 ? "tool" : "tools"}`;
  async function request(path, body) {
    const config = JSON.parse(readFileSync(configPath, "utf8"));
    const base = new URL(config.origin);
    if (base.protocol !== "https:" && !(base.protocol === "http:" && ["localhost", "127.0.0.1"].includes(base.hostname))) throw new Error("Invalid MCP broker origin");
    const target = new URL(path, base);
    if (target.origin !== base.origin) throw new Error("Invalid MCP broker path");
    const response = await fetch(target, {
      method: body ? "POST" : "GET",
      headers: {Authorization: `Bearer ${config.token}`, ...(body ? {"Content-Type": "application/json"} : {})},
      body: body ? JSON.stringify(body) : undefined,
      signal: AbortSignal.timeout(30000),
      redirect: "error"
    });
    if (!response.ok) throw new Error(`MCP broker returned ${response.status}`);
    return response.json();
  }
  pi.registerTool({
    name: "mcp_catalog", label: "MCP Catalog",
    description: "List available MCP services and tool counts. Pass service (for example, Linear) or connection_id to list only that service's tool names. Pass tool as well to inspect one tool's input schema before mcp_call. Connections added to the account appear here without restarting the sandbox.",
    parameters: {type: "object", properties: {
      service: {type: "string"}, connection_id: {type: "string"}, tool: {type: "string"}
    }, additionalProperties: false},
    async execute(_id, params = {}) {
      if (params.service && params.connection_id) throw new Error("Provide service or connection_id, not both");
      if (params.tool && !params.service && !params.connection_id) throw new Error("Provide service or connection_id to inspect a tool schema");
      const data = await request("/v1/mcp/runtime");
      if (params.service || params.connection_id) {
        const matches = data.filter(({id, name}) => id === params.connection_id || name.toLowerCase() === params.service?.toLowerCase());
        if (matches.length !== 1) throw new Error(matches.length ? "MCP service name is ambiguous; use connection_id" : "MCP service not found in the current catalog");
        const connection = matches[0];
        if (params.tool) {
          const tool = connection.tools.find(({name}) => name === params.tool);
          if (!tool) throw new Error("MCP tool not found in the current catalog");
          return {content: [{type: "text", text: JSON.stringify({id: connection.id, name: connection.name, tool}, null, 2)}], details: {}};
        }
        const names = connection.tools.map(({name}) => `- ${name}`).join("\n");
        return {content: [{type: "text", text: `${connection.name} (${connection.id}) — ${toolCount(connection.tools.length)}\n${names || "No tools available"}`}], details: {}};
      }
      const summary = data.map(({id, name, tools}) => `- ${name} (${id}): ${toolCount(tools.length)}`).join("\n");
      return {content: [{type: "text", text: summary || "No MCP services available"}], details: {}};
    }
  });
  pi.registerTool({
    name: "mcp_call", label: "MCP Tool",
    description: "Call a tool returned by mcp_catalog. First inspect its input schema with mcp_catalog using its service or connection_id and exact tool name, then pass arguments matching that schema.",
    parameters: {type: "object", properties: {
      connection_id: {type: "string"}, tool: {type: "string"}, arguments: {type: "object"}
    }, required: ["connection_id", "tool", "arguments"], additionalProperties: false},
    async execute(_id, params) {
      const data = await request("/v1/mcp/runtime/call", params);
      if (data.error) throw new Error(`MCP call failed: ${data.error.message || "remote error"}`);
      return {content: [{type: "text", text: JSON.stringify(data.result ?? data)}], details: {}};
    }
  });
}
