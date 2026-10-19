import { api } from "../api";
import { store } from "../store";
import { h } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";
import { applyLayout } from "./layout";
import { PB_BUTTONS, PB_PARTS, pbButtons, pbHidden } from "./playerbar";
import { COLS, shownColumns } from "./tracklist";

export interface OrderItem {
  id: string;
  label: string;
  /** Always shown (it can still be moved). */
  locked?: boolean;
}

/** Save a layout setting and let every part of the UI redraw. */
export async function setLayoutSetting(key: string, value: string) {
  store.settings[key] = value;
  applyLayout();
  store.emit("settings");
  await api.setSetting(key, value);
}

/**
 * Items the user ticks on or off and moves up or down. `shown` returns the
 * ticked ids in order; hidden items are listed after them.
 */
function orderList(all: OrderItem[], shown: () => string[], save: (ids: string[]) => Promise<void>): HTMLElement {
  const list = h("ul", { class: "order-list" });
  const render = () => {
    const on = shown();
    const hidden = all.filter((i) => !on.includes(i.id));
    const byId = new Map(all.map((i) => [i.id, i]));
    const update = (ids: string[]) => run(async () => {
      await save(ids);
      render();
    });
    const move = (from: number, to: number) => {
      const ids = [...on];
      ids.splice(to, 0, ids.splice(from, 1)[0]);
      update(ids);
    };
    list.replaceChildren(
      ...on.map((id, i) => {
        const item = byId.get(id)!;
        const box = h("input", { type: "checkbox", checked: true, disabled: !!item.locked });
        box.addEventListener("change", () => update(on.filter((x) => x !== id)));
        return h(
          "li",
          {},
          h("label", { class: "check" }, box, item.label),
          h("button", { class: "btn move", type: "button", title: "Move up", disabled: i === 0, onclick: () => move(i, i - 1) }, "↑"),
          h("button", { class: "btn move", type: "button", title: "Move down", disabled: i === on.length - 1, onclick: () => move(i, i + 1) }, "↓"),
        );
      }),
      ...hidden.map((item) => {
        const box = h("input", { type: "checkbox", checked: false });
        box.addEventListener("change", () => update([...on, item.id]));
        return h("li", { class: "off" }, h("label", { class: "check" }, box, item.label));
      }),
    );
  };
  render();
  return list;
}

export function openPlayerBarCustomizer() {
  const m = modal("Customize player bar");
  const buttons = orderList(
    PB_BUTTONS.map((b) => ({ id: b.id, label: b.label, locked: b.id === "play" })),
    pbButtons,
    (ids) => setLayoutSetting("layout.pbButtons", ids.join(" ")),
  );
  const parts = h(
    "div",
    { class: "order-parts" },
    ...PB_PARTS.map((p) => {
      const box = h("input", { type: "checkbox", checked: !pbHidden().includes(p.id) });
      box.addEventListener("change", () => {
        const hidden = pbHidden().filter((x) => x !== p.id);
        if (!box.checked) hidden.push(p.id);
        run(() => setLayoutSetting("layout.pbHidden", hidden.join(" ")));
      });
      return h("label", { class: "check" }, box, p.label);
    }),
  );
  m.body.append(h("h3", {}, "Buttons"), buttons, h("h3", {}, "Show"), parts);
  m.footer.append(
    button("Reset", () =>
      run(async () => {
        await setLayoutSetting("layout.pbButtons", "");
        await setLayoutSetting("layout.pbHidden", "");
        m.close();
        openPlayerBarCustomizer();
      }),
    ),
    button("Done", m.close, true),
  );
}

export function openColumnsCustomizer() {
  const m = modal("Track list columns");
  const list = orderList(
    COLS.map((c) => ({ id: c.key, label: c.menuLabel ?? c.label, locked: c.key === "title" })),
    shownColumns,
    (ids) => setLayoutSetting("layout.columnsShown", ids.join(" ")),
  );
  m.body.append(
    h("p", { class: "muted" }, "Tick the columns to show. You can also drag column headers to reorder them, and right-click a header to show or hide columns."),
    list,
  );
  m.footer.append(
    button("Reset", () =>
      run(async () => {
        await setLayoutSetting("layout.columnsShown", "");
        m.close();
        openColumnsCustomizer();
      }),
    ),
    button("Done", m.close, true),
  );
}
