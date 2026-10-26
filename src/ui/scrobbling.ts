import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, ScrobbleStatus } from "../api";
import { h, toast } from "../util";
import { run } from "./actions";
import { button } from "./dialogs";

/** Settings → Scrobbling: sign in to Last.fm and ListenBrainz. */
export function scrobblingSection(): HTMLElement[] {
  const box = h("div", { class: "scrobbling" });
  let signingIn = false;

  const render = (s: ScrobbleStatus) => {
    const lastfm = h("div", { class: "row" });
    if (s.lastfmUser !== null) {
      lastfm.append(
        h("span", {}, `Last.fm: signed in as ${s.lastfmUser || "you"}`),
        button("Sign out", () => run(async () => render(await disconnect("lastfm")))),
      );
    } else if (!s.lastfmAvailable) {
      lastfm.append(h("span", { class: "muted" }, "Last.fm: this build was made without a Last.fm API key."));
    } else if (signingIn) {
      lastfm.append(h("span", {}, "Last.fm: approve Honeycomb in the browser window that opened…"));
    } else {
      lastfm.append(h("span", {}, "Last.fm"), button("Sign in…", () => run(signInLastfm), true));
    }

    const lb = h("div", { class: "row" });
    if (s.listenbrainzUser !== null) {
      lb.append(
        h("span", {}, `ListenBrainz: signed in as ${s.listenbrainzUser || "you"}`),
        button("Sign out", () => run(async () => render(await disconnect("listenbrainz")))),
      );
    } else {
      const token = h("input", { type: "password", class: "input", placeholder: "User token", autocomplete: "off" });
      const connect = () =>
        run(async () => {
          if (!token.value.trim()) return;
          const user = await api.listenbrainzConnect(token.value);
          toast(`ListenBrainz: signed in as ${user}`);
          render(await api.scrobbleStatus());
        });
      token.addEventListener("keydown", (e) => {
        if (e.key === "Enter") connect();
      });
      lb.append(h("span", {}, "ListenBrainz"), token, button("Connect", connect, true));
    }

    const notes: (HTMLElement | false)[] = [
      s.pending > 0 && h("p", { class: "muted" }, `${s.pending} scrobble${s.pending === 1 ? "" : "s"} waiting to be sent.`),
      !!s.error && h("p", { class: "error" }, s.error),
    ];
    box.replaceChildren(lastfm, lb, ...notes.filter((n): n is HTMLElement => !!n));
  };

  const disconnect = async (service: "lastfm" | "listenbrainz") => {
    await api.scrobbleDisconnect(service);
    return api.scrobbleStatus();
  };

  const signInLastfm = async () => {
    const [token, url] = await api.lastfmBegin();
    signingIn = true;
    render(await api.scrobbleStatus());
    await openUrl(url);
    // Wait for the approval in the browser, for up to 5 minutes or until Settings closes.
    const until = Date.now() + 5 * 60_000;
    try {
      while (Date.now() < until && box.isConnected) {
        await new Promise((r) => setTimeout(r, 3000));
        const user = await api.lastfmFinish(token);
        if (user !== null) {
          toast(`Last.fm: signed in as ${user}`);
          return;
        }
      }
    } finally {
      signingIn = false;
      if (box.isConnected) render(await api.scrobbleStatus());
    }
  };

  run(async () => render(await api.scrobbleStatus()));
  const unlisten = listen<ScrobbleStatus>("scrobble-status", (e) => {
    if (box.isConnected) render(e.payload);
  });
  // Stop listening once Settings closes.
  const watch = new MutationObserver(() => {
    if (!box.isConnected && box.dataset.mounted) {
      watch.disconnect();
      void unlisten.then((f) => f());
    }
    if (box.isConnected) box.dataset.mounted = "1";
  });
  watch.observe(document.body, { childList: true, subtree: true });

  return [
    h("h3", {}, "Scrobbling"),
    box,
    h(
      "p",
      { class: "muted" },
      "Songs longer than 30 seconds are scrobbled once you've heard half of them or 4 minutes. Scrobbles made offline are kept and sent later. For ListenBrainz, copy your user token from listenbrainz.org/settings.",
    ),
  ];
}
