// Loaded inside the sandbox. Only public workspace metadata is passed here.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { CustomEditor } from "@earendil-works/pi-coding-agent";
import { CURSOR_MARKER, Text, renderFakeCursor, stripTerminalSequences, truncateToWidth, visibleWidth } from "@earendil-works/pi-tui";

const LOGO = `                           ⣿⣿⣿⣿⣿⣿⣿⣿
                           ⣿⣿⣿⣿⣿⣿⣿⣿
                           ⣿⣿⣿⣿⣿⣿⣿⣿

                  ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿
                  ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿
                  ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿

⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿
⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿
⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿`;

const WELCOME = "Atop the MagicMtn sandbox, do your most ambitious work";
const COMPACT_LOGO = "      ⣿\n    ⣿ ⣿\n⣿ ⣿ ⣿ ⣿";
const SPARKLE_FRAMES = ["", ".", "+", "*", "+", ".", "", "", "", "", "", ""];
const SPARKLE_POSITIONS = [[1, 30], [5, 21], [9, 4], [9, 31], [5, 31]];
const COMPACT_SPARKLE_POSITIONS = [[0, 6], [1, 4], [2, 0], [2, 6], [1, 6]];

export function providerLabel(provider) {
  return ({ "apple-container": "Apple Container", exe: "exe.dev", aws: "AWS Bedrock", fireworks: "Fireworks", baseten: "Baseten" })[provider] || provider;
}

