import assert from "node:assert/strict";
import test from "node:test";
import { CustomEditor } from "@earendil-works/pi-coding-agent";
import { Container, CURSOR_MARKER, KeybindingsManager, ScrollView, Spacer, TUI_KEYBINDINGS, TuiMainScreen, VStack, renderFakeCursor, stripTerminalSequences, visibleWidth } from "@earendil-works/pi-tui";
import workspaceStatus from "../workspace.js";

function fixture(t, { mode = "tui", sandbox = true, rows = 40, initialText = "", entries = [], headerRows = 8, nested = false, previousEditor } = {}) {
  const previous = process.env.MAGICMTN_WORKSPACE_INFO;
  t.after(() => {
    if (previous === undefined) delete process.env.MAGICMTN_WORKSPACE_INFO;
    else process.env.MAGICMTN_WORKSPACE_INFO = previous;
  });
  if (sandbox) process.env.MAGICMTN_WORKSPACE_INFO = JSON.stringify({ id: "d215125b-4896-4ca4-a36f-aefdeea095fb" });
  else delete process.env.MAGICMTN_WORKSPACE_INFO;

  const handlers = new Map();
  const widgets = new Map();
  const statuses = new Map();
  const input = [];
  const tui = new TuiMainScreen({ columns: 80, rows });
  let renders = 0;
  tui.requestRender = () => { renders++; };
  const widgetContainer = new Container();
  tui.requestImmediateRender = () => {};
  const theme = { fg(color, text) {
    assert.ok(["accent", "dim"].includes(color));
    return `\x1b[${color === "accent" ? 36 : 90}m${text}\x1b[39m`;
  } };
  const editorTheme = { borderColor(text) { return text; }, selectList: {} };
  const keys = new KeybindingsManager({ ...TUI_KEYBINDINGS,
    "app.clear": { defaultKeys: "ctrl+c" },
    "app.interrupt": { defaultKeys: "escape" },
    "app.exit": { defaultKeys: "ctrl+d" },
  });
  let editorFactory = previousEditor;
  let editor = new CustomEditor(tui, editorTheme, keys);
  editor.setText(initialText);
  tui.setFocus({ handleInput(data) { input.push(data); editor.handleInput(data); } });
  let subscriptions = 0;
  const ctx = { mode, sessionManager: { getBranch: () => entries }, ui: {
    theme,
    getEditorComponent() { return editorFactory; },
    setEditorComponent(factory) {
      const text = editor.getText();
      const onSubmit = editor.onSubmit;
      editorFactory = factory;
      editor = factory?.(tui, editorTheme, keys) ?? new CustomEditor(tui, editorTheme, keys);
      editor.setText(text);
      editor.onSubmit = onSubmit;
      editor.focused = true;
    },
    setStatus(key, value) { statuses.set(key, value); },
    setWidget(key, factory) {
      if (factory) widgets.set(key, factory(tui, theme));
      else widgets.delete(key);
      widgetContainer.clear();
      widgetContainer.addChild(new Spacer(1));
      for (const widget of widgets.values()) widgetContainer.addChild(widget);
    },
    onTerminalInput(handler) {
      subscriptions++;
      const unsubscribe = tui.addInputListener(handler);
      return () => { subscriptions--; unsubscribe(); };
    },
  } };
  const header = { render: () => [...Array(headerRows - 2).fill("Pi startup"), "[Extensions]", "  magicmtn-mcp, magicmtn-workspace"], invalidate() {} };
  const inputEditor = { render: (width) => editor.render(width), invalidate() {} };
  const footer = { render: () => ["/workspace", "0.0%/33k", "MagicMtn · sandbox d215125b"], invalidate() {} };
  if (nested) {
    tui.addChild(new VStack([new ScrollView(header), new VStack([widgetContainer, inputEditor, footer])]));
  } else {
    for (const component of [header, widgetContainer, inputEditor, footer]) tui.addChild(component);
  }
  workspaceStatus({ on(event, handler) { handlers.set(event, handler); } });
  const emit = (event) => handlers.get(event)?.({}, ctx);
  emit("session_start");
  t.after(() => emit("session_shutdown"));
  return { tui, renders: () => renders, widgets, statuses, input, emit, editor: () => editor, editorFactory: () => editorFactory, subscriptions: () => subscriptions,
    send: (data) => tui.handleTerminalInput(data) };
}

