import { h } from "../util";

export interface MenuItem {
  label: string;
  action?: () => void;
  submenu?: MenuItem[];
  disabled?: boolean;
  separator?: boolean;
}

export const SEP: MenuItem = { label: "", separator: true };

let current: HTMLElement | null = null;

export function closeMenu() {
  current?.remove();
  current = null;
}

window.addEventListener("mousedown", (e) => {
  if (current && !current.contains(e.target as Node)) closeMenu();
});
window.addEventListener("blur", closeMenu);
window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeMenu();
});

function build(items: MenuItem[]): HTMLElement {
  const el = h("div", { class: "menu" });
  for (const it of items) {
    if (it.separator) {
      el.append(h("div", { class: "menu-sep" }));
      continue;
    }
    const row = h("div", { class: `menu-item${it.disabled ? " disabled" : ""}${it.submenu ? " has-sub" : ""}` }, it.label);
    if (it.submenu) {
      const sub = build(it.submenu);
      sub.classList.add("submenu");
      row.append(sub);
      row.addEventListener("mouseenter", () => {
        // Flip the submenu left if it would run off-screen.
        const r = row.getBoundingClientRect();
        sub.style.left = r.right + 180 > window.innerWidth ? "auto" : "100%";
        sub.style.right = r.right + 180 > window.innerWidth ? "100%" : "auto";
      });
    } else if (!it.disabled && it.action) {
      const action = it.action;
      row.addEventListener("click", () => {
        closeMenu();
        action();
      });
    }
    el.append(row);
  }
  return el;
}

export function openMenu(x: number, y: number, items: MenuItem[]) {
  closeMenu();
  const el = build(items);
  document.body.append(el);
  const r = el.getBoundingClientRect();
  el.style.left = `${Math.min(x, window.innerWidth - r.width - 4)}px`;
  el.style.top = `${Math.min(y, window.innerHeight - r.height - 4)}px`;
  current = el;
}
