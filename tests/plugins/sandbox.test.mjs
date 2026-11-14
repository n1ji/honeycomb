// Tests for the plugin side of the sandbox (src/plugin-runtime.js), with this
// file standing in for Honeycomb at the other end of the port.
// Run with: npm run test:plugins

import assert from "node:assert/strict";
import { test } from "node:test";
import { createRuntime } from "../../src/plugin-runtime.js";

const MANIFEST = { id: "demo", name: "Demo", version: "1.0.0", main: "main.js" };
const tick = () => new Promise((r) => setTimeout(r, 10));

/** A runtime for `module`, and a fake Honeycomb: `answers[name](...args)` replies to calls. */
function setup(t, module, { answers = {}, settings = {} } = {}) {
  const { port1: host, port2 } = new MessageChannel();
  const seen = { calls: [], ops: [], ready: null };
  host.onmessage = ({ data }) => {
    if (data.ops) seen.ops.push(...data.ops);
    else if (data.ready !== undefined) seen.ready = data;
    else if (data.call) {
      seen.calls.push(data);
      try {
        host.postMessage({ reply: data.id, ok: true, value: answers[data.call]?.(...data.args) });
      } catch (e) {
        host.postMessage({ reply: data.id, ok: false, error: e.message });
      }
    }
  };
  t.after(() => (host.close(), port2.close()));
  const runtime = createRuntime(port2, { manifest: MANIFEST, settings, main: "plugin://main.js" }, async () => (typeof module === "function" ? module() : module));
  return { host, seen, ctx: runtime.ctx, start: () => runtime.start() };
}

/** Every piece of text the plugin put on screen. */
const text = (ops) => ops.filter(([kind]) => kind === "add" || kind === "set").flatMap(([, , kids]) => kids.filter((k) => typeof k === "string"));

test("requests say what's wanted, never who's asking", async (t) => {
  const { ctx, seen } = setup(t, {}, { answers: { "secrets.get": (key) => `value of ${key}` }, settings: { volume: "3" } });
  assert.equal(await ctx.secrets.get("token"), "value of token");
  await ctx.settings.set("volume", "4");
  assert.equal(ctx.settings.get("volume"), "4");
  assert.deepEqual(
    seen.calls.map((c) => [c.call, c.args]),
    [
      ["secrets.get", ["token"]],
      ["settings.set", ["volume", "4"]],
    ],
  );
  assert.ok(seen.calls.every((c) => !JSON.stringify(c).includes("demo")));
});

test("a refused request rejects, and a form body is encoded", async (t) => {
  const { ctx, seen } = setup(t, {}, {
    answers: {
      "http.request": (r) => {
        if (r.url.includes("elsewhere")) throw new Error("elsewhere.example isn't in the hosts plugin.json lists");
        return { status: 201, headers: {}, body: '{"a":1}' };
      },
    },
  });
  const r = await ctx.http.request("https://api.example.com/x", { method: "POST", body: { a: "1 2" } });
  assert.deepEqual([r.ok, r.status, r.json()], [true, 201, { a: 1 }]);
  assert.equal(seen.calls[0].args[0].body, "a=1+2");
  assert.equal(seen.calls[0].args[0].headers["Content-Type"], "application/x-www-form-urlencoded");
  await assert.rejects(ctx.http.request("https://elsewhere.example/"), /isn't in the hosts/);
});

test("a plugin that fails to start says why", async (t) => {
  const missing = setup(t, {});
  await missing.start();
  await tick();
  assert.match(missing.seen.ready.error, /doesn't export an activate/);
  const throwing = setup(t, { activate: () => Promise.reject(new Error("no luck")) });
  await throwing.start();
  await tick();
  assert.deepEqual(throwing.seen.ready, { ready: false, error: "no luck" });
});

test("a settings section is drawn, hears its fields and knows when it's gone", async (t) => {
  let field, typed, root;
  const { host, seen, start } = setup(t, {
    activate(ctx) {
      ctx.ui.addSettingsSection((el) => {
        root = el;
        field = ctx.ui.h("input", { type: "text", value: "before", onchange: (e) => (typed = e.target.value) });
        el.append(ctx.ui.h("p", { class: "muted" }, "Hello ", ctx.ui.h("strong", {}, "there")), field);
      });
    },
  });
  await start();
  await tick();
  assert.deepEqual(seen.ready, { ready: true });
  assert.deepEqual(seen.ops, [["sections", 1]]);

  host.postMessage({ event: "section", index: 0, key: "s1" });
  await tick();
  const made = Object.fromEntries(seen.ops.filter(([kind]) => kind === "new").map(([, id, tag]) => [tag, id]));
  assert.deepEqual(seen.ops.find(([kind]) => kind === "root"), ["root", root.id, "s1"]);
  assert.deepEqual(text(seen.ops), ["there", "Hello "]);
  assert.ok(seen.ops.some(([kind, id, type]) => kind === "on" && id === made.input && type === "change"));
  assert.equal(field.value, "before");
  assert.equal(root.isConnected && field.isConnected, true);

  host.postMessage({ event: "dom", id: made.input, type: "input", value: "after" });
  await tick();
  assert.equal(field.value, "after");
  assert.equal(typed, undefined);
  host.postMessage({ event: "dom", id: made.input, type: "change", value: "after" });
  await tick();
  assert.equal(typed, "after");

  host.postMessage({ event: "gone", ids: [root.id] });
  await tick();
  assert.equal(field.isConnected, false);
});

test("dialogs and sidebar entries", async (t) => {
  let cancelled = 0;
  let clicked = 0;
  const { host, seen, ctx } = setup(t, {});
  const item = ctx.ui.addSidebarItem({ label: "Demo", onClick: () => clicked++ });
  item.setBadge("3");
  const m = ctx.ui.modal("Title", { wide: true, onCancel: () => cancelled++ });
  m.body.append("inside");
  await tick();
  const [, sideId] = seen.ops.find(([kind]) => kind === "sidebar");
  assert.deepEqual(seen.ops.find(([kind]) => kind === "badge"), ["badge", sideId, "3"]);
  assert.deepEqual(seen.ops.find(([kind]) => kind === "modal"), ["modal", m.body.id, m.footer.id, "Title", true]);
  assert.equal(m.body.isConnected, true);

  host.postMessage({ event: "sidebar", id: sideId });
  host.postMessage({ event: "gone", ids: [m.body.id, m.footer.id] });
  await tick();
  assert.deepEqual([clicked, cancelled, m.body.isConnected], [1, 1, false]);
  // Closing a dialog that's already gone doesn't ask Honeycomb again.
  m.close();
  await tick();
  assert.equal(seen.ops.some(([kind]) => kind === "unmodal"), false);
});

test("the scrobbling plugin starts in the sandbox and draws its settings", async (t) => {
  const { host, seen, start } = setup(t, () => import("../../plugins/scrobbling/main.js"), { answers: { "secrets.get": () => null } });
  await start();
  await tick();
  assert.deepEqual(seen.ready, { ready: true });
  host.postMessage({ event: "section", index: 0, key: "s1" });
  await tick();
  const shown = text(seen.ops).join(" | ");
  assert.match(shown, /ListenBrainz/);
  assert.match(shown, /Last\.fm/);
  // Not signed in anywhere, so a listen isn't queued or sent.
  host.postMessage({ event: "listened", track: { id: 1, title: "Song", artist: "Artist", duration: 200 }, playedAt: 1000 });
  await tick();
  assert.deepEqual(seen.calls.filter((c) => c.call === "http.request"), []);
  host.postMessage({ event: "deactivate" });
  await tick();
});
