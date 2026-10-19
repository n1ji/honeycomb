import { api, UpdateStatus } from "../api";
import { h } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";

const RECHECK_MS = 30 * 60 * 1000;

/**
 * A small bulb in the sidebar footer: dim when this build is current, lit with
 * a count when GitHub's main has newer builds. Click for details.
 */
export function mountUpdateBulb(): HTMLElement {
  const count = h("span", { class: "count" });
  const bulb = h("button", { class: "bulb", type: "button", title: "Checking for updates…" }, "💡");
  let last: UpdateStatus | null = null;

  const render = (s: UpdateStatus) => {
    last = s;
    bulb.className = `bulb ${s.state}`;
    bulb.title = s.message;
    count.textContent = s.behind > 99 ? "99+" : String(s.behind);
    if (s.behind > 0) bulb.append(count);
    else count.remove();
  };
  const check = (refresh = false) =>
    api
      .updateStatus(refresh)
      .then(render)
      .catch(() => {});

  bulb.addEventListener("click", () => openDetails(last, check));
  check();
  window.setInterval(() => check(true), RECHECK_MS);
  return bulb;
}

function openDetails(s: UpdateStatus | null, check: (refresh?: boolean) => Promise<void>) {
  const m = modal("Updates");
  const body = () => {
    if (!s) {
      m.body.replaceChildren(h("p", { class: "muted" }, "Still checking…"));
      return;
    }
    const short = (c: string) => (c ? c.slice(0, 7) : "—");
    const rows: (Node | string)[] = [
      h("p", {}, s.message),
      h("p", { class: "muted" }, `This build: ${short(s.local)} · GitHub main: ${short(s.remote)}`),
    ];
    if (s.state === "behind" || s.state === "diverged") {
      rows.push(
        h("p", {}, "Quit Honeycomb, then run this in a terminal to update and rebuild:"),
        h("code", { class: "update-cmd" }, `cd "${s.repo}" && git pull && npm ci && npm run tauri build`),
      );
      if (s.state === "diverged") {
        rows.push(h("p", { class: "muted" }, "Your local commits will be merged with GitHub's; git may ask you to resolve conflicts."));
      }
    }
    m.body.replaceChildren(...rows);
  };
  const recheck = button("Check now", () =>
    run(async () => {
      recheck.disabled = true;
      recheck.textContent = "Checking…";
      try {
        s = await api.updateStatus(true);
        await check();
      } finally {
        recheck.disabled = false;
        recheck.textContent = "Check now";
      }
      body();
    }),
  );
  m.footer.append(recheck, button("Close", m.close, true));
  body();
}
