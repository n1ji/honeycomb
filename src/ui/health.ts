import { ask } from "@tauri-apps/plugin-dialog";
import { api, TagEdit, Track } from "../api";
import { Category, CATEGORIES, checkLibrary, Issue } from "../health";
import { store } from "../store";
import { h, toast } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";

const SHOWN = 150;
const FIELD: Record<string, string> = { album: "Album", albumArtist: "Album artist", year: "Year" };
const describe = (fix: TagEdit) =>
  Object.entries(fix)
    .map(([k, v]) => `${FIELD[k] ?? k}: ${v}`)
    .join(", ");

/** Files on this computer that belong in the library (server songs can't be retagged from here). */
const localTracks = (): Track[] => [...store.tracks.values()].filter((t) => !t.inbox && !t.path.startsWith("subsonic://"));

/** Look for split albums, track number problems, missing covers and the like, and fix what tags can fix. */
export function openHealth() {
  const m = modal("Library health", { wide: true });
  m.el.classList.add("health");
  let issues: Issue[] = [];
  let only: Category | null = null;
  const cats = h("div", { class: "health-cats" });
  const list = h("div", { class: "health-list" });

  const fixAll = button("", () =>
    run(async () => {
      const todo = issues.filter((i) => i.fix && Object.keys(i.fix).length);
      const ok = await ask(`Write album tags to the songs of ${todo.length} albums? Each album gets its most common album name, album artist and year.`, {
        title: "Fix split albums",
        kind: "warning",
        okLabel: "Fix all",
      });
      if (!ok) return;
      for (const i of todo) await apply(i);
      toast(`Fixed ${todo.length} albums.`);
      refresh();
    }),
  );

  async function apply(i: Issue) {
    store.updateTracks(await api.editTags(i.ids, i.fix!));
  }

  function refresh() {
    issues = checkLibrary(localTracks());
    render();
  }

  function render() {
    const counts = new Map<Category, number>();
    for (const i of issues) counts.set(i.category, (counts.get(i.category) ?? 0) + 1);
    if (only && !counts.get(only)) only = null;
    const chip = (c: Category | null, label: string, n: number) =>
      h("button", { type: "button", class: `chip${only === c ? " on" : ""}`, onclick: () => ((only = c), render()) }, `${label} `, h("span", { class: "muted" }, n.toLocaleString()));
    cats.replaceChildren(
      chip(null, "All", issues.length),
      ...(Object.keys(CATEGORIES) as Category[]).filter((c) => counts.get(c)).map((c) => chip(c, CATEGORIES[c], counts.get(c)!)),
    );
    const shown = issues.filter((i) => !only || i.category === only);
    list.replaceChildren(
      ...(shown.length ? shown.slice(0, SHOWN).map(row) : [h("p", { class: "muted health-empty" }, "Nothing to fix. Your library looks healthy!")]),
      ...(shown.length > SHOWN ? [h("p", { class: "muted" }, `Showing ${SHOWN} of ${shown.length.toLocaleString()}. Fix some, or pick a kind above.`)] : []),
    );
    const fixable = issues.filter((i) => i.fix && Object.keys(i.fix).length).length;
    fixAll.textContent = `Fix ${fixable} split album${fixable === 1 ? "" : "s"}`;
    fixAll.hidden = fixable < 2;
  }

  function row(i: Issue): HTMLElement {
    const fixable = i.fix && Object.keys(i.fix).length > 0;
    return h(
      "div",
      { class: "health-issue" },
      h("div", { class: "health-head" }, h("strong", {}, i.title), i.artist ? h("span", { class: "muted" }, ` · ${i.artist}`) : "", h("span", { class: "health-tag" }, CATEGORIES[i.category])),
      h("div", {}, i.summary),
      i.details.length ? h("details", {}, h("summary", { class: "muted" }, "Details"), h("pre", {}, i.details.join("\n"))) : "",
      fixable ? h("div", { class: "muted health-fix" }, `Fix writes ${describe(i.fix!)}`) : "",
      h(
        "div",
        { class: "row" },
        button("Show songs", () => {
          m.close();
          run(() => store.showFiltered({ album: store.tracks.get(i.ids[0])?.album ?? i.title }));
        }),
        fixable
          ? button(
              "Fix",
              () =>
                run(async () => {
                  await apply(i);
                  toast(`Fixed “${i.title}”.`);
                  refresh();
                }),
              true,
            )
          : "",
      ),
    );
  }

  m.body.replaceChildren(
    h("p", { class: "muted" }, "Checks the music files on this computer for things that make the library look messy. Only Fix changes anything, and only the album tags it lists."),
    cats,
    list,
  );
  m.footer.replaceChildren(button("Check again", refresh), fixAll, button("Close", m.close, true));
  refresh();
}
