import { Input, matchesKey, truncateToWidth } from "@earendil-works/pi-tui";

export const BACK = Symbol("back");

export class DefaultPicker {
  constructor(tui, theme, keys, done, { items, kind, defaultId, title = "Choose default model", state = {} }) {
    this.tui = tui;
    this.theme = theme;
    this.keys = keys;
    this.done = done;
    this.items = items;
    this.kind = kind;
    this.defaultId = defaultId;
    this.title = title;
    this.state = state;
    this.input = new Input();
    this.input.setValue(state.query || "");
    const words = this.input.getValue().toLowerCase().split(/\s+/).filter(Boolean);
    this.filtered = items.filter((item) => words.every((word) => `${item.label} ${item.detail}`.toLowerCase().includes(word)));
    const selected = this.filtered.findIndex((item) => item.id === (state.selectedId ?? defaultId));
    this.index = Math.max(0, Math.min(selected >= 0 ? selected : state.index ?? 0, this.filtered.length - 1));
  }

  get focused() { return this.input.focused; }
  set focused(value) { this.input.focused = value; }
  invalidate() { this.input.invalidate(); }

  finish(value) {
    this.state.query = this.input.getValue();
    this.state.index = this.index;
    this.state.selectedId = this.filtered[this.index]?.id;
    this.done(value);
  }

  handleInput(data) {
    if (matchesKey(data, "left")) return this.finish(BACK);
    if (this.keys.matches(data, "tui.select.cancel")) return this.finish(undefined);
    if (this.keys.matches(data, "tui.select.confirm") || (this.kind === "Model" && matchesKey(data, "ctrl+s"))) {
      if (this.filtered[this.index]) this.finish(this.filtered[this.index]);
      return;
    }
    const count = this.filtered.length;
    if (this.keys.matches(data, "tui.select.up")) this.index = count ? (this.index + count - 1) % count : 0;
    else if (this.keys.matches(data, "tui.select.down")) this.index = count ? (this.index + 1) % count : 0;
    else if (this.keys.matches(data, "tui.select.pageUp")) this.index = Math.max(0, this.index - 10);
    else if (this.keys.matches(data, "tui.select.pageDown")) this.index = Math.max(0, Math.min(count - 1, this.index + 10));
    else {
      const previous = this.input.getValue();
      this.input.handleInput(data);
      if (previous !== this.input.getValue()) {
        const words = this.input.getValue().toLowerCase().split(/\s+/).filter(Boolean);
        this.filtered = this.items.filter((item) => words.every((word) => `${item.label} ${item.detail}`.toLowerCase().includes(word)));
        this.index = 0;
      }
    }
    this.tui.requestRender();
  }

  render(width) {
    const { theme } = this;
    const start = Math.max(0, Math.min(this.index - 4, this.filtered.length - 10));
    const selected = this.filtered[this.index];
    const lines = [theme.fg("border", "─".repeat(width)), theme.fg("accent", this.title)];
    if (this.kind) lines.push(theme.fg("muted", this.kind));
    lines.push("", ...this.input.render(width), "");
    for (const item of this.filtered.slice(start, start + 10)) {
      const active = item === selected;
      lines.push(theme.fg(active ? "accent" : "text", `${active ? "→ " : "  "}${item.label}${item.id === this.defaultId ? " · default" : ""}`));
    }
    if (!selected) lines.push(theme.fg("muted", "  No matching options"));
    lines.push(theme.fg("muted", `  (${selected ? this.index + 1 : 0}/${this.filtered.length})`), "",
      theme.fg("muted", `  ${this.kind === "Model" ? "Model Name" : this.kind || "Selection"}: ${selected?.detail || "—"}`), "");
    if (this.kind) lines.push(theme.fg("success", "  Available in your MagicMtn account."), "");
    lines.push(theme.fg("dim", this.kind === "Model"
      ? "  Enter / Ctrl+S to save · ← back · Escape/Ctrl+C to cancel"
      : "  Enter to select · ← back · Escape/Ctrl+C to cancel"), theme.fg("border", "─".repeat(width)));
    return lines.map((line) => truncateToWidth(line, width));
  }
}

export async function pickDefault(ctx, options) {
  if (!options.items.length) throw new Error(`No available ${(options.kind || options.title).toLowerCase()} options in your MagicMtn account.`);
  return ctx.ui.custom((tui, theme, keys, done) => new DefaultPicker(tui, theme, keys, done, options));
}
