import { api, Playlist, SmartRule, SmartRules } from "../api";
import { store } from "../store";
import { h, toast } from "../util";
import { button, modal } from "./dialogs";

type Kind = "text" | "num" | "date";

const FIELDS: { key: string; label: string; kind: Kind }[] = [
  { key: "title", label: "Title", kind: "text" },
  { key: "artist", label: "Artist", kind: "text" },
  { key: "albumArtist", label: "Album artist", kind: "text" },
  { key: "album", label: "Album", kind: "text" },
  { key: "genre", label: "Genre", kind: "text" },
  { key: "composer", label: "Composer", kind: "text" },
  { key: "comment", label: "Comment", kind: "text" },
  { key: "format", label: "Format", kind: "text" },
  { key: "path", label: "File path", kind: "text" },
  { key: "year", label: "Year", kind: "num" },
  { key: "rating", label: "Rating (stars, 3.5 = ★★★½)", kind: "num" },
  { key: "loved", label: "Loved (1 = yes, 0 = no)", kind: "num" },
  { key: "playCount", label: "Play count", kind: "num" },
  { key: "duration", label: "Length (seconds)", kind: "num" },
  { key: "bitrate", label: "Bitrate (kbps)", kind: "num" },
  { key: "dateAdded", label: "Date added", kind: "date" },
  { key: "lastPlayed", label: "Last played", kind: "date" },
];

const OPS: Record<Kind, { key: string; label: string }[]> = {
  text: [
    { key: "contains", label: "contains" },
    { key: "notContains", label: "does not contain" },
    { key: "is", label: "is" },
    { key: "isNot", label: "is not" },
    { key: "startsWith", label: "starts with" },
  ],
  num: [
    { key: "is", label: "is" },
    { key: "isNot", label: "is not" },
    { key: "gt", label: "is greater than" },
    { key: "lt", label: "is less than" },
  ],
  date: [
    { key: "inLast", label: "in the last (days)" },
    { key: "notInLast", label: "not in the last (days)" },
  ],
};

const kindOf = (field: string): Kind => FIELDS.find((f) => f.key === field)?.kind ?? "text";

function select(options: { key: string; label: string }[], value: string, onchange: (v: string) => void) {
  const s = h("select", { class: "input" });
  for (const o of options) s.append(h("option", { value: o.key, selected: o.key === value }, o.label));
  s.addEventListener("change", () => onchange(s.value));
  return s;
}

/** Create a smart playlist, or edit the rules of `existing`. */
export function openSmartEditor(existing?: Playlist) {
  const rules: SmartRules = existing?.rules
    ? structuredClone(existing.rules)
    : { match: "all", rules: [{ field: "genre", op: "contains", value: "" }], limit: null, sortBy: null, sortDesc: false };

  const m = modal(existing ? `Edit ${existing.name}` : "New smart playlist", { wide: true });
  const name = h("input", { type: "text", class: "input", value: existing?.name ?? "New smart playlist" });
  const list = h("div", { class: "rules" });
  const count = h("span", { class: "muted" });

  let previewTimer = 0;
  const preview = () => {
    clearTimeout(previewTimer);
    previewTimer = window.setTimeout(async () => {
      try {
        const n = await api.previewSmart(rules);
        count.textContent = `${n} matching track${n === 1 ? "" : "s"}`;
      } catch (e) {
        count.textContent = String(e);
      }
    }, 250);
  };

  const renderRules = () => {
    list.replaceChildren(
      ...rules.rules.map((r: SmartRule, i) => {
        const value = h("input", { type: "text", class: "input", value: r.value });
        value.addEventListener("input", () => {
          r.value = value.value;
          preview();
        });
        return h(
          "div",
          { class: "rule" },
          select(FIELDS, r.field, (v) => {
            const changedKind = kindOf(v) !== kindOf(r.field);
            r.field = v;
            if (changedKind) {
              r.op = OPS[kindOf(v)][0].key;
              r.value = "";
            }
            renderRules();
            preview();
          }),
          select(OPS[kindOf(r.field)], r.op, (v) => {
            r.op = v;
            preview();
          }),
          value,
          button("−", () => {
            rules.rules.splice(i, 1);
            renderRules();
            preview();
          }),
        );
      }),
    );
  };

  const limit = h("input", { type: "number", min: 1, class: "input short", value: rules.limit ?? "" });
  limit.addEventListener("input", () => {
    rules.limit = limit.value ? Math.max(1, Number(limit.value)) : null;
    preview();
  });
  const sortOptions = [{ key: "", label: "Library order" }, { key: "random", label: "Random" }, ...FIELDS];
  const desc = h("input", { type: "checkbox", checked: rules.sortDesc });
  desc.addEventListener("change", () => (rules.sortDesc = desc.checked));

  m.body.append(
    h("label", { class: "field" }, h("span", {}, "Name"), name),
    h(
      "div",
      { class: "row" },
      h("span", {}, "Match"),
      select(
        [
          { key: "all", label: "all" },
          { key: "any", label: "any" },
        ],
        rules.match,
        (v) => {
          rules.match = v as "all" | "any";
          preview();
        },
      ),
      h("span", {}, "of these rules:"),
    ),
    list,
    h(
      "div",
      { class: "row" },
      button("+ Add rule", () => {
        rules.rules.push({ field: "artist", op: "contains", value: "" });
        renderRules();
      }),
    ),
    h(
      "div",
      { class: "row" },
      h("span", {}, "Limit to"),
      limit,
      h("span", {}, "tracks, sorted by"),
      select(sortOptions, rules.sortBy ?? "", (v) => (rules.sortBy = v || null)),
      h("label", { class: "check" }, desc, "descending"),
    ),
    count,
  );

  m.footer.append(
    button("Cancel", m.close),
    button(
      "Save",
      async () => {
        const n = name.value.trim() || "Smart playlist";
        try {
          if (existing) {
            await api.updatePlaylist(existing.id, n, rules);
            await store.loadPlaylists();
            if (store.currentPlaylist?.id === existing.id) await store.refreshView();
          } else {
            const p = await api.createPlaylist(n, rules);
            await store.loadPlaylists();
            await store.setView({ kind: "playlist", id: p.id });
          }
          m.close();
        } catch (e) {
          toast(String(e), true);
        }
      },
      true,
    ),
  );
  renderRules();
  preview();
  name.focus();
}
