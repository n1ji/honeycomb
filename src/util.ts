type Attrs = Record<string, string | number | boolean | EventListener | undefined>;

/** Tiny DOM builder: h("div", { class: "x", onclick: fn }, child, "text"). */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs = {},
  ...children: (Node | string | null | undefined | false)[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2), v as EventListener);
    } else if (k === "class") {
      el.className = String(v);
    } else if (v === true) {
      el.setAttribute(k, "");
    } else {
      el.setAttribute(k, String(v));
    }
  }
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    el.append(c);
  }
  return el;
}

export function $(sel: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(sel);
  if (!el) throw new Error(`missing element ${sel}`);
  return el;
}

export function fmtTime(secs: number): string {
  if (!isFinite(secs) || secs < 0) secs = 0;
  const s = Math.floor(secs % 60);
  const m = Math.floor(secs / 60) % 60;
  const hrs = Math.floor(secs / 3600);
  const ss = String(s).padStart(2, "0");
  return hrs ? `${hrs}:${String(m).padStart(2, "0")}:${ss}` : `${m}:${ss}`;
}

export function fmtTotal(secs: number): string {
  const hrs = Math.floor(secs / 3600);
  const m = Math.round((secs % 3600) / 60);
  if (hrs >= 24) return `${(secs / 86400).toFixed(1)} days`;
  return hrs ? `${hrs} h ${m} min` : `${m} min`;
}

export function fmtDate(secs: number | null): string {
  return secs ? new Date(secs * 1000).toLocaleDateString() : "";
}

export function stars(n: number): string {
  return "★".repeat(n) + "☆".repeat(5 - n);
}

let toastTimer = 0;
export function toast(msg: string, isError = false) {
  const el = $("#toast");
  el.textContent = msg;
  el.className = isError ? "show error" : "show";
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (el.className = ""), isError ? 6000 : 3000);
}

export const isMac = navigator.userAgent.includes("Mac");

/** Ctrl on Windows/Linux, Cmd on macOS. */
export function modKey(e: MouseEvent | KeyboardEvent): boolean {
  return isMac ? e.metaKey : e.ctrlKey;
}
