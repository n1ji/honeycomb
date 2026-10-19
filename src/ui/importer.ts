import { open, save } from "@tauri-apps/plugin-dialog";
import { api, ImportPreview } from "../api";
import { store } from "../store";
import { h, toast } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";

const fileName = (p: string) => p.split(/[\\/]/).pop() || p;
const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

/** Import ratings, play stats and playlists from another player's export. */
export function openImporter() {
  const m = modal("Import from another player", { wide: true });
  let xmlPath: string | null = null;
  let m3uPaths: string[] = [];

  const xmlLabel = h("span", { class: "muted" });
  const m3uLabel = h("span", { class: "muted" });
  const previewBtn = button("Preview", () => run(showPreview), true);

  const refresh = () => {
    xmlLabel.textContent = xmlPath ? fileName(xmlPath) : "None";
    xmlLabel.title = xmlPath ?? "";
    m3uLabel.textContent = m3uPaths.length ? m3uPaths.map(fileName).join(", ") : "None";
    m3uLabel.title = m3uPaths.join("\n");
    previewBtn.disabled = !xmlPath && !m3uPaths.length;
  };

  const chooseXml = () =>
    run(async () => {
      const p = await open({ multiple: false, filters: [{ name: "Library XML", extensions: ["xml"] }] });
      if (typeof p === "string") xmlPath = p;
      refresh();
    });
  const addM3uFiles = () =>
    run(async () => {
      const p = await open({ multiple: true, filters: [{ name: "Playlists", extensions: ["m3u", "m3u8"] }] });
      if (Array.isArray(p)) m3uPaths = [...new Set([...m3uPaths, ...p])];
      refresh();
    });
  const addM3uFolder = () =>
    run(async () => {
      const p = await open({ directory: true, multiple: false, title: "Folder of playlists" });
      if (typeof p === "string") m3uPaths = [...new Set([...m3uPaths, p])];
      refresh();
    });

  function showChoose() {
    m.body.replaceChildren(
      h(
        "p",
        { class: "muted" },
        "Brings over ratings, play counts, play dates and loved tracks from an iTunes-format library XML, which many players can export, and playlists from that XML or from M3U files. Add your music folders and let the scan finish first: only tracks already in your library can be matched.",
      ),
      h("h3", {}, "Library XML"),
      h("div", { class: "row" }, button("Choose…", chooseXml), button("Clear", () => ((xmlPath = null), refresh())), xmlLabel),
      h("h3", {}, "Playlists (M3U)"),
      h(
        "div",
        { class: "row" },
        button("Add files…", addM3uFiles),
        button("Add folder…", addM3uFolder),
        button("Clear", () => ((m3uPaths = []), refresh())),
      ),
      h("div", { class: "import-paths" }, m3uLabel),
    );
    m.footer.replaceChildren(button("Cancel", m.close), previewBtn);
    refresh();
  }

  async function showPreview() {
    previewBtn.disabled = true;
    previewBtn.textContent = "Reading…";
    let p: ImportPreview;
    try {
      p = await api.importPreview(xmlPath, m3uPaths);
    } finally {
      previewBtn.textContent = "Preview";
      refresh();
    }
    renderPreview(p);
  }

  function renderPreview(p: ImportPreview) {
    const body: (Node | string)[] = [];
    if (!p.libraryTracks) {
      body.push(h("p", { class: "import-warn" }, "Your library is empty, so nothing can be matched. Add your music folders in Settings first."));
    }

    const statsBox = h("input", { type: "checkbox", checked: !!p.stats?.matched });
    if (p.stats) {
      const s = p.stats;
      body.push(
        h("h3", {}, "Ratings and play counts"),
        h(
          "p",
          {},
          `${plural(s.tracks, "track")} in the export: ${s.matched.toLocaleString()} found in your library` +
            (s.unmatched ? `, ${s.unmatched.toLocaleString()} not found.` : "."),
        ),
        h(
          "label",
          { class: "check" },
          statsBox,
          `Import ${plural(s.rated, "rating")}, ${plural(s.played, "play count")} and ${s.loved.toLocaleString()} loved`,
        ),
        h(
          "p",
          { class: "muted" },
          "Existing data is kept where it's better: the higher play count, the newer last-played date, the older date added, and your own rating if you've already rated a track.",
        ),
      );
    }

    // Default: tick playlists that matched something, and only the best one of any duplicate name.
    const best = new Map<string, number>();
    for (const pl of p.playlists) {
      const cur = best.get(pl.name);
      if (pl.matched && (cur === undefined || pl.matched > p.playlists[cur].matched)) best.set(pl.name, pl.index);
    }
    const boxes = new Map<number, HTMLInputElement>();
    if (p.playlists.length) {
      const list = h("div", { class: "import-list" });
      for (const pl of p.playlists) {
        const box = h("input", { type: "checkbox", checked: best.get(pl.name) === pl.index, disabled: !pl.matched });
        boxes.set(pl.index, box);
        const notes = [
          pl.placeholders && `${pl.placeholders.toLocaleString()} placeholder ${pl.placeholders === 1 ? "entry" : "entries"} skipped`,
          pl.missing && `${pl.missing.toLocaleString()} not found`,
          pl.exists && "replaces existing playlist",
        ].filter(Boolean);
        list.append(
          h(
            "label",
            { class: `import-item${pl.matched ? "" : " empty"}` },
            box,
            h("span", { class: "name", title: pl.source }, pl.name),
            h("span", { class: "count" }, `${pl.matched.toLocaleString()} of ${pl.total.toLocaleString()}`),
            h("span", { class: "notes" }, notes.join(" · ")),
          ),
        );
      }
      const setAll = (on: boolean) => boxes.forEach((b) => (b.checked = on && !b.disabled));
      body.push(
        h("h3", {}, `Playlists (${p.playlists.length})`),
        h("div", { class: "row" }, button("Select all", () => setAll(true)), button("Select none", () => setAll(false))),
        list,
        h(
          "p",
          { class: "muted" },
          "Placeholder entries (\"Unknown Title\") stand for tracks the other player had lost, so they can't be recovered. Playlists built automatically, like recently added or most played, are better recreated as smart playlists.",
        ),
      );
    }
    m.body.replaceChildren(...body);

    const saveReport = () =>
      run(async () => {
        const path = await save({ defaultPath: "import report.txt", filters: [{ name: "Text", extensions: ["txt"] }] });
        if (path) await api.saveText(path, p.report);
      });
    const doImport = () =>
      run(async () => {
        const chosen = [...boxes].filter(([, b]) => b.checked).map(([i]) => i);
        const r = await api.importApply(xmlPath, m3uPaths, statsBox.checked, chosen);
        m.close();
        await store.loadTracks(true);
        await store.loadPlaylists();
        const parts = [
          statsBox.checked && p.stats && `updated ${plural(r.statsUpdated, "track")}`,
          r.playlistsCreated && `created ${plural(r.playlistsCreated, "playlist")}`,
          r.playlistsReplaced && `replaced ${plural(r.playlistsReplaced, "playlist")}`,
          r.placeholdersSkipped && `skipped ${r.placeholdersSkipped.toLocaleString()} placeholder entries`,
          r.missingSkipped && `${r.missingSkipped.toLocaleString()} entries not found`,
        ].filter(Boolean);
        toast(parts.length ? `Import done: ${parts.join(", ")}.` : "Nothing was imported.");
      });
    m.footer.replaceChildren(
      button("Back", showChoose),
      ...(p.report ? [button("Save report…", saveReport)] : []),
      button("Import", doImport, true),
    );
  }

  showChoose();
}
