import { api } from "../api";
import { store } from "../store";
import { h, isMac } from "../util";

/** Listening: albums, artists and what's playing. Advanced: the browser, columns, tags and the Inbox. */
export type Mode = "listening" | "advanced";

export const mode = (): Mode => (store.settings.mode === "listening" ? "listening" : "advanced");

export function applyMode() {
  document.documentElement.dataset.mode = mode();
}

export async function setMode(m: Mode) {
  if (m === mode()) return;
  store.settings.mode = m;
  applyMode();
  store.emit("mode");
  await api.setSetting("mode", m);
}

export const toggleMode = () => setMode(mode() === "listening" ? "advanced" : "listening");

/** Filled in by Listening mode, so the player bar can open its Now Playing view. */
export const nowView = { toggle: () => {} };

const SHORTCUT = `${isMac ? "⌘" : "Ctrl+"}L`;

/** The two-way switch; one sits in each mode's chrome. */
export function modeSwitch(): HTMLElement {
  const btn = (m: Mode, label: string, hint: string) =>
    h("button", { type: "button", "data-mode": m, title: `${hint} (${SHORTCUT} switches)`, onclick: () => void setMode(m) }, label);
  const el = h(
    "div",
    { class: "mode-switch", role: "group", "aria-label": "Interface" },
    btn("listening", "Listening", "Just play music"),
    btn("advanced", "Advanced", "Manage the library: tags, columns, Inbox"),
  );
  const render = () => {
    for (const b of el.querySelectorAll<HTMLElement>("button")) b.classList.toggle("on", b.dataset.mode === mode());
  };
  store.on("mode", render);
  render();
  return el;
}
