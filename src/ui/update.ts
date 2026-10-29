import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, UpdateStatus } from "../api";
import { h } from "../util";
import { run } from "./actions";
import { button, modal } from "./dialogs";

const RECHECK_MS = 30 * 60 * 1000;

/**
 * A small bulb in the sidebar footer: dim when this build is current, lit with
 * a count when GitHub has newer builds (on main for builds made from a
 * checkout, in releases for downloaded ones). Click for details.
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
      h(
        "p",
        { class: "muted" },
        s.source === "release"
          ? `This build: ${short(s.local)} · Latest release: ${s.remote || "—"}`
          : `This build: ${short(s.local)} · GitHub main: ${short(s.remote)}`,
      ),
    ];
    if (s.source === "release") {
      if (s.state === "behind" && s.updateUrl) {
        rows.push(h("p", {}, "Install it and Honeycomb restarts on the new version. Your library and settings stay as they are."));
      } else if (s.state === "behind" && s.url) {
        rows.push(h("p", {}, "Download the new version from its release page and install it over this one. Your library and settings stay as they are."));
      }
    } else if (s.state === "behind" || s.state === "diverged") {
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
      render();
    }),
  );
  const download = button("Download", () => run(() => openUrl(s!.url)), true);
  const install = button("Install and restart", () => run(installUpdate), true);
  const installUpdate = async () => {
    const bar = h("progress", { class: "update-progress", max: 1, value: 0 });
    const label = h("p", { class: "muted" }, "Downloading…");
    m.body.replaceChildren(h("p", {}, `Installing ${s!.remote}…`), bar, label);
    m.footer.replaceChildren();
    const stop = await listen<{ done: number; total: number | null }>("update-progress", (e) => {
      const { done, total } = e.payload;
      const mb = (n: number) => (n / 1048576).toFixed(1);
      if (total) {
        bar.value = done / total;
        label.textContent = `Downloading… ${mb(done)} of ${mb(total)} MB`;
      } else {
        bar.removeAttribute("value");
        label.textContent = `Downloading… ${mb(done)} MB`;
      }
      if (total && done >= total) label.textContent = "Installing…";
    });
    try {
      await api.updateInstall();
    } finally {
      stop();
      // Still here: it failed before the restart (the error shows as a toast).
      render();
    }
  };
  const isGithub = (url: string) => url.startsWith("https://github.com/");
  const close = button("Close", m.close);
  const footer = () => {
    const behind = s?.source === "release" && s.state === "behind";
    const canInstall = behind && !!s?.updateUrl;
    const canDownload = behind && !canInstall && isGithub(s!.url);
    close.className = canInstall || canDownload ? "btn" : "btn primary";
    m.footer.replaceChildren(recheck, ...(canInstall ? [install] : canDownload ? [download] : []), close);
  };
  const render = () => {
    body();
    footer();
  };
  render();
}
