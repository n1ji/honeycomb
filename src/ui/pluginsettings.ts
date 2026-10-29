import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { enabledIds, ListedPlugin, pluginApi, pluginState, setEnabled, settingsSections } from "../plugins";
import { h } from "../util";
import { run } from "./actions";
import { button } from "./dialogs";

/** Settings → Plugins: turn plugins on and off, then each active plugin's own settings. */
export function pluginsSection(): HTMLElement[] {
  const list = h("div", { class: "plugin-list" });
  const own = h("div", {});

  const renderOwn = () =>
    own.replaceChildren(
      ...settingsSections().map(({ manifest, render }) => {
        const el = h("div", { class: "plugin-settings" });
        try {
          render(el);
        } catch (e) {
          el.replaceChildren(h("p", { class: "error" }, String(e)));
        }
        return h("div", {}, h("h3", {}, manifest.name), el);
      }),
    );

  const row = (p: ListedPlugin) => {
    const m = p.manifest;
    if (!m) return h("div", { class: "plugin-row" }, h("strong", {}, p.folder), h("p", { class: "error" }, p.error ?? "Not a plugin"));
    const state = pluginState(m.id);
    const box = h("input", { type: "checkbox", checked: enabledIds().includes(m.id) });
    box.addEventListener("change", () =>
      run(async () => {
        try {
          await setEnabled(m, box.checked);
        } finally {
          await render();
        }
      }),
    );
    const access = [
      m.hosts.length ? `connects to ${m.hosts.join(", ")}` : "no network access",
      m.loopbackPorts.length ? `listens on 127.0.0.1:${m.loopbackPorts.join(", ")} for sign-ins` : "",
    ].filter(Boolean);
    return h(
      "div",
      { class: "plugin-row" },
      h("label", { class: "check" }, box, h("strong", {}, m.name), m.version ? h("span", { class: "muted" }, ` ${m.version}`) : "", p.bundled ? h("span", { class: "muted" }, " · built in") : ""),
      m.description ? h("p", { class: "muted" }, m.description) : null,
      h("p", { class: "muted" }, `Access: ${access.join("; ")}.`),
      state.error ? h("p", { class: "error" }, state.error) : null,
    );
  };

  async function render() {
    const found = await pluginApi.list();
    list.replaceChildren(
      ...(found.length ? found.map(row) : [h("p", { class: "muted" }, "No plugins installed. Put a plugin's folder in the plugins folder, then reopen Settings.")]),
    );
    renderOwn();
  }

  run(render);
  return [
    h("h3", {}, "Plugins"),
    list,
    h("div", { class: "row" }, button("Show plugins folder", () => run(async () => revealItemInDir(await pluginApi.folder())))),
    h(
      "p",
      { class: "muted" },
      "Plugins run inside Honeycomb with the same access to your library and files as the app itself. Only turn on plugins you trust.",
    ),
    own,
  ];
}
