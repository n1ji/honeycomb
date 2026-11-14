import { toast } from "./util";
import { Modal, modal } from "./ui/dialogs";

/**
 * Draws one plugin's interface. The plugin's frame can't touch the page, so it
 * sends a list of changes to its own elements (plugin-runtime.js) and they're
 * replayed here. Only the tags, attributes and styles below get through, and
 * text only ever goes in as text.
 */
const TAGS = new Set(
  "div span p strong em b i u s small code pre br hr h3 h4 h5 label input textarea select option button progress table thead tbody tr th td ul ol li".split(" "),
);
const ATTRS = new Set(
  "class style title type value placeholder checked disabled readonly selected hidden rows cols min max step maxlength colspan rowspan spellcheck autocomplete role".split(" "),
);
const INPUT_TYPES = new Set(["text", "password", "search", "number", "checkbox", "radio", "range"]);
const EVENTS = new Set(["click", "dblclick", "input", "change", "keydown", "keyup", "focus", "blur"]);
const PROPS = new Set(["value", "checked", "disabled", "hidden", "title"]);
/** Layout and text only: nothing that takes an element out of its box or loads a URL. */
const STYLES =
  /^(display|flex(-[a-z]+)?|gap|(row|column)-gap|align-(items|self|content)|justify-(content|items|self)|grid(-[a-z-]+)?|(min-|max-)?(width|height)|margin(-[a-z]+)?|padding(-[a-z]+)?|text-align|font-(size|weight|style)|line-height|white-space|word-break|overflow(-[xy])?|opacity|color|border-spacing|vertical-align|resize)$/;

export interface SidebarEntry {
  plugin: string;
  label: string;
  icon: string;
  badge: string;
  onClick: () => void;
}

type Op = [string, ...unknown[]];

export class PluginDom {
  private nodes = new Map<number, HTMLElement>();
  /** Elements the app put on screen for the plugin, checked now and then for having gone. */
  private roots = new Map<number, HTMLElement>();
  private waiting = new Map<string, HTMLElement>();
  private modals = new Map<number, { modal: Modal; footer: number }>();
  private heard = new WeakMap<HTMLElement, Set<string>>();
  private sidebar = new Map<number, SidebarEntry>();
  private sweep = 0;
  private lastKey = 0;
  /** How many Settings sections the plugin has. */
  sections = 0;

  constructor(
    private plugin: string,
    private send: (event: string, data: Record<string, unknown>) => void,
    private items: SidebarEntry[],
    private itemsChanged: () => void,
  ) {}

  /** Have the plugin draw its Settings section `index` into `el`. */
  mount(index: number, el: HTMLElement) {
    const key = `s${++this.lastKey}`;
    this.waiting.set(key, el);
    this.send("section", { index, key });
  }

  apply(ops: unknown) {
    if (!Array.isArray(ops)) return;
    for (const op of ops) {
      try {
        if (Array.isArray(op)) this.one(op as Op);
      } catch (e) {
        console.error(`plugin ${this.plugin}:`, e);
      }
    }
  }

  /** Take everything the plugin drew off the screen. */
  dispose() {
    window.clearInterval(this.sweep);
    for (const { modal } of this.modals.values()) modal.close();
    for (const el of this.roots.values()) el.replaceChildren();
    for (const entry of this.sidebar.values()) this.dropItem(entry);
    this.nodes.clear();
    this.roots.clear();
    this.modals.clear();
    this.sidebar.clear();
    this.itemsChanged();
  }

