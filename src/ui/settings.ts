import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { store } from "../store";
import { h } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";
import { openImporter } from "./importer";

export const ACCENTS = ["#f5a623", "#e8590c", "#e03131", "#d6336c", "#ae3ec9", "#4c6ef5", "#1c7ed6", "#0ca678", "#74b816"];

/** Apply theme and accent from settings. Themes are CSS variable sets keyed by data-theme. */
export function applyTheme() {
  const root = document.documentElement;
  root.dataset.theme = store.settings.theme || "system";
  root.style.setProperty("--accent", store.settings.accent || ACCENTS[0]);
  root.dataset.density = store.settings.density || "normal";
}

async function setSetting(key: string, value: string) {
  store.settings[key] = value;
  applyTheme();
  store.emit("settings");
  await api.setSetting(key, value);
}

export async function openSettings() {
  const m = modal("Settings", { wide: true });

  // Library folders
  const folderList = h("ul", { class: "folders" });
  const renderFolders = async () => {
    const folders = await api.getFolders();
    folderList.replaceChildren(
      ...(folders.length
        ? folders.map((f) =>
            h(
              "li",
              {},
              h("span", { title: f }, f),
              button("Remove", () =>
                run(async () => {
                  await api.removeFolder(f);
                  await renderFolders();
                }),
              ),
            ),
          )
        : [h("li", { class: "muted" }, "No folders yet.")]),
    );
  };

  const addFolder = () =>
    run(async () => {
      const dir = await open({ directory: true, multiple: false, title: "Add a music folder" });
      if (typeof dir !== "string") return;
      await api.addFolder(dir);
      await renderFolders();
    });

  // Appearance
  const theme = h("select", { class: "input" });
  for (const [k, label] of [
    ["system", "Match system"],
    ["dark", "Dark"],
    ["light", "Light"],
    ["midnight", "Midnight"],
  ]) {
    theme.append(h("option", { value: k, selected: (store.settings.theme || "system") === k }, label));
  }
  theme.addEventListener("change", () => run(() => setSetting("theme", theme.value)));

  const density = h("select", { class: "input" });
  for (const [k, label] of [
    ["normal", "Normal"],
    ["compact", "Compact"],
  ]) {
    density.append(h("option", { value: k, selected: (store.settings.density || "normal") === k }, label));
  }
  density.addEventListener("change", () => run(() => setSetting("density", density.value)));

  const swatches = h("div", { class: "swatches" });
  const custom = h("input", { type: "color", value: store.settings.accent || ACCENTS[0], title: "Custom color" });
  const renderSwatches = () => {
    const cur = store.settings.accent || ACCENTS[0];
    swatches.replaceChildren(
      ...ACCENTS.map((c) => {
        const s = h("button", {
          class: `swatch${c === cur ? " active" : ""}`,
          title: c,
          type: "button",
          onclick: () =>
            run(async () => {
              await setSetting("accent", c);
              custom.value = c;
              renderSwatches();
            }),
        });
        s.style.background = c;
        return s;
      }),
      custom,
    );
  };
  custom.addEventListener("input", () =>
    run(async () => {
      await setSetting("accent", custom.value);
      renderSwatches();
    }),
  );

  m.body.append(
    h("h3", {}, "Library"),
    folderList,
    h(
      "div",
      { class: "row" },
      button("Add folder…", addFolder, true),
      button("Rescan now", () => run(api.rescan)),
      button("Import from another player…", () => {
        m.close();
        openImporter();
      }),
    ),
    h("p", { class: "muted" }, `${store.tracks.size.toLocaleString()} tracks in library. Honeycomb never moves or deletes your files.`),
    h("h3", {}, "Appearance"),
    h("label", { class: "field inline" }, h("span", {}, "Theme"), theme),
    h("label", { class: "field inline" }, h("span", {}, "Row density"), density),
    h("div", { class: "field inline" }, h("span", {}, "Accent color"), swatches),
  );
  m.footer.append(button("Done", m.close, true));
  renderSwatches();
  await renderFolders();
}
