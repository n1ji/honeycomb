// Tests for the scrobbling plugin's logic, against fake Last.fm and
// ListenBrainz services. Run with: npm run test:plugins

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { test } from "node:test";
import { LASTFM_API, LISTENBRAINZ_API, Scrobbler, sign } from "../../plugins/scrobbling/scrobbler.js";

const KEY = { key: "app-key", secret: "app-secret" };
const md5 = (s) => createHash("md5").update(s, "utf8").digest("hex");

function track(id, extra = {}) {
  return { id, title: `Song ${id}`, artist: "Artist", albumArtist: "Artist", album: "Album", duration: 200, trackNo: id, ...extra };
}

const json = (status, body) => ({ status, ok: status >= 200 && status < 300, text: JSON.stringify(body), json: () => body });

/** A Scrobbler wired to fakes. `answer(call)` plays the services; calls are recorded. */
function setup({ answer, secrets = {}, settings = {}, builtInKey = KEY } = {}) {
  const calls = [];
  const timers = [];
  const store = { ...settings };
  const keychain = { ...secrets };
  const s = new Scrobbler({
    http: async (url, init = {}) => {
      const call = { url, method: init.method ?? "GET", headers: init.headers ?? {}, body: init.body };
      calls.push(call);
      return (answer ?? defaultAnswer)(call);
    },
    settings: { get: (k) => store[k], set: async (k, v) => void (store[k] = v) },
    secrets: {
      get: async (k) => keychain[k] ?? null,
      set: async (k, v) => void (keychain[k] = v),
      delete: async (k) => void delete keychain[k],
    },
    builtInKey,
    version: "1.0.0",
    setTimeout: (fn, delay) => (timers.push({ fn, delay }), timers.length),
    clearTimeout: () => {},
    log: () => {},
  });
  return { s, calls, timers, store, keychain };
}

function defaultAnswer(call) {
  if (call.url === LASTFM_API) return json(200, { scrobbles: {} });
  if (call.url.endsWith("/1/submit-listens")) return json(200, { status: "ok" });
  throw new Error(`unexpected ${call.url}`);
}

const BOTH = { lastfmSession: "sk-1", listenbrainzToken: "lb-token" };

test("the signature follows Last.fm's rule", () => {
  const p = { method: "track.scrobble", api_key: "k", sk: "s", "artist[0]": "Björk", format: "json" };
  assert.equal(sign(p, "secret"), md5("api_keykartist[0]Björkmethodtrack.scrobbleskssecret"));
});

test("nothing is queued without an account", async () => {
  const { s, store } = setup();
  await s.listened(track(1), 1000);
  assert.equal(s.queue.length, 0);
  assert.equal(store.queue, undefined);
});

test("tracks without an artist or title are skipped", async () => {
  const { s } = setup({ secrets: BOTH });
  await s.listened(track(1, { artist: "", albumArtist: "" }), 1000);
  await s.listened(track(2, { title: "" }), 1000);
  assert.equal(s.queue.length, 0);
  // The album artist stands in for a missing artist.
  await s.listened(track(3, { artist: "", albumArtist: "Band" }), 1000);
  assert.equal(s.queue[0].artist, "Band");
});

test("a listen goes to both services, signed, with every detail", async () => {
  const { s, calls, store } = setup({ secrets: BOTH });
  await s.listened(track(1, { albumArtist: "Various" }), 1700000000);
  assert.equal(JSON.parse(store.queue).length, 1, "saved before sending");
  await s.flush();

  const fm = calls.find((c) => c.url === LASTFM_API);
  assert.equal(fm.method, "POST");
  assert.deepEqual(
    { ...fm.body, api_sig: undefined },
    {
      "artist[0]": "Artist",
      "track[0]": "Song 1",
      "timestamp[0]": "1700000000",
      "album[0]": "Album",
      "albumArtist[0]": "Various",
      "duration[0]": "200",
      "trackNumber[0]": "1",
      method: "track.scrobble",
      api_key: "app-key",
      sk: "sk-1",
      format: "json",
      api_sig: undefined,
    },
  );
  const { api_sig, ...unsigned } = fm.body;
  assert.equal(api_sig, sign(unsigned, "app-secret"));

  const lb = calls.find((c) => c.url === `${LISTENBRAINZ_API}/1/submit-listens`);
  assert.equal(lb.headers.Authorization, "Token lb-token");
  assert.deepEqual(JSON.parse(lb.body), {
    listen_type: "single",
    payload: [
      {
        listened_at: 1700000000,
        track_metadata: {
          artist_name: "Artist",
          track_name: "Song 1",
          release_name: "Album",
          additional_info: { submission_client: "Honeycomb", media_player: "Honeycomb", submission_client_version: "1.0.0", duration_ms: 200000, tracknumber: 1 },
        },
      },
    ],
  });
  assert.equal(s.queue.length, 0);
  assert.equal(store.queue, "[]");
  assert.equal(s.error, null);
});

