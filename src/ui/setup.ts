import { open } from "@tauri-apps/plugin-dialog";
import { api } from "../api";
import { enabledIds, ListedPlugin, pluginApi, setEnabled } from "../plugins";
import { store } from "../store";
import { h } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";
import { openImporter } from "./importer";

/**
 * Bumped when the setup gets a step people who finished it should still see
 * (e.g. sound analysis). `setup.done` holds the version last finished.
 */
const SETUP_VERSION = 1;

export function needsSetup(): boolean {
  return Number(store.settings["setup.done"] || 0) < SETUP_VERSION;
}

/**
 * The first-start setup: music folders and importing for new users, then the
 * listening history question and the plugins for everyone. People who used
 * Honeycomb before it existed only get the last two. Closing it without
 * finishing asks again on the next start.
 */
export async function openSetup() {
  const fresh = (await api.getFolders()).length === 0 && store.tracks.size === 0;
  const plugins = (await pluginApi.list()).filter((p) => p.bundled && p.manifest);
  const steps = [...(fresh ? [foldersStep, importStep] : []), historyStep, pluginsStep(plugins)];

  const m = modal("", { wide: true });
  m.el.classList.add("setup");
  const title = m.el.querySelector("h2")!;
  const progress = h("span", { class: "muted setup-progress" });
  const back = button("Back", () => show(at - 1));
  const next = button("Next", () => run(advance), true);
  m.footer.append(progress, back, next);

  let at = 0;
  let answer: Answer = { ok: true };
  const setAnswer = (a: Answer) => {
    answer = a;
    next.disabled = !a.ok;
  };
  function show(i: number) {
    // Slide the new step in from the side it's coming from.
    m.body.style.setProperty("--step-from", i < at ? "-14px" : "14px");
    m.body.classList.remove("step-in");
    void m.body.offsetWidth;
    m.body.classList.add("step-in");
    at = i;
    const step = steps[i];
    title.textContent = step.title;
    progress.textContent = steps.length > 1 ? `Step ${i + 1} of ${steps.length}` : "";
    back.style.visibility = i > 0 ? "visible" : "hidden";
    next.textContent = i === steps.length - 1 ? "Finish" : "Next";
    // Reset first: a step may give its answer while rendering.
    setAnswer({ ok: true });
    m.body.replaceChildren(...step.render(setAnswer, m.close));
  }
  async function advance() {
    if (!answer.ok) return;
    await answer.apply?.();
    if (at < steps.length - 1) return show(at + 1);
    store.settings["setup.done"] = String(SETUP_VERSION);
    await api.setSetting("setup.done", String(SETUP_VERSION));
    m.close();
  }
  show(0);
}

/** What a step wants done when the user moves on, and whether they may yet. */
interface Answer {
  ok: boolean;
  apply?: () => Promise<void>;
}

interface Step {
  title: string;
  render: (answer: (a: Answer) => void, close: () => void) => HTMLElement[];
}

const foldersStep: Step = {
  title: "Welcome to Honeycomb",
  render: () => {
    const list = h("ul", { class: "folders" });
    const renderList = async () => {
      const folders = await api.getFolders();
      list.replaceChildren(
        ...(folders.length ? folders.map((f) => h("li", {}, h("span", { title: f }, f))) : [h("li", { class: "muted" }, "No folders yet.")]),
      );
    };
    void renderList();
    return [
      h("p", {}, "Let's get your music in. Pick the folders your music lives in; Honeycomb scans them in the background and never moves or changes your files."),
      list,
      h(
        "div",
        { class: "row" },
        button(
          "Add folder…",
          () =>
            run(async () => {
              const dir = await open({ directory: true, multiple: false, title: "Add a music folder" });
              if (typeof dir !== "string") return;
              await api.addFolder(dir);
              await renderList();
            }),
          true,
        ),
      ),
      h("p", { class: "muted" }, "You can add more later in Settings → Library."),
    ];
  },
};

const importStep: Step = {
  title: "Coming from another player?",
  render: (_answer, close) => [
    h(
      "p",
      {},
      "Honeycomb can bring over your play counts, ratings, play dates and playlists from an iTunes-format library XML (many players can export one) and from M3U playlists.",
    ),
    h(
      "div",
      { class: "row" },
      button("Import from another player…", () => {
        // The importer is its own window; setup comes back on the next start for the rest.
        close();
        openImporter();
      }),
    ),
    h("p", { class: "muted" }, "Starting fresh? Just go on; you can import any time from Settings → Library."),
  ],
};

const historyStep: Step = {
  title: "Your listening history",
  render: (answer) => {
    const choice = (value: "on" | "off", label: string, detail: string) => {
      const input = h("input", { type: "radio", name: "listenLog", value });
      // Nothing is picked for people who never answered: they have to choose.
      input.checked = store.settings.listenLog === value;
      input.addEventListener("change", pick);
      return h("label", { class: "setup-choice" }, input, h("span", {}, h("strong", {}, label), h("span", { class: "muted" }, detail)));
    };
    const yes = choice("on", "Yes, keep a listening history", "Recommended if you'd like playlists that fit your taste.");
    const no = choice("off", "No thanks", "Smart mixes won't work without it. You can turn it on later.");
    function pick() {
      const picked = (yes.querySelector("input") as HTMLInputElement).checked ? "on" : (no.querySelector("input") as HTMLInputElement).checked ? "off" : null;
      answer(
        picked
          ? {
              ok: true,
              apply: async () => {
                store.settings.listenLog = picked;
                await api.setSetting("listenLog", picked);
              },
            }
          : { ok: false },
      );
    }
    queueMicrotask(pick);
    return [
      h(
        "p",
        {},
        "Honeycomb can remember which songs you play, when, and where in a song you skipped or stopped. Later, the Smart mixes plugin uses that to make playlists from what you actually like, instead of only what's similar.",
      ),
      h(
        "ul",
        { class: "setup-facts" },
        h("li", {}, "It stays on this computer. It isn't sent anywhere, not to us and not to anyone else."),
        h("li", {}, "Without it, Smart mixes won't work, since there's nothing to learn from."),
        h("li", {}, "You can turn it off or clear it any time in Settings → Listening history."),
      ),
      yes,
      no,
    ];
  },
};

function pluginsStep(plugins: ListedPlugin[]): Step {
  return {
    title: "Plugins",
    render: (answer) => {
      const picked = new Set(enabledIds());
      const rows = plugins.map((p) => {
        const m = p.manifest!;
        const box = h("input", { type: "checkbox", checked: picked.has(m.id) });
        box.addEventListener("change", () => (box.checked ? picked.add(m.id) : picked.delete(m.id)));
        return h(
          "label",
          { class: "setup-choice" },
          box,
          h("span", {}, h("strong", {}, m.name), m.description ? h("span", { class: "muted" }, m.description) : null),
        );
      });
      answer({
        ok: true,
        apply: async () => {
          for (const p of plugins) {
            const m = p.manifest!;
            if (picked.has(m.id) !== enabledIds().includes(m.id)) await setEnabled(m, picked.has(m.id));
          }
        },
      });
      return [
        h("p", {}, "Extras that come with Honeycomb. They're all off until you turn them on, so skip any you don't need."),
        ...(rows.length ? rows : [h("p", { class: "muted" }, "No plugins came with this build.")]),
        h("p", { class: "muted" }, "Plugins that need an account (like scrobbling) ask you to sign in in Settings → Plugins."),
      ];
    },
  };
}
