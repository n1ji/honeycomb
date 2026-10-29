// Scrobbling plugin: wires the scrobbler to Honeycomb and draws its section
// in Settings → Plugins.

import { Scrobbler } from "./scrobbler.js";

/** @typedef {import("../../src/plugin-api").PluginContext} PluginContext */

/** @type {Scrobbler | null} */
let scrobbler = null;

/**
 * Release builds carry the app's Last.fm API key in `lastfm-key.js`, written by
 * CI from its secrets (see .github/workflows/build.yml). It's never in the repo;
 * without it, users can paste their own key.
 */
async function builtInKey() {
  try {
    const m = await import("./lastfm-key.js");
    return m.default?.key && m.default?.secret ? m.default : null;
  } catch {
    return null;
  }
}

/** @param {PluginContext} ctx */
export async function activate(ctx) {
  scrobbler = new Scrobbler({
    http: ctx.http.request,
    settings: ctx.settings,
    secrets: ctx.secrets,
    builtInKey: await builtInKey(),
    version: ctx.manifest.version,
  });
  const s = scrobbler;
  ctx.player.onNowPlaying((t) => void s.nowPlaying(t));
  ctx.player.onListened((t, playedAt) => void s.listened(t, playedAt));
  ctx.ui.addSettingsSection((el) => settings(ctx, s, el));
  // Send what's left from last time.
  s.flushSoon(0);
}

export function deactivate() {
  scrobbler?.stop();
  scrobbler = null;
}

/**
 * @param {PluginContext} ctx
 * @param {Scrobbler} s
 * @param {HTMLElement} el
 */
function settings(ctx, s, el) {
  const { h, button, toast } = ctx.ui;
  let signingIn = false;
  const run = (fn) => () => fn().catch((e) => toast(String(e?.message ?? e), true));

  /** The service name column, so both rows line up. */
  const label = (text, cls) => h("span", { class: cls, style: "min-width: 110px" }, text);

  const render = async () => {
    const st = await s.status();
    if (!el.isConnected && el.dataset.drawn) return;

    const lastfm = h("div", { class: "row" });
    if (st.lastfmUser !== null) {
      lastfm.append(
        label(`Last.fm: signed in as ${st.lastfmUser || "you"}`),
        button("Sign out", run(() => s.disconnect("lastfm"))),
      );
    } else if (!st.lastfmKey) {
      lastfm.append(
        label("Last.fm: this build has no Last.fm API key.", "muted"),
        button("Use my own key…", run(ownKey)),
      );
    } else if (signingIn) {
      lastfm.append(label("Last.fm: approve Honeycomb in the browser window that opened…"));
    } else {
      lastfm.append(label("Last.fm"), button("Sign in…", run(signInLastfm), true));
      if (st.lastfmKey === "own") lastfm.append(button("Forget my key", run(() => s.forgetOwnKey())));
    }

    const lb = h("div", { class: "row" });
    if (st.listenbrainzUser !== null) {
      lb.append(
        label(`ListenBrainz: signed in as ${st.listenbrainzUser || "you"}`),
        button("Sign out", run(() => s.disconnect("listenbrainz"))),
      );
    } else {
      const token = h("input", { type: "password", class: "input", placeholder: "User token", autocomplete: "off" });
      const connect = run(async () => {
        if (!token.value.trim()) return;
        toast(`ListenBrainz: signed in as ${await s.listenbrainzConnect(token.value)}`);
      });
      token.addEventListener("keydown", (e) => {
        if (e.key === "Enter") connect();
      });
      lb.append(label("ListenBrainz"), token, button("Connect", connect, true));
    }

    const notes = [];
    if (st.pending > 0) notes.push(h("p", { class: "muted" }, `${st.pending} scrobble${st.pending === 1 ? "" : "s"} waiting to be sent.`));
    if (st.error) notes.push(h("p", { class: "error" }, st.error));
    el.replaceChildren(
      h("div", { style: "display: flex; flex-direction: column; gap: 8px" }, lastfm, lb),
      ...notes,
      h(
        "p",
        { class: "muted" },
        "Songs longer than 30 seconds are scrobbled once you've heard half of them or 4 minutes. Scrobbles made offline are kept and sent later. For ListenBrainz, copy your user token from listenbrainz.org/settings.",
      ),
    );
    el.dataset.drawn = "1";
  };

  const signInLastfm = async () => {
    const { token, url } = await s.lastfmBegin();
    signingIn = true;
    await render();
    await ctx.openUrl(url);
    // Wait for the approval in the browser, for up to 5 minutes or until Settings closes.
    const until = Date.now() + 5 * 60_000;
    try {
      while (Date.now() < until && el.isConnected) {
        await new Promise((r) => setTimeout(r, 3000));
        const user = await s.lastfmFinish(token);
        if (user !== null) {
          toast(`Last.fm: signed in as ${user || "you"}`);
          return;
        }
      }
    } finally {
      signingIn = false;
      await render();
    }
  };

  const ownKey = async () => {
    await ctx.openUrl("https://www.last.fm/api/account/create");
    const key = await ctx.ui.prompt("Your Last.fm API key", "API key (from the page that opened)");
    if (!key?.trim()) return;
    const secret = await ctx.ui.prompt("Your Last.fm API key", "Shared secret");
    if (!secret?.trim()) return;
    await s.setOwnKey(key, secret);
    toast("Last.fm key saved. Sign in to start scrobbling.");
  };

  const stop = s.onChange(() => {
    if (el.isConnected || !el.dataset.drawn) void render();
    else stop();
  });
  void render();
}
