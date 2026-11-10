import test from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { visibleWidth } from "@earendil-works/pi-tui";
import mcpCatalog from "../mcp.js";

test("Pi MCP tools use the scoped broker token and fetch the current catalog", async () => {
  const requests = [];
  const inputSchema = {type: "object", properties: {query: {type: "string", description: "A long tool argument description"}}};
  let catalog = [{id: "connection-one", name: "Sentry", tools: [{name: "find_issues", description: "A long tool description", inputSchema}]}];
  const server = createServer(async (request, response) => {
    let body = "";
    for await (const chunk of request) body += chunk;
    requests.push({url: request.url, authorization: request.headers.authorization, body});
    response.setHeader("Content-Type", "application/json");
    response.end(JSON.stringify(request.url === "/v1/mcp/runtime" ? catalog : {result: {content: [{type: "text", text: "issue found"}]}}));
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const directory = mkdtempSync(join(tmpdir(), "magicmtn-mcp-"));
  const previous = process.env.MAGICMTN_INTEGRATIONS_CONFIG;
  try {
    const path = join(directory, "integrations.json");
    writeFileSync(path, JSON.stringify({origin: `http://127.0.0.1:${server.address().port}`, token: "scoped-session-token"}));
    process.env.MAGICMTN_INTEGRATIONS_CONFIG = path;
    const tools = new Map();
    mcpCatalog({registerTool(definition) { tools.set(definition.name, definition); }});
    assert.deepEqual([...tools.keys()], ["mcp_catalog", "mcp_call"]);
    const first = await tools.get("mcp_catalog").execute();
    assert.equal(first.content[0].text, "- Sentry (connection-one): 1 tool");
    assert.doesNotMatch(first.content[0].text, /inputSchema|description/);
    const service = await tools.get("mcp_catalog").execute("service-id", {service: "sentry"});
    assert.equal(service.content[0].text, "Sentry (connection-one) — 1 tool\n- find_issues");
    const detail = await tools.get("mcp_catalog").execute("detail-id", {service: "Sentry", tool: "find_issues"});
    assert.deepEqual(JSON.parse(detail.content[0].text), {
      id: "connection-one", name: "Sentry", tool: catalog[0].tools[0]
    });
    await assert.rejects(tools.get("mcp_catalog").execute("detail-id", {tool: "find_issues"}), /service or connection_id/);
    await assert.rejects(tools.get("mcp_catalog").execute("detail-id", {service: "Sentry", connection_id: "connection-one"}), /not both/);
    await assert.rejects(tools.get("mcp_catalog").execute("detail-id", {connection_id: "connection-one", tool: "missing"}), /not found/);
    catalog = [
      {id: "connection-one", name: "Sentry", tools: [{name: "find_issues"}]},
      {id: "connection-two", name: "Linear", tools: [{name: "list_issues"}]},
      {id: "connection-three", name: "Notion", tools: [{name: "notion-search"}]},
      {id: "connection-four", name: "Grafana Cloud", tools: [{name: "query_prometheus"}]},
      {id: "connection-five", name: "PostHog", tools: [{name: "exec"}]}
    ];
    const second = await tools.get("mcp_catalog").execute();
    assert.match(second.content[0].text, /Linear/);
    assert.doesNotMatch(second.content[0].text, /list_issues|notion-search|query_prometheus/);
    assert.match(second.content[0].text, /PostHog/);
    const linear = await tools.get("mcp_catalog").execute("service-id", {service: "Linear"});
    assert.equal(linear.content[0].text, "Linear (connection-two) — 1 tool\n- list_issues");
    const called = await tools.get("mcp_call").execute("call-id", {connection_id: "connection-two", tool: "list_issues", arguments: {query: "api"}});
    assert.match(called.content[0].text, /issue found/);
    const theme = {fg(_color, text) { return text; }};
    const callTool = tools.get("mcp_call");
    const args = {tool: "get_issue", arguments: {id: "ROD-60", fields: ["description"]}};
    const context = {args, isPartial: false, isError: false};
    assert.deepEqual(callTool.renderCall(args, theme, context).render(80), []);
    const compact = callTool.renderResult(called, {expanded: false}, theme, context);
    assert.deepEqual(compact.render(80), ["Called get_issue for ROD-60"]);
    assert.equal(compact.render(12).length, 1);
    assert.ok(visibleWidth(compact.render(12)[0]) <= 12);
    assert.match(callTool.renderResult(called, {expanded: true}, theme, context).render(80).join("\n"), /issue found/);
    assert.deepEqual(callTool.renderCall(args, theme, {...context, isPartial: true}).render(80), ["Calling get_issue for ROD-60…"]);
    assert.deepEqual(callTool.renderResult(called, {expanded: false}, theme, {...context, isError: true}).render(80), ["Failed: get_issue for ROD-60"]);
    assert.deepEqual(callTool.renderResult(called, {expanded: false}, theme, {args: {}}).render(80), ["Called MCP tool"]);
    const catalogTool = tools.get("mcp_catalog");
    assert.deepEqual(catalogTool.renderResult(detail, {expanded: false}, theme, {args: {tool: "find_issues"}}).render(80), ["MCP: inspected find_issues schema"]);
    assert.equal(requests.length, 7);
    assert.ok(requests.every(request => request.authorization === "Bearer scoped-session-token"));
    assert.deepEqual(JSON.parse(requests[6].body), {connection_id: "connection-two", tool: "list_issues", arguments: {query: "api"}});
  } finally {
    if (previous === undefined) delete process.env.MAGICMTN_INTEGRATIONS_CONFIG;
    else process.env.MAGICMTN_INTEGRATIONS_CONFIG = previous;
    rmSync(directory, {recursive: true, force: true});
    await new Promise(resolve => server.close(resolve));
  }
});