export default function workspaceStatus(pi) {
  const raw = process.env.MAGICMTN_WORKSPACE_INFO;
  if (!raw) return;
  const workspace = JSON.parse(raw);
  let cleanupWelcome;
  let relayTimer;
  let cleanupRelay;
  const watchRelay = (ctx) => {
    cleanupRelay?.();
    const statusPath = join("/tmp", `magicmtn-relay-${workspace.id}.json`);
    let previous;
    const refresh = () => {
      let state;
      try { state = JSON.parse(readFileSync(statusPath, "utf8")).state; } catch { /* No active relay. */ }
      if (state === previous) return;
      previous = state;
      const message = state === "copying" ? "Copying your local files in the background…"
        : state === "failed" ? "Local file copy failed. Check the transfer log on your local machine." : undefined;
      ctx.ui.setWidget("magicmtn-relay", message
        ? (_tui, theme) => new Text(theme.fg("dim", message), 0, 0) : undefined);
    };
    refresh();
    relayTimer = setInterval(refresh, 1000);
    relayTimer.unref?.();
    cleanupRelay = () => {
      clearInterval(relayTimer);
      ctx.ui.setWidget("magicmtn-relay", undefined);
    };
  };
  let dismissWelcome;
  const update = (_event, ctx) => {
    const model = ctx.model;
    const inference = model
      ? `${model.name || model.id} (${providerLabel(model.provider === "magicmtn" ? "MagicMtn" : model.provider)})`
      : "inference not selected";
    ctx.ui.setStatus("magicmtn", `MagicMtn · sandbox ${workspace.id.slice(0, 8)} · ${inference}`);
  };
  pi.on("session_start", (event, ctx) => {
    update(event, ctx);
    cleanupWelcome?.();
    if (ctx.mode !== "tui") return;
    watchRelay(ctx);
    if (ctx.sessionManager.getBranch().some((entry) => entry.type === "message"
      && ["user", "assistant"].includes(entry.message.role))) return;

    let showWelcome = true;
    let animation;
    let frame = 0;
    let requestRender = () => {};
    const previousEditor = ctx.ui.getEditorComponent();
    const placeholderEditor = (tui, theme, keybindings) => {
      const editor = previousEditor?.(tui, theme, keybindings)
        ?? new CustomEditor(tui, theme, keybindings, { embedWorkingStatus: true });
      const render = editor.render.bind(editor);
      editor.render = (width) => {
        const lines = render(width);
        if (!showWelcome || editor.getText() || !lines[1] || stripTerminalSequences(lines[1]).trim()) return lines;
        const padding = Math.min(editor.getPaddingX?.() ?? 0, Math.max(0, Math.floor((width - 1) / 2)));
        const available = Math.max(0, width - padding * 2 - 1);
        if (!available) return lines;
        // Retain Pi's cursor cell and render the hint after it, never in input state.
        const marker = lines[1].includes(CURSOR_MARKER) ? CURSOR_MARKER : "";
        const prefix = " ".repeat(padding) + marker + renderFakeCursor(" ");
        const hint = ctx.ui.theme.fg("dim", truncateToWidth(WELCOME, available, ""));
        const content = prefix + hint;
        lines[1] = content + " ".repeat(Math.max(0, width - visibleWidth(content)));
        return lines;
      };
      return editor;
    };
    ctx.ui.setEditorComponent(placeholderEditor);

    // A component avoids Pi's ten-line limit for string-array widgets.
    ctx.ui.setWidget("magicmtn-welcome", (tui, theme) => {
      requestRender = () => tui.requestRender();
      const tip = `   ${theme.fg("accent", "Tip:")} Integrate with your GitHub repos and add MCPs to power your agents at https://control.magicmtn.ai/`;
      const tipText = new Text(tip, 0, 0);
      const copy = new Text("MagicMtn", 0, 0);
      let measuring = false;
      const widget = {
        render(width) {
          if (measuring) return [];
          const center = (line) => {
            const text = line.trim();
            return " ".repeat(Math.max(0, Math.floor((width - visibleWidth(text)) / 2))) + text;
          };
          const welcome = copy.render(width).map(center);
          if (!showWelcome) return welcome;

          // Measure Pi's native content with this widget omitted to avoid recursion.
          let occupied;
          measuring = true;
          try { occupied = tui.children.reduce((height, child) => height + child.render(width).length, 0); }
          finally { measuring = false; }
          const height = Math.max(0, tui.terminal.rows - occupied);
          if (height < 4 + welcome.length) return welcome.slice(0, height);

          let tips = tipText.render(width);
          const full = width >= 35 && height >= tips.length + 13 + welcome.length;
          const logo = (full ? LOGO : COMPACT_LOGO).split("\n");
          const positions = full ? SPARKLE_POSITIONS : COMPACT_SPARKLE_POSITIONS;
          const sparkle = SPARKLE_FRAMES[frame % SPARKLE_FRAMES.length];
          if (sparkle) {
            const [row, column] = positions[Math.floor(frame / SPARKLE_FRAMES.length) % positions.length];
            logo[row] = logo[row].slice(0, column) + theme.fg("accent", sparkle) + logo[row].slice(column + 1);
          }
          const logoPadding = " ".repeat(Math.max(0, Math.floor((width - (full ? 35 : 7)) / 2)));
          // Braille cells provide fine dots; faint theme color softens the mark on any background.
          const artwork = logo.map((line) => logoPadding
            + `\x1b[2m${theme.fg("dim", truncateToWidth(line, width - logoPadding, ""))}\x1b[22m`);
          const content = [...artwork, "", ...welcome];
          if (height < tips.length + 1 + content.length) tips = [];
          const heading = tips.length ? [...tips, ""] : [];
          const space = Math.max(0, height - heading.length - content.length);
          const above = Math.min(space, Math.floor((height - heading.length - logo.length) / 2));
          return [...heading, ...Array(above).fill(""), ...content, ...Array(space - above).fill("")];
        },
        invalidate() { tipText.invalidate(); copy.invalidate(); },
        dispose() { clearInterval(animation); },
      };
      return widget;
    });
    let unsubscribe;
    dismissWelcome = () => {
      if (!showWelcome) return;
      showWelcome = false;
      clearInterval(animation);
      unsubscribe?.();
      unsubscribe = undefined;
      requestRender();
    };
    unsubscribe = ctx.ui.onTerminalInput((data) => {
      // Focus and terminal size/cursor reports are not keystrokes.
      if (!data || /^(?:\x1b\[(?:[IO]|\d+;\d+R|(?:4|6|8);\d+;\d+t))+$/.test(data)) return;
      dismissWelcome?.();
      // Leave input untouched so the first key or paste still reaches Pi.
    });
    cleanupWelcome = () => {
      clearInterval(animation);
      unsubscribe?.();
      ctx.ui.setWidget("magicmtn-welcome", undefined);
      if (ctx.ui.getEditorComponent() === placeholderEditor) ctx.ui.setEditorComponent(previousEditor);
      dismissWelcome = undefined;
      cleanupWelcome = undefined;
    };
    animation = setInterval(() => {
      const previous = SPARKLE_FRAMES[frame % SPARKLE_FRAMES.length];
      frame++;
      if (previous || SPARKLE_FRAMES[frame % SPARKLE_FRAMES.length]) requestRender();
    }, 180);
    animation.unref?.();
  });
  pi.on("agent_start", () => dismissWelcome?.());
  pi.on("session_shutdown", () => { cleanupWelcome?.(); cleanupRelay?.(); });
  pi.on("model_select", update);
}
