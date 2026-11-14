// Runs inside each plugin's sandbox frame. It builds the `ctx` the plugin gets
// (see plugin-api.ts) on top of one message port to Honeycomb: requests go out
// as calls, the plugin's interface goes out as a list of changes to elements
// Honeycomb draws, and events come back. src/plugins.ts is the other end.

const PROPS = ["value", "checked", "disabled", "hidden", "title"];

/** Everything a plugin's frame keeps, on `port`. `load(url)` imports the plugin's module. */
export function createRuntime(port, { manifest, settings, main }, load = (url) => import(url)) {
  const calls = new Map();
  let lastCall = 0;
  const call = (name, ...args) =>
    new Promise((resolve, reject) => {
      calls.set(++lastCall, { resolve, reject });
      port.postMessage({ call: name, id: lastCall, args });
    });

  // ---------- elements ----------

  let ops = [];
  const op = (...o) => {
    if (!ops.length) queueMicrotask(() => port.postMessage({ ops: ops.splice(0) }));
    ops.push(o);
  };
  let lastId = 0;
  /** Elements by id, held weakly: one the plugin dropped is forgotten on both sides. */
  const elements = new Map();
  const dropped = typeof FinalizationRegistry === "function" ? new FinalizationRegistry((id) => (elements.delete(id), op("free", id))) : null;

  class Element {
    constructor(tag) {
      this.id = ++lastId;
      this.tagName = tag;
      this.dataset = {};
      this.parent = null;
      this.kids = [];
      this.attrs = {};
      this.props = {};
      this.listeners = new Map();
      /** Roots only: still on screen. */
      this.live = false;
      elements.set(this.id, typeof WeakRef === "function" ? new WeakRef(this) : { deref: () => this });
      dropped?.register(this, this.id);
      op("new", this.id, tag);
    }

    get isConnected() {
      let el = this;
      while (el.parent) el = el.parent;
      return el.live;
    }

    adopt(nodes) {
      const out = [];
      for (const n of nodes.flat()) {
        if (n === null || n === undefined || n === false) continue;
        if (n instanceof Element) {
          if (n.parent) n.parent.kids = n.parent.kids.filter((k) => k !== n);
          n.parent = this;
          out.push(n);
        } else {
          out.push(String(n));
        }
      }
      return out;
    }

    append(...nodes) {
      const added = this.adopt(nodes);
      this.kids.push(...added);
      op("add", this.id, added.map((k) => (k instanceof Element ? k.id : k)));
    }

    replaceChildren(...nodes) {
      for (const k of this.kids) if (k instanceof Element) k.parent = null;
      this.kids = [];
      this.kids = this.adopt(nodes);
      op("set", this.id, this.kids.map((k) => (k instanceof Element ? k.id : k)));
    }

    remove() {
      if (this.parent) this.parent.kids = this.parent.kids.filter((k) => k !== this);
      this.parent = null;
      op("remove", this.id);
    }

    get textContent() {
      return this.kids.map((k) => (k instanceof Element ? k.textContent : k)).join("");
    }

    set textContent(text) {
      this.replaceChildren(String(text ?? ""));
    }

    get className() {
      return this.attrs.class ?? "";
    }

    set className(v) {
      this.setAttribute("class", v);
    }

    getAttribute(name) {
      return this.attrs[name] ?? null;
    }

    setAttribute(name, value) {
      this.attrs[name] = String(value);
      if (name === "value" || name === "title") this.props[name] = String(value);
      if (name === "checked" || name === "disabled" || name === "hidden") this.props[name] = true;
      op("attr", this.id, name, String(value));
    }

    removeAttribute(name) {
      delete this.attrs[name];
      op("attr", this.id, name, null);
    }

    addEventListener(type, fn) {
      if (!this.listeners.has(type)) {
        this.listeners.set(type, new Set());
        op("on", this.id, type);
      }
      this.listeners.get(type).add(fn);
    }

    removeEventListener(type, fn) {
      this.listeners.get(type)?.delete(fn);
    }

    focus() {
      op("do", this.id, "focus");
    }

    select() {
      op("do", this.id, "select");
    }
  }
  for (const name of PROPS) {
    Object.defineProperty(Element.prototype, name, {
      get() {
        return this.props[name] ?? (name === "value" || name === "title" ? "" : false);
      },
      set(v) {
        this.props[name] = v;
        op("prop", this.id, name, v);
      },
    });
  }

  function h(tag, attrs = {}, ...children) {
    const el = new Element(String(tag));
    for (const [k, v] of Object.entries(attrs ?? {})) {
      if (v === undefined || v === false) continue;
      if (k.startsWith("on") && typeof v === "function") el.addEventListener(k.slice(2), v);
      else el.setAttribute(k, v === true ? "" : v);
    }
    el.append(...children);
    return el;
  }

  const button = (label, onclick, primary = false) => h("button", { class: primary ? "btn primary" : "btn", type: "button", onclick }, label);

  /** An element Honeycomb put on screen for the plugin to fill. */
  function root() {
    const el = new Element("div");
    el.live = true;
    roots.set(el.id, el);
    return el;
  }

  const roots = new Map();
  const modals = new Map();
  const sidebar = new Map();
  const sections = [];

  // ---------- events from Honeycomb ----------

  const subscribers = { library: new Set(), nowPlaying: new Set(), listened: new Set() };
  const guarded = (fn, ...args) => {
    try {
      const r = fn(...args);
      if (r && typeof r.catch === "function") r.catch((e) => console.error(`plugin ${manifest.id}:`, e));
    } catch (e) {
      console.error(`plugin ${manifest.id}:`, e);
    }
  };

  let module = null;
  const events = {
    library: () => subscribers.library.forEach((fn) => guarded(fn)),
    nowPlaying: ({ track }) => subscribers.nowPlaying.forEach((fn) => guarded(fn, track)),
    listened: ({ track, playedAt }) => subscribers.listened.forEach((fn) => guarded(fn, track, playedAt)),
    dom: ({ id, type, value, checked, key }) => {
      const el = elements.get(id)?.deref();
      if (!el) return;
      if (value !== undefined) el.props.value = value;
      if (checked !== undefined) el.props.checked = checked;
      const event = { type, key, target: el, currentTarget: el, preventDefault() {}, stopPropagation() {} };
      for (const fn of el.listeners.get(type) ?? []) guarded(fn, event);
    },
    section: ({ index, key }) => {
      const el = root();
      op("root", el.id, key);
      try {
        sections[index]?.(el);
      } catch (e) {
        el.replaceChildren(h("p", { class: "error" }, String(e)));
      }
    },
    gone: ({ ids }) => {
      for (const id of ids) {
        const el = roots.get(id);
        if (el) el.live = false;
        roots.delete(id);
        const onCancel = modals.get(id);
        modals.delete(id);
        if (onCancel) guarded(onCancel);
      }
    },
    sidebar: ({ id }) => {
      const onClick = sidebar.get(id);
      if (onClick) guarded(onClick);
    },
    deactivate: async () => {
      try {
        await module?.deactivate?.();
      } catch (e) {
        console.error(`plugin ${manifest.id}:`, e);
      }
      port.postMessage({ deactivated: true });
    },
  };

  port.onmessage = ({ data }) => {
    if (data.reply !== undefined) {
      const waiting = calls.get(data.reply);
      calls.delete(data.reply);
      if (data.ok) waiting?.resolve(data.value);
      else waiting?.reject(new Error(data.error));
    } else if (typeof data.event === "string" && Object.prototype.hasOwnProperty.call(events, data.event)) {
      events[data.event](data);
    }
  };

  // ---------- the plugin's ctx ----------

  const own = { ...settings };
  const ctx = {
    manifest,
    library: {
      tracks: () => call("library.tracks"),
      track: (id) => call("library.track", id),
      match: (candidates) => call("library.match", candidates),
      onChange: (fn) => void subscribers.library.add(fn),
    },
    playlists: {
      list: () => call("playlists.list"),
      create: (name, trackIds) => call("playlists.create", name, trackIds),
      setTracks: (id, trackIds) => call("playlists.setTracks", id, trackIds),
      trackIds: (id) => call("playlists.trackIds", id),
      show: (id) => call("playlists.show", id),
    },
    player: {
      play: (trackIds, index = 0) => call("player.play", trackIds, index),
      enqueue: (trackIds, next = false) => call("player.enqueue", trackIds, next),
      onNowPlaying: (fn) => void subscribers.nowPlaying.add(fn),
      onListened: (fn) => void subscribers.listened.add(fn),
    },
    http: {
      request: async (url, init = {}) => {
        const headers = { ...(init.headers ?? {}) };
        let body = init.body;
        if (body !== undefined && typeof body !== "string") {
          body = new URLSearchParams(body).toString();
          headers["Content-Type"] ??= "application/x-www-form-urlencoded";
        }
        const r = await call("http.request", { url, method: init.method, headers, body });
        return { status: r.status, ok: r.status >= 200 && r.status < 300, headers: r.headers, text: r.body, json: () => JSON.parse(r.body) };
      },
    },
    settings: {
      get: (key) => own[key],
      set: async (key, value) => {
        own[key] = String(value);
        await call("settings.set", String(key), String(value));
      },
    },
    secrets: {
      get: (key) => call("secrets.get", key),
      set: (key, value) => call("secrets.set", key, value),
      delete: (key) => call("secrets.delete", key),
    },
    openUrl: (url) => call("openUrl", url),
    waitForRedirect: async (p, path, timeoutSecs) => new URLSearchParams(await call("waitForRedirect", p, path, timeoutSecs)),
    ui: {
      addSidebarItem: ({ label, icon = "◆", onClick }) => {
        const id = ++lastId;
        sidebar.set(id, onClick);
        op("sidebar", id, String(label), String(icon));
        return {
          setBadge: (text) => op("badge", id, String(text)),
          remove: () => {
            sidebar.delete(id);
            op("unsidebar", id);
          },
        };
      },
      addSettingsSection: (render) => {
        sections.push(render);
        op("sections", sections.length);
      },
      modal: (title, opts = {}) => {
        const body = root();
        const footer = root();
        op("modal", body.id, footer.id, String(title), !!opts.wide);
        if (opts.onCancel) modals.set(body.id, opts.onCancel);
        const close = () => {
          if (!body.live) return;
          body.live = footer.live = false;
          roots.delete(body.id);
          roots.delete(footer.id);
          modals.delete(body.id);
          op("unmodal", body.id);
        };
        return { body, footer, close };
      },
      button,
      h,
      toast: (message, isError = false) => op("toast", String(message), !!isError),
      prompt: (title, label, initial = "") => call("ui.prompt", String(title), String(label), String(initial)),
      copyText: (text) => call("ui.copyText", String(text)),
    },
  };

  return {
    ctx,
    async start() {
      try {
        module = await load(main);
        if (typeof module.activate !== "function") throw new Error(`${manifest.main} doesn't export an activate() function`);
        await module.activate(ctx);
        port.postMessage({ ready: true });
      } catch (e) {
        port.postMessage({ ready: false, error: String(e?.message ?? e) });
      }
    },
  };
}

if (typeof window !== "undefined") {
  const init = (e) => {
    if (e.source !== window.parent || e.data?.honeycomb !== "init" || !e.ports[0]) return;
    window.removeEventListener("message", init);
    void createRuntime(e.ports[0], e.data).start();
  };
  window.addEventListener("message", init);
}