test("sandbox welcome renders the faint dotted logo and copy above the editor", (t) => {
  const f = fixture(t);
  const widget = f.widgets.get("magicmtn-welcome");
  const renderedLines = widget.render(80);
  for (const line of renderedLines.filter((line) => line.includes("⣿"))) {
    assert.match(line, /\x1b\[2m\x1b\[90m.*⣿.*\x1b\[39m\x1b\[22m$/);
  }
  const lines = renderedLines.map(stripTerminalSequences).map((line) => line.trimEnd());
  assert.deepEqual(lines.filter((line) => line.includes("⣿")), [
    ...Array(3).fill(" ".repeat(49) + "⣿⣿⣿⣿⣿⣿⣿⣿"),
    ...Array(3).fill(" ".repeat(40) + "⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿"),
    ...Array(3).fill(" ".repeat(22) + "⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿"),
  ]);
  for (const width of [40, 80, 120]) {
    const rendered = widget.render(width).map(stripTerminalSequences);
    const base = rendered.find((line) => line.trim() === "⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿ ⣿⣿⣿⣿⣿⣿⣿⣿");
    const copy = rendered.slice(rendered.findLastIndex((line) => line.includes("⣿")) + 1)
      .filter((line) => line.trim());
    assert.equal(copy.map((line) => line.trim()).join(" "), "MagicMtn");
    assert.doesNotMatch(rendered.join("\n"), /Atop the MagicMtn sandbox/);
    for (const entry of [base, ...copy]) {
      const line = entry.trimEnd();
      const left = line.length - line.trimStart().length;
      const right = width - visibleWidth(line);
      assert.ok(Math.abs(left - right) <= 1, `center ${line.trim()} at width ${width}`);
    }
  }
  assert.ok(renderedLines[0].includes("\x1b[36mTip:\x1b[39m"));
  assert.match(stripTerminalSequences(lines.join("\n")), /Tip: Integrate with your GitHub repos and add MCPs to power your agents at\s+https:\/\/control\.magicmtn\.ai\//);
  assert.doesNotMatch(lines.join("\n"), /REMOTE SANDBOX|widget truncated/);
  for (const width of [20, 40, 80]) {
    assert.ok(widget.render(width).every((line) => visibleWidth(line) <= width));
  }
});

test("short terminals use a compact mountain and retain the editor", (t) => {
  const f = fixture(t, { rows: 24 });
  const lines = f.widgets.get("magicmtn-welcome").render(80);
  assert.match(stripTerminalSequences(lines[0]), /^   Tip:/);
  assert.ok(lines.length <= 16);
  assert.equal(lines.join("\n").match(/⣿/g).length, 7);
  assert.equal(f.tui.render(80).length, 24);
  assert.match(lines.join("\n"), /https:\/\/control\.magicmtn\.ai\//);
});

for (const nested of [false, true]) {
  test(`welcome fills the available ${nested ? "fullscreen" : "regular"} layout without a gap above the tip`, (t) => {
    const f = fixture(t, { rows: 48, headerRows: 10, nested });
    for (const width of [80, 120, 200]) {
      const lines = f.tui.render(width).map(stripTerminalSequences);
      assert.equal(lines.length, 48);
      assert.equal(lines[8], "[Extensions]");
      assert.match(lines[11], /^   Tip:/);
      const bodyStart = lines.findIndex((line, index) => index > 11 && !line.trim()) + 1;
      const first = lines.findIndex((line) => line.includes("⣿"));
      const last = lines.findLastIndex((line) => line.includes("⣿"));
      const bodyEnd = lines.length - f.editor().render(width).length - 3;
      assert.ok(Math.abs((first - bodyStart) - (bodyEnd - last - 1)) <= 1);
      const base = lines[last].trimEnd();
      assert.ok(Math.abs((base.length - base.trimStart().length) - (width - visibleWidth(base))) <= 1);
    }
  });
}

test("welcome re-centers when the terminal is resized", (t) => {
  const f = fixture(t);
  for (const rows of [60, 24, 40]) {
    f.tui.terminal.rows = rows;
    assert.equal(f.tui.render(80).length, rows);
    assert.match(f.widgets.get("magicmtn-welcome").render(80).join("\n"), /Tip:/);
  }
});

test("sparkles pulse one at a time at different positions and loop", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const f = fixture(t);
  const frames = [];
  for (let frame = 0; frame <= 61; frame++) {
    const logo = f.widgets.get("magicmtn-welcome").render(80).filter((line) => /⣿/.test(line)).map(stripTerminalSequences);
    assert.ok((logo.join("\n").match(/[.*+]/g) || []).length <= 1);
    frames.push(logo.join("\n"));
    t.mock.timers.tick(180);
  }
  assert.notEqual(frames[1], frames[13]);
  assert.equal(frames[0], frames[60]);
  assert.equal(frames[1], frames[61]);
  assert.match(frames[3], /\*/);
  assert.doesNotMatch(frames[8], /[.*+]/);
});

test("animation stops on interaction, agent start, restart and shutdown", (t) => {
  t.mock.timers.enable({ apis: ["setInterval"] });
  const f = fixture(t);
  t.mock.timers.tick(180);
  assert.equal(f.renders(), 1);
  f.emit("session_start");
  t.mock.timers.tick(180);
  assert.equal(f.renders(), 2);
  f.send("x");
  const stopped = f.renders();
  t.mock.timers.tick(3600);
  assert.equal(f.renders(), stopped);
  f.emit("session_start");
  f.emit("agent_start");
  const agentStarted = f.renders();
  t.mock.timers.tick(3600);
  assert.equal(f.renders(), agentStarted);
  f.emit("session_start");
  f.emit("session_shutdown");
  t.mock.timers.tick(3600);
  assert.equal(f.renders(), agentStarted);
});

for (const [name, data] of [
  ["typing", "a"], ["arrow key", "\x1b[A"], ["shortcut", "\x03"],
  ["paste", "\x1b[200~first prompt\x1b[201~"],
]) {
  test(`first ${name} dismisses the mountain and tip and reaches Pi unchanged`, (t) => {
    const f = fixture(t);
    const status = f.statuses.get("magicmtn");
    const before = f.widgets.get("magicmtn-welcome").render(80);
    f.send(data);
    const after = f.widgets.get("magicmtn-welcome").render(80);
    assert.ok(before.length > after.length);
    assert.equal(after.length, 1);
    const remaining = after.join("\n");
    assert.doesNotMatch(remaining, /⣿|Tip:|GitHub|MCPs|control\.magicmtn\.ai/);
    assert.ok(after.some((line) => line.trim() === "MagicMtn"));
    assert.doesNotMatch(f.editor().render(80).join("\n"), /Atop the MagicMtn sandbox/);
    assert.equal(f.subscriptions(), 0);
    assert.deepEqual(f.input, [data]);
    assert.equal(f.statuses.get("magicmtn"), status);
    f.send("next");
    assert.deepEqual(f.input, [data, "next"]);
    f.emit("model_select");
    assert.doesNotMatch(f.widgets.get("magicmtn-welcome").render(80).join("\n"), /⣿|Tip:|control\.magicmtn\.ai/);
    f.emit("session_shutdown");
    assert.equal(f.widgets.size, 0);
    assert.equal(f.subscriptions(), 0);
  });
}

test("starting the agent dismisses the tip without requiring terminal input", (t) => {
  const f = fixture(t);
  f.emit("agent_start");
  const rendered = f.widgets.get("magicmtn-welcome").render(80).join("\n");
  assert.doesNotMatch(rendered, /⣿|Tip:|GitHub|MCPs|control\.magicmtn\.ai/);
  assert.match(rendered, /MagicMtn/);
  assert.equal(f.subscriptions(), 0);
  assert.deepEqual(f.input, []);
});

for (const role of ["user", "assistant"]) {
  test(`resuming a session with a ${role} message skips the welcome`, (t) => {
    const f = fixture(t, { entries: [{ type: "message", message: { role, content: "Existing conversation" } }] });
    assert.equal(f.widgets.size, 0);
    assert.equal(f.subscriptions(), 0);
    assert.match(f.statuses.get("magicmtn"), /sandbox d215125b/);
  });
}

test("terminal reports and focus changes leave the welcome visible", (t) => {
  const f = fixture(t);
  for (const data of ["", "\x1b[I", "\x1b[O", "\x1b[6;20;10t", "\x1b[8;40;80t", "\x1b[1;1R"]) {
    f.send(data);
    assert.ok(f.widgets.has("magicmtn-welcome"));
    assert.match(f.widgets.get("magicmtn-welcome").render(80).join("\n"), /Tip:/);
  }
  f.send("x");
  assert.doesNotMatch(f.widgets.get("magicmtn-welcome").render(80).join("\n"), /⣿/);
});

test("dim input placeholder is visual only, retains the cursor, and fits narrow terminals", (t) => {
  const f = fixture(t);
  const editor = f.editor();
  const rendered = editor.render(80);
  assert.match(rendered[1], /\x1b\[90mAtop the MagicMtn sandbox, do your most ambitious work\x1b\[39m/);
  assert.ok(rendered[1].includes(CURSOR_MARKER));
  assert.ok(rendered[1].includes(`${renderFakeCursor(" ")}\x1b[90mAtop`));
  assert.equal(editor.getText(), "");
  for (const width of [1, 10, 40, 80, 200]) {
    assert.ok(editor.render(width).every((line) => visibleWidth(line) <= width));
  }
  f.send("h");
  f.send("i");
  let submitted;
  editor.onSubmit = (text) => { submitted = text; };
  f.send("\r");
  assert.equal(submitted, "hi");
  assert.doesNotMatch(editor.render(80).join("\n"), /Atop the MagicMtn sandbox/);
});

test("prefilled input is preserved without a placeholder overlay", (t) => {
  const f = fixture(t, { initialText: "Existing draft" });
  assert.equal(f.editor().getText(), "Existing draft");
  assert.doesNotMatch(f.editor().render(80).join("\n"), /Atop the MagicMtn sandbox/);
});

test("placeholder composes with a custom editor and restores its factory on shutdown", (t) => {
  let actions = 0;
  const previousEditor = (tui, theme, keys) => {
    const editor = new CustomEditor(tui, theme, keys);
    editor.onAction("app.clear", () => { actions++; });
    return editor;
  };
  const f = fixture(t, { previousEditor });
  assert.match(f.editor().render(80).join("\n"), /Atop the MagicMtn sandbox/);
  f.send("\x03");
  assert.equal(actions, 1);
  f.emit("session_shutdown");
  assert.equal(f.editorFactory(), previousEditor);
});

test("session restart replaces the listener and shutdown removes it", (t) => {
  const f = fixture(t);
  f.emit("session_start");
  assert.equal(f.subscriptions(), 1);
  f.emit("session_shutdown");
  f.emit("session_shutdown");
  assert.equal(f.subscriptions(), 0);
  assert.equal(f.widgets.size, 0);
});

for (const mode of ["rpc", "json", "print"]) {
  test(`${mode} mode keeps the footer without installing a welcome or input hook`, (t) => {
    const f = fixture(t, { mode });
    assert.match(f.statuses.get("magicmtn"), /sandbox d215125b/);
    assert.equal(f.widgets.size, 0);
    assert.equal(f.subscriptions(), 0);
  });
}

test("local Pi does not show a sandbox welcome", (t) => {
  const f = fixture(t, { sandbox: false });
  assert.equal(f.widgets.size, 0);
  assert.equal(f.subscriptions(), 0);
  assert.equal(f.statuses.size, 0);
});
