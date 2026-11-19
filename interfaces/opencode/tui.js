import { Plugin } from "@opencode/plugin/tui";
import { spawn } from "node:child_process";
import { executable, magic, sandboxTool } from "./magic.js";

async function connectTerminal(renderer, args) {
  await renderer.suspend();
  try {
    const child = spawn(executable, args, { stdio: "inherit" });
    await new Promise((resolve, reject) => {
      child.once("error", reject);
      child.once("close", (code) => code === 0 ? resolve() : reject(new Error("Workspace connection ended")));
    });
  } finally {
    await renderer.resume();
  }
}

export default Plugin.define({
  id: "magicmtn",
  setup(context) {
    const { dialog, toast } = context.ui;
    const choose = (title, items, current) => dialog.select({
      title,
      current,
      options: items.map(([value, label]) => ({ value, title: label })),
    });
    const notify = (message) => toast.show({ title: "MagicMtn", message });
    const connect = (args) => connectTerminal(context.renderer, args);
    let busy = false;

    async function connectedModels() {
      const { connections } = await magic(["opencode", "connections"]);
      return connections.filter((item) => !item.revoked).map((item) =>
        [item.id, item.model?.name || item.model?.id || item.id]);
    }

    async function run(input) {
      if (busy) return;
      busy = true;
      try {
        const account = await magic(["opencode", "account"]);
        const local = account.mode === "local";
        if (!local && !account.signed_in) {
          notify("Run magic login to open your MagicMtn workspace.");
          return;
        }

        const menu = [
          ["new", local ? "New sandbox" : "New OpenCode workspace"],
          ["view", "View sandboxes"],
        ];
        if (!local) menu.push(
          ["default", "Choose workspace model"],
          ["inference", "MagicMtn inference with local tools"],
          ["connected", "New workspace with a connected provider"],
        );
        const action = input?.trim() || await choose("MagicMtn", menu);
        if (!action) return;

        if (action === "default") {
          const [{ defaults, models }, connected] = await Promise.all([
            magic(["opencode", "catalog"]),
            connectedModels(),
          ]);
          const items = [
            ...models.map((item) => [item.id, `${item.display_name} · ${item.provider}`]),
            ...connected.map(([id, label]) => [`connection:${id}`, `${label} · Connected provider`]),
          ];
          const defaultModel = defaults.connection_id ? `connection:${defaults.connection_id}` : defaults.model_id;
          const model = await choose("Workspace model", items, defaultModel);
          if (model) {
            const selection = model.startsWith("connection:")
              ? ["--connection", model.slice("connection:".length)]
              : ["--model", model];
            await magic(["opencode", "defaults", ...selection]);
            notify("Workspace model saved.");
          }
        } else if (action === "inference") {
          await context.keymap.dispatch("model.list");
        } else if (action === "connected" && !local) {
          const connections = await connectedModels();
          if (!connections.length) {
            notify("Run magic opencode connect to add a provider.");
            return;
          }
          const id = await choose("Connected provider", connections);
          if (id) {
            notify("Starting workspace…");
            const workspace = await magic(["opencode", "create", "--connection", id]);
            await connect(["opencode", "attach", workspace.id]);
          }
        } else if (action === "new") {
          if (local) {
            const image = await dialog.prompt({ title: "Sandbox image", placeholder: "MagicMtn Canonical" });
            if (image === undefined) return;
            const sandbox = await sandboxTool("sandbox_create", image.trim() ? { image: image.trim() } : {});
            notify(`Created sandbox ${sandbox.id}.`);
          } else {
            notify("Starting workspace…");
            const workspace = await magic(["opencode", "create"]);
            await connect(["opencode", "attach", workspace.id]);
          }
        } else if (["view", "open"].includes(action)) {
          const workspaces = local ? await sandboxTool("sandbox_list") : (await magic(["opencode", "list"])).workspaces;
          const running = workspaces.filter((item) => ["running", "starting", "ready", "pending"].includes(item.status.toLowerCase()));
          if (!running.length) {
            notify("Use /magic new to start a workspace.");
            return;
          }
          const id = await choose("MagicMtn sandboxes", running.map((item) => [item.id, `${item.id} · ${item.status}`]));
          if (!id) return;
          const options = [["attach", local ? "Open shell" : "Enter workspace"], ["terminate", "Terminate sandbox"]];
          if (!local) options.push(["stop", "Stop agent"], ["restart", "Restart agent"]);
          const operation = await choose(`Sandbox ${id.slice(0, 8)}`, options);
          if (!operation) return;
          if (operation === "attach") {
            await connect(local ? ["--local", "sandbox", "ssh", id] : ["opencode", "attach", id]);
          } else if (await dialog.confirm({
            title: options.find(([value]) => value === operation)[1],
            message: operation === "terminate" ? `Delete sandbox ${id} and its workspace files?` : `Apply this action to sandbox ${id}?`,
          })) {
            if (local) await sandboxTool("sandbox_terminate", { id });
            else await magic(["opencode", operation, id]);
            notify("Sandbox updated.");
          }
        } else {
          notify("Use /magic to see workspace commands.");
        }
      } catch (error) {
        toast.show({ title: "MagicMtn", message: error.stderr?.trim() || error.message, variant: "error" });
      } finally {
        busy = false;
      }
    }

    context.ui.slot({
      append: "prompt.footer.status",
      render: () => context.ui.model.current()?.providerID === "magicmtn" ? "MagicMtn inference" : null,
    });

    context.keymap.layer(() => ({
      mode: "global",
      commands: [{
        id: "magicmtn.menu",
        title: "MagicMtn workspaces and inference",
        group: "MagicMtn",
        palette: true,
        slash: { name: "magic", arguments: true },
        run,
      }],
    }));
  },
});