test("offline listens wait, retry with backoff, and go out in order", async () => {
  let online = false;
  const { s, calls, timers } = setup({
    secrets: { lastfmSession: "sk" },
    answer: (c) => {
      if (!online) throw new Error("network down");
      return defaultAnswer(c);
    },
  });
  await s.listened(track(2), 2000);
  await s.listened(track(1), 1000);
  await s.flush();
  assert.match(s.error, /Couldn't reach Last\.fm: network down/);
  assert.equal(s.queue.length, 2);
  assert.equal(timers.at(-1).delay, 30_000);
  await s.flush();
  assert.equal(timers.at(-1).delay, 60_000);
  for (let i = 0; i < 10; i++) await s.flush();
  assert.equal(timers.at(-1).delay, 15 * 60_000, "capped at 15 minutes");

  online = true;
  calls.length = 0;
  await s.flush();
  assert.equal(s.error, null);
  assert.equal(s.queue.length, 0);
  assert.equal(calls.length, 1, "both in one request");
  assert.equal(calls[0].body["timestamp[0]"], "1000", "oldest first");
  assert.equal(calls[0].body["timestamp[1]"], "2000");
});

test("big backlogs go in batches of 50 (Last.fm) and 100 (ListenBrainz)", async () => {
  const { s, calls } = setup({ secrets: BOTH });
  s.flushSoon = () => {};
  for (let i = 1; i <= 120; i++) await s.listened(track(i), i);
  await s.flush();
  const fm = calls.filter((c) => c.url === LASTFM_API).map((c) => Object.keys(c.body).filter((k) => k.startsWith("artist[")).length);
  const lb = calls.filter((c) => c.url.endsWith("/submit-listens")).map((c) => JSON.parse(c.body).payload.length);
  assert.deepEqual(fm, [50, 50, 20]);
  assert.deepEqual(lb, [100, 20]);
  assert.equal(JSON.parse(calls.find((c) => c.url.endsWith("/submit-listens")).body).listen_type, "import");
  assert.equal(s.queue.length, 0);
});

test("a dead Last.fm session signs out of Last.fm only", async () => {
  const { s, calls, keychain, store } = setup({
    secrets: BOTH,
    settings: { lastfmUser: "me" },
    answer: (c) => (c.url === LASTFM_API ? json(403, { error: 9, message: "Invalid session key" }) : defaultAnswer(c)),
  });
  await s.listened(track(1), 1000);
  await s.flush();
  assert.match(s.error, /Invalid session key \(error 9\)\. Sign in again/);
  assert.equal(keychain.lastfmSession, undefined);
  assert.equal(store.lastfmUser, "");
  assert.ok(calls.some((c) => c.url.endsWith("/submit-listens")) || s.queue.every((e) => !e.todo.includes("lastfm")));
  // ListenBrainz still gets it on the next pass.
  await s.flush();
  assert.equal(s.queue.length, 0);
  assert.equal((await s.status()).lastfmUser, null);
});

test("scrobbles a service refuses for good are skipped, not retried", async () => {
  const { s, timers } = setup({
    secrets: { listenbrainzToken: "t" },
    answer: () => json(400, { error: "bad listen" }),
  });
  await s.listened(track(1), 1000);
  const before = timers.length;
  await s.flush();
  assert.equal(s.queue.length, 0);
  assert.equal(s.error, null);
  assert.equal(timers.length, before, "no retry scheduled");
});

test("a rate limit is retried later", async () => {
  const { s } = setup({ secrets: { lastfmSession: "sk" }, answer: () => json(200, { error: 29, message: "Rate limit exceeded" }) });
  await s.listened(track(1), 1000);
  await s.flush();
  assert.equal(s.queue.length, 1);
  assert.match(s.error, /Rate limit/);
});

test("the queue survives a restart", async () => {
  const first = setup({ secrets: BOTH, answer: () => { throw new Error("offline"); } });
  await first.s.listened(track(1), 1000);
  await first.s.listened(track(2), 2000);
  const second = setup({ secrets: BOTH, settings: { queue: first.store.queue } });
  assert.equal(second.s.queue.length, 2);
  await second.s.listened(track(3), 3000);
  assert.equal(new Set(second.s.queue.map((e) => e.id)).size, 3, "ids stay unique");
  await second.s.flush();
  assert.equal(second.s.queue.length, 0);
  // A broken saved queue doesn't stop the plugin.
  assert.equal(setup({ settings: { queue: "{nope" } }).s.queue.length, 0);
});

test("now playing goes to both services", async () => {
  const { s, calls } = setup({ secrets: BOTH });
  await s.nowPlaying(track(1));
  assert.equal(calls[0].body.method, "track.updateNowPlaying");
  assert.equal(calls[0].body.artist, "Artist");
  assert.equal(JSON.parse(calls[1].body).listen_type, "playing_now");
  // Failures are only logged.
  const down = setup({ secrets: BOTH, answer: () => { throw new Error("offline"); } });
  await down.s.nowPlaying(track(1));
  assert.equal(down.s.error, null);
});

test("signing in to Last.fm", async () => {
  let approved = false;
  const { s, keychain, store, calls } = setup({
    answer: (c) => {
      if (c.body.method === "auth.getToken") return json(200, { token: "tok" });
      if (c.body.method === "auth.getSession") {
        return approved ? json(200, { session: { key: "new-sk", name: "listener" } }) : json(403, { error: 14, message: "Unauthorized Token" });
      }
    },
  });
  const { token, url } = await s.lastfmBegin();
  assert.equal(token, "tok");
  assert.equal(url, "https://www.last.fm/api/auth/?api_key=app-key&token=tok");
  assert.equal(calls[0].body.sk, undefined);
  assert.equal(await s.lastfmFinish(token), null, "not approved yet");
  approved = true;
  assert.equal(await s.lastfmFinish(token), "listener");
  assert.equal(keychain.lastfmSession, "new-sk");
  assert.equal(store.lastfmUser, "listener");
  assert.equal((await s.status()).lastfmUser, "listener");
});

test("connecting ListenBrainz checks the token first", async () => {
  const { s, keychain, calls } = setup({
    answer: (c) => json(200, c.headers.Authorization === "Token good" ? { valid: true, user_name: "lb-user" } : { valid: false }),
  });
  await assert.rejects(s.listenbrainzConnect("bad"), /didn't accept that token/);
  assert.equal(keychain.listenbrainzToken, undefined);
  assert.equal(await s.listenbrainzConnect("  good \n"), "lb-user");
  assert.equal(keychain.listenbrainzToken, "good");
  assert.equal(calls[0].url, `${LISTENBRAINZ_API}/1/validate-token`);
});

test("signing out drops what was waiting for that service", async () => {
  const { s } = setup({ secrets: BOTH, answer: () => { throw new Error("offline"); } });
  await s.listened(track(1), 1000);
  await s.disconnect("lastfm");
  assert.deepEqual(s.queue[0].todo, ["listenbrainz"]);
  await s.disconnect("listenbrainz");
  assert.equal(s.queue.length, 0);
});

test("without a built-in key, the user's own key is used", async () => {
  const { s, calls } = setup({ builtInKey: null, secrets: { lastfmSession: "sk" } });
  assert.equal((await s.status()).lastfmKey, null);
  await assert.rejects(s.lastfmBegin(), /No Last\.fm API key/);
  await s.setOwnKey(" my-key ", "my-secret");
  assert.equal((await s.status()).lastfmKey, "own");
  await s.listened(track(1), 1000);
  await s.flush();
  const { api_sig, ...unsigned } = calls[0].body;
  assert.equal(unsigned.api_key, "my-key");
  assert.equal(api_sig, sign(unsigned, "my-secret"));
  await s.forgetOwnKey();
  assert.equal((await s.status()).lastfmKey, null);
  assert.equal((await s.status()).lastfmUser, null);
});

test("the built-in key wins over a pasted one", async () => {
  const { s } = setup({ secrets: { lastfmKey: "mine", lastfmSecret: "x" } });
  assert.deepEqual(await s.apiKey(), KEY);
  assert.equal((await s.status()).lastfmKey, "built-in");
});

test("the default timers work the way browsers call them", async () => {
  // WebKit throws when setTimeout is called with `this` set to anything but
  // the window, as happens if it's kept as a method on another object.
  const real = { set: globalThis.setTimeout, clear: globalThis.clearTimeout };
  globalThis.setTimeout = function (fn, ms) {
    if (this !== undefined && this !== globalThis) throw new TypeError("Can only call Window.setTimeout on instances of Window");
    return real.set(fn, ms);
  };
  globalThis.clearTimeout = function (id) {
    if (this !== undefined && this !== globalThis) throw new TypeError("Can only call Window.clearTimeout on instances of Window");
    return real.clear(id);
  };
  try {
    const s = new Scrobbler({
      http: async () => json(200, {}),
      settings: { get: () => undefined, set: async () => {} },
      secrets: { get: async () => null, set: async () => {}, delete: async () => {} },
      log: () => {},
    });
    s.flushSoon(0);
    s.flushSoon(5);
    s.stop();
  } finally {
    globalThis.setTimeout = real.set;
    globalThis.clearTimeout = real.clear;
  }
});
