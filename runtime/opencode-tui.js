import { Plugin } from "@opencode/plugin/tui";

export default Plugin.define({
  id: "magicmtn-runtime",
  setup(context) {
    const label = "MagicMtn sandbox";
    context.ui.slot({ append: "home.footer.status", render: () => label });
    context.ui.slot({ append: "prompt.footer.status", render: () => label });
  },
});
