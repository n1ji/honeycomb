import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { store } from "../store";
import { h } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";
import { openImporter } from "./importer";

export const ACCENTS = ["#f5a623", "#e8590c", "#e03131", "#d6336c", "#ae3ec9", "#4c6ef5", "#1c7ed6", "#0ca678", "#74b816"];

/** Relative luminance of a #rrggbb color, 0 (black) to 1 (white). */
function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Variables a custom background/secondary color overrides; cleared when both are unset. */
const CUSTOM_VARS = ["--bg", "--input-bg", "--panel", "--panel-2", "--border", "--text", "--muted", "--row-alt", "--hover", "--shadow", "color-scheme"];

/**
 * Apply theme, accent and custom colors from settings. Themes are CSS variable
 * sets keyed by data-theme; custom colors override them inline, and text and
 * borders follow the background's brightness so they stay readable.
 */
export function applyTheme() {
  const root = document.documentElement;
  root.dataset.theme = store.settings.theme || "system";
  root.style.setProperty("--accent", store.settings.accent || ACCENTS[0]);
  root.dataset.density = store.settings.density || "normal";

  const bg = store.settings.bgColor || "";
  const panel = store.settings.panelColor || "";
  for (const v of CUSTOM_VARS) root.style.removeProperty(v);
  if (!bg && !panel) return;

  const set = (k: string, v: string) => root.style.setProperty(k, v);
  if (bg) {
    set("--bg", bg);
    set("--input-bg", `color-mix(in srgb, ${bg} 85%, black)`);
  }
  if (panel) {
    set("--panel", panel);
    set("--panel-2", `color-mix(in srgb, ${panel} 92%, var(--text))`);
  }
  const light = luminance(bg || panel) > 0.4;
  set("--text", light ? "#1d1d20" : "#e7e7e9");
  set("--muted", light ? "#5f6067" : "#a0a1a8");
  set("--row-alt", light ? "rgba(0, 0, 0, 0.025)" : "rgba(255, 255, 255, 0.022)");
  set("--hover", light ? "rgba(0, 0, 0, 0.05)" : "rgba(255, 255, 255, 0.055)");
  set("--shadow", light ? "rgba(0, 0, 0, 0.18)" : "rgba(0, 0, 0, 0.5)");
  set("--border", `color-mix(in srgb, var(--panel) 84%, var(--text))`);
  set("color-scheme", light ? "light" : "dark");
}

/** Current value of a theme color, as #rrggbb for a color input. */
function themeColor(cssVar: string): string {
  const probe = h("span", {});
  probe.style.color = `var(${cssVar})`;
  document.body.append(probe);
  const m = getComputedStyle(probe).color.match(/\d+(\.\d+)?/g) ?? ["0", "0", "0"];
  probe.remove();
  return "#" + m.slice(0, 3).map((n) => Math.round(Number(n)).toString(16).padStart(2, "0")).join("");
}

async function setSetting(key: string, value: string) {
  store.settings[key] = value;
  applyTheme();
  store.emit("settings");
  await api.setSetting(key, value);
}

function lyricsToggle(): HTMLElement {
  const box = h("input", { type: "checkbox", checked: store.settings.lyricsOnline !== "off" });
  box.addEventListener("change", () => run(() => setSetting("lyricsOnline", box.checked ? "on" : "off")));
  return h("label", { class: "check" }, box, "Look up lyrics online");
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

  /** A color picker bound to a setting, with Reset back to the theme's color. */
  const colorSetting = (key: string, cssVar: string) => {
    const input = h("input", { type: "color", value: store.settings[key] || themeColor(cssVar) });
    input.addEventListener("input", () => run(() => setSetting(key, input.value)));
    const reset = button("Reset", () =>
      run(async () => {
        await setSetting(key, "");
        input.value = themeColor(cssVar);
      }),
    );
    return h("div", { class: "row" }, input, reset);
  };
  theme.addEventListener("change", () => {
    // Pickers that follow the theme show its new colors.
    for (const [key, cssVar, el] of colorRows) {
      if (!store.settings[key]) (el.querySelector("input") as HTMLInputElement).value = themeColor(cssVar);
    }
  });
  const colorRows: [string, string, HTMLElement][] = [
    ["bgColor", "--bg", colorSetting("bgColor", "--bg")],
    ["panelColor", "--panel", colorSetting("panelColor", "--panel")],
  ];

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
    h("h3", {}, "Lyrics"),
    lyricsToggle(),
    h(
      "p",
      { class: "muted" },
      "Lyrics in the file or a .lrc file next to it are always used. Online lookup asks LRCLIB (lrclib.net, a free lyrics database) for the rest, sending the song's title, artist, album and length.",
    ),
    h("h3", {}, "Appearance"),
    h("label", { class: "field inline" }, h("span", {}, "Theme"), theme),
    h("label", { class: "field inline" }, h("span", {}, "Row density"), density),
    h("div", { class: "field inline" }, h("span", {}, "Accent color"), swatches),
    h("div", { class: "field inline" }, h("span", {}, "Background"), colorRows[0][2]),
    h("div", { class: "field inline" }, h("span", {}, "Secondary"), colorRows[1][2]),
    h("p", { class: "muted" }, "Background is the track list and browser; secondary is the sidebar, headers and panels. Text switches between light and dark to stay readable."),
  );
  m.footer.append(button("Done", m.close, true));
  renderSwatches();
  await renderFolders();
}