  private one([kind, ...a]: Op) {
    const id = Number(a[0]);
    const el = this.nodes.get(id);
    switch (kind) {
      case "new": {
        const tag = TAGS.has(String(a[1])) ? String(a[1]) : "span";
        const made = document.createElement(tag);
        if (tag === "button") made.setAttribute("type", "button");
        this.nodes.set(id, made);
        // Kept in step so the plugin can read a field's value without asking.
        if (tag === "input" || tag === "textarea" || tag === "select") for (const t of ["input", "change"]) this.listen(id, made, t);
        break;
      }
      case "attr":
        if (el) this.attr(el, String(a[1]).toLowerCase(), a[2] === null ? null : String(a[2]));
        break;
      case "prop":
        if (el && PROPS.has(String(a[1]))) {
          const name = String(a[1]);
          const value = name === "value" || name === "title" ? String(a[2] ?? "") : !!a[2];
          (el as unknown as Record<string, unknown>)[name] = value;
        }
        break;
      case "add":
      case "set": {
        if (!el || !Array.isArray(a[1])) break;
        const kids = a[1].flatMap((k): (Node | string)[] => {
          if (typeof k === "string") return [k];
          const child = this.nodes.get(Number(k));
          return child && !this.roots.has(Number(k)) && !child.contains(el) ? [child] : [];
        });
        if (kind === "set") el.replaceChildren(...kids);
        else el.append(...kids);
        break;
      }
      case "remove":
        if (el && !this.roots.has(id)) el.remove();
        break;
      case "on":
        if (el && EVENTS.has(String(a[1]))) this.listen(id, el, String(a[1]));
        break;
      case "do":
        if (a[1] === "focus") el?.focus();
        else if (a[1] === "select" && el instanceof HTMLInputElement) el.select();
        break;
      case "free":
        if (!this.roots.has(id)) this.nodes.delete(id);
        break;
      case "root": {
        const waiting = this.waiting.get(String(a[1]));
        this.waiting.delete(String(a[1]));
        if (waiting) this.root(id, waiting);
        break;
      }
      case "sections":
        this.sections = Math.min(Number(a[0]) || 0, 20);
        break;
      case "modal": {
        const footer = Number(a[1]);
        const m = modal(String(a[2]), { wide: !!a[3], onCancel: () => this.gone([id, footer]) });
        this.modals.set(id, { modal: m, footer });
        this.root(id, m.body);
        this.root(footer, m.footer);
        break;
      }
      case "unmodal": {
        const open = this.modals.get(id);
        if (!open) break;
        open.modal.close();
        this.forget([id, open.footer]);
        break;
      }
      case "sidebar": {
        const entry: SidebarEntry = { plugin: this.plugin, label: String(a[1]), icon: String(a[2]), badge: "", onClick: () => this.send("sidebar", { id }) };
        this.sidebar.set(id, entry);
        this.items.push(entry);
        this.itemsChanged();
        break;
      }
      case "badge": {
        const entry = this.sidebar.get(id);
        if (entry) entry.badge = String(a[1]);
        this.itemsChanged();
        break;
      }
      case "unsidebar": {
        const entry = this.sidebar.get(id);
        if (entry) this.dropItem(entry);
        this.sidebar.delete(id);
        this.itemsChanged();
        break;
      }
      case "toast":
        toast(String(a[0]), !!a[1]);
        break;
    }
  }

  private attr(el: HTMLElement, name: string, value: string | null) {
    if (!ATTRS.has(name)) return;
    if (value === null) return el.removeAttribute(name);
    if (name === "type") {
      if (el instanceof HTMLInputElement && INPUT_TYPES.has(value)) el.type = value;
    } else if (name === "style") {
      el.removeAttribute("style");
      for (const part of value.split(";")) {
        const [prop, ...rest] = part.split(":");
        const v = rest.join(":").trim();
        if (STYLES.test(prop.trim().toLowerCase()) && !/url\(|expression\(/i.test(v)) el.style.setProperty(prop.trim().toLowerCase(), v);
      }
    } else {
      el.setAttribute(name, value);
      if (name === "value" && "value" in el) (el as HTMLInputElement).value = value;
    }
  }

  private listen(id: number, el: HTMLElement, type: string) {
    const heard = this.heard.get(el) ?? new Set();
    this.heard.set(el, heard);
    if (heard.has(type)) return;
    heard.add(type);
    el.addEventListener(type, (e) => {
      const field = el as HTMLInputElement;
      this.send("dom", {
        id,
        type,
        value: "value" in el ? String(field.value) : undefined,
        checked: el instanceof HTMLInputElement ? el.checked : undefined,
        key: e instanceof KeyboardEvent ? e.key : undefined,
      });
    });
  }

  private root(id: number, el: HTMLElement) {
    this.nodes.set(id, el);
    this.roots.set(id, el);
    // Settings closing isn't announced, so look for roots that left the page.
    if (!this.sweep) {
      this.sweep = window.setInterval(() => {
        this.gone([...this.roots].filter(([, r]) => !r.isConnected).map(([i]) => i));
      }, 1000);
    }
  }

  private gone(ids: number[]) {
    if (!ids.length) return;
    this.forget(ids);
    this.send("gone", { ids });
  }

  private forget(ids: number[]) {
    for (const id of ids) {
      this.nodes.delete(id);
      this.roots.delete(id);
      this.modals.delete(id);
    }
    if (!this.roots.size) {
      window.clearInterval(this.sweep);
      this.sweep = 0;
    }
  }

  private dropItem(entry: SidebarEntry) {
    const i = this.items.indexOf(entry);
    if (i >= 0) this.items.splice(i, 1);
  }
}
