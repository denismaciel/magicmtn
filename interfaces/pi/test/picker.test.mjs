import assert from "node:assert/strict";
import test from "node:test";
import { getKeybindings, visibleWidth } from "@earendil-works/pi-tui";
import { BACK, DefaultPicker } from "../picker.js";

function picker(kind = "Model") {
  let result;
  const items = Array.from({ length: 14 }, (_, i) => ({ id: String(i), label: `GLM model ${i} [${i === 12 ? "baseten" : "fireworks"}]`, detail: `GLM model ${i}` }));
  const component = new DefaultPicker({ requestRender() {} }, { fg(_color, text) { return text; } }, getKeybindings(), (value) => { result = value; }, { items, kind, defaultId: "0" });
  return { component, result: () => result };
}

test("picker filters provider names and shows selection details and count", () => {
  const { component } = picker();
  component.handleInput("baseten");
  const rendered = component.render(100).join("\n");
  assert.match(rendered, /GLM model 12 \[baseten\]/);
  assert.match(rendered, /\(1\/1\)/);
  assert.match(rendered, /Model Name: GLM model 12/);
  assert.doesNotMatch(rendered, /\[fireworks\]/);
});
test("picker navigates beyond the visible page and handles an empty search", () => {
  const { component } = picker();
  component.handleInput("\x1b[6~");
  assert.match(component.render(100).join("\n"), /\(11\/14\)/);
  component.handleInput("unavailable");
  assert.match(component.render(100).join("\n"), /No matching options/);
  assert.match(component.render(100).join("\n"), /\(0\/0\)/);
});
test("model picker saves with Ctrl+S and renders within narrow terminals", () => {
  const { component, result } = picker("Model");
  assert.ok(component.render(30).every((line) => visibleWidth(line) <= 30));
  component.handleInput("\x13");
  assert.equal(result().id, "0");
});
test("Left goes back separately from cancellation and preserves search and selection", () => {
  const { component, result } = picker();
  component.handleInput("baseten");
  component.handleInput("\x1b[D");
  assert.equal(result(), BACK);
  const restored = new DefaultPicker(component.tui, component.theme, component.keys, () => {}, {
    items: component.items, kind: "Model", defaultId: "0", state: component.state,
  });
  assert.equal(restored.input.getValue(), "baseten");
  assert.equal(restored.filtered[restored.index].id, "12");
  assert.match(restored.render(100).join("\n"), /← back/);
});
