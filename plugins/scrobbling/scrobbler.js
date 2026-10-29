// Scrobbling to Last.fm and ListenBrainz, without the UI. main.js wires it to
// Honeycomb; the tests drive it with fake services.
//
// Listens go into a queue first (saved in the plugin's settings) and are sent
// from there, oldest first, so songs heard offline go out once the internet is
// back, in order. A listen leaves the queue only after every service it was
// meant for took it (or refused it for good).

import { md5 } from "./md5.js";

export const LASTFM_API = "https://ws.audioscrobbler.com/2.0/";
export const LISTENBRAINZ_API = "https://api.listenbrainz.org";

const LASTFM_BATCH = 50;
const LB_BATCH = 100;
/** Retry delays after a failed send, in ms: doubled each time, up to the maximum. */
const RETRY_FIRST = 30_000;
const RETRY_MAX = 15 * 60_000;
/** Last.fm ignores scrobbles older than two weeks anyway. */
const MAX_QUEUE = 5000;

const SERVICES = /** @type {const} */ (["lastfm", "listenbrainz"]);
const NAMES = { lastfm: "Last.fm", listenbrainz: "ListenBrainz" };
/** Where each service's login is kept (plugin secrets). */
const CREDENTIAL = { lastfm: "lastfmSession", listenbrainz: "listenbrainzToken" };
const USER = { lastfm: "lastfmUser", listenbrainz: "listenbrainzUser" };

/** Try again later: offline, rate-limited, or the service is down. */
class Retry extends Error {}
/** The session or token stopped working: sign in again. */
class Auth extends Error {}
/** Refused for good: skip these scrobbles. */
class Rejected extends Error {
  constructor(message, code) {
    super(message);
    this.code = code;
  }
}

/** Last.fm's signature: every parameter but `format`, sorted by name, names and values run together, the secret appended, MD5. */
export function sign(params, secret) {
  const s = Object.keys(params)
    .filter((k) => k !== "format")
    .sort()
    .map((k) => k + params[k])
    .join("");
  return md5(s + secret);
}

/** Artist and title to scrobble a track as, or null without them. */
function names(t) {
  const artist = t.artist || t.albumArtist;
  return artist && t.title ? { artist, title: t.title } : null;
}

/**
 * @typedef {{ status: number, ok: boolean, text: string, json(): any }} Response
 * @typedef {{
 *   http(url: string, init?: { method?: string, headers?: Record<string, string>, body?: string | Record<string, string> }): Promise<Response>,
 *   settings: { get(key: string): string | undefined, set(key: string, value: string): Promise<void> },
 *   secrets: { get(key: string): Promise<string | null>, set(key: string, value: string): Promise<void>, delete(key: string): Promise<void> },
 *   builtInKey?: { key: string, secret: string } | null,
 *   version?: string,
 *   lastfmApi?: string,
 *   listenbrainzApi?: string,
 *   now?: () => number,
 *   setTimeout?: typeof setTimeout,
 *   clearTimeout?: typeof clearTimeout,
 *   log?: (...args: any[]) => void,
 * }} Deps
 */

export class Scrobbler {
  /** @param {Deps} deps */
  constructor(deps) {
    this.d = deps;
    this.lastfmApi = deps.lastfmApi ?? LASTFM_API;
    this.lbApi = (deps.listenbrainzApi ?? LISTENBRAINZ_API).replace(/\/$/, "");
    this.now = deps.now ?? (() => Date.now());
    // Wrapped: browsers refuse a bare `setTimeout` called as some other object's method.
    this.setTimer = deps.setTimeout ?? ((fn, ms) => setTimeout(fn, ms));
    this.clearTimer = deps.clearTimeout ?? ((id) => clearTimeout(id));
    this.log = deps.log ?? ((...a) => console.warn("scrobbling:", ...a));
    /** @type {{ id: number, artist: string, title: string, album: string, albumArtist: string, duration: number, trackNo: number | null, playedAt: number, todo: string[] }[]} */
    this.queue = [];
    try {
      const saved = JSON.parse(deps.settings.get("queue") || "[]");
      if (Array.isArray(saved)) this.queue = saved.filter((e) => e && Array.isArray(e.todo) && e.todo.length);
    } catch {
      // A broken queue is dropped rather than blocking scrobbling.
    }
    this.nextId = Math.max(0, ...this.queue.map((e) => e.id || 0)) + 1;
    /** Secrets read once, so the keychain isn't asked on every send. */
    this.cache = new Map();
    this.error = null;
    this.retryDelay = 0;
    this.timer = null;
    this.flushing = null;
    this.stopped = false;
    this.listeners = new Set();
  }

  // ---------- state for the settings screen ----------

  onChange(fn) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  changed() {
    for (const fn of this.listeners) fn();
  }

  async secret(key) {
    if (!this.cache.has(key)) this.cache.set(key, (await this.d.secrets.get(key)) || null);
    return this.cache.get(key);
  }

  async setSecret(key, value) {
    await this.d.secrets.set(key, value);
    this.cache.set(key, value);
  }

  async deleteSecret(key) {
    await this.d.secrets.delete(key);
    this.cache.set(key, null);
  }

  /** The Last.fm API key and secret: the one built in, else one the user pasted. */
  async apiKey() {
    if (this.d.builtInKey?.key && this.d.builtInKey?.secret) return this.d.builtInKey;
    const [key, secret] = [await this.secret("lastfmKey"), await this.secret("lastfmSecret")];
    return key && secret ? { key, secret } : null;
  }

  async status() {
    const user = async (s) => ((await this.secret(CREDENTIAL[s])) ? this.d.settings.get(USER[s]) || "" : null);
    return {
      lastfmKey: this.d.builtInKey?.key ? "built-in" : (await this.apiKey()) ? "own" : null,
      lastfmUser: await user("lastfm"),
      listenbrainzUser: await user("listenbrainz"),
      pending: this.queue.length,
      error: this.error,
    };
  }

  // ---------- the queue ----------

  async save() {
    await this.d.settings.set("queue", JSON.stringify(this.queue));
  }

  /** A song was heard long enough to count. Queued for every signed-in service. */
  async listened(track, playedAt) {
    const n = names(track);
    if (!n) return;
    const todo = [];
    for (const s of SERVICES) if (await this.secret(CREDENTIAL[s])) todo.push(s);
    if (!todo.length) return;
    this.queue.push({
      id: this.nextId++,
      ...n,
      album: track.album || "",
      albumArtist: track.albumArtist || "",
      duration: track.duration || 0,
      trackNo: track.trackNo ?? null,
      playedAt,
      todo,
    });
    if (this.queue.length > MAX_QUEUE) this.queue.splice(0, this.queue.length - MAX_QUEUE);
    await this.save();
    this.changed();
    this.flushSoon(0);
  }

  /** Take `service` off the listed scrobbles, and drop the ones nobody needs any more. */
  async markSent(service, rows) {
    const ids = new Set(rows.map((r) => r.id));
    for (const e of this.queue) if (ids.has(e.id)) e.todo = e.todo.filter((s) => s !== service);
    this.queue = this.queue.filter((e) => e.todo.length);
    await this.save();
  }

  pendingFor(service, limit) {
    return this.queue
      .filter((e) => e.todo.includes(service))
      .sort((a, b) => a.playedAt - b.playedAt || a.id - b.id)
      .slice(0, limit);
  }

  // ---------- sending ----------

  async lastfm(method, params, sk) {
    const k = await this.apiKey();
    if (!k) throw new Auth("No Last.fm API key.");
    const p = { ...params, method, api_key: k.key };
    if (sk) p.sk = sk;
    p.api_sig = sign(p, k.secret);
    p.format = "json";
    let r;
    try {
      r = await this.d.http(this.lastfmApi, { method: "POST", body: p });
    } catch (e) {
      throw new Retry(`Couldn't reach Last.fm: ${e?.message ?? e}`);
    }
    let body;
    try {
      body = r.json();
    } catch {
      throw new Retry(r.ok ? "Last.fm sent an unreadable answer." : `Last.fm answered with HTTP ${r.status}`);
    }
    if (body?.error === undefined) {
      if (!r.ok) throw new Retry(`Last.fm answered with HTTP ${r.status}`);
      return body;
    }
    const code = Number(body.error);
    const msg = `Last.fm: ${body.message || "request failed"} (error ${code})`;
    // Auth failed, invalid session key, invalid or suspended API key.
    if ([4, 9, 10, 26].includes(code)) throw new Auth(msg);
    // Service offline or temporarily unavailable, rate limited.
    if ([8, 11, 16, 29].includes(code)) throw new Retry(msg);
    throw new Rejected(msg, code);
  }

  lastfmScrobble(sk, rows) {
    const p = {};
    rows.forEach((r, i) => {
      p[`artist[${i}]`] = r.artist;
      p[`track[${i}]`] = r.title;
      p[`timestamp[${i}]`] = String(r.playedAt);
      if (r.album) p[`album[${i}]`] = r.album;
      if (r.albumArtist && r.albumArtist !== r.artist) p[`albumArtist[${i}]`] = r.albumArtist;
      if (r.duration > 0) p[`duration[${i}]`] = String(Math.round(r.duration));
      if (r.trackNo != null) p[`trackNumber[${i}]`] = String(r.trackNo);
    });
    // Accepted or ignored (too old, filtered by Last.fm): either way it's done.
    return this.lastfm("track.scrobble", p, sk);
  }

  lbMetadata(r) {
    const info = { submission_client: "Honeycomb", media_player: "Honeycomb" };
    if (this.d.version) info.submission_client_version = this.d.version;
    if (r.duration > 0) info.duration_ms = Math.round(r.duration * 1000);
    if (r.trackNo != null) info.tracknumber = r.trackNo;
    const meta = { artist_name: r.artist, track_name: r.title, additional_info: info };
    if (r.album) meta.release_name = r.album;
    return meta;
  }

  async lbPost(token, body) {
    let r;
    try {
      r = await this.d.http(`${this.lbApi}/1/submit-listens`, {
        method: "POST",
        headers: { Authorization: `Token ${token}`, "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
    } catch (e) {
      throw new Retry(`Couldn't reach ListenBrainz: ${e?.message ?? e}`);
    }
    if (r.ok) return;
    let detail = `HTTP ${r.status}`;
    try {
      detail = r.json().error || detail;
    } catch {}
    const msg = `ListenBrainz: ${detail}`;
    if (r.status === 401) throw new Auth(msg);
    if (r.status === 400) throw new Rejected(msg);
    throw new Retry(msg);
  }

  lbSubmit(token, rows) {
    const payload = rows.map((r) => ({ listened_at: r.playedAt, track_metadata: this.lbMetadata(r) }));
    return this.lbPost(token, { listen_type: rows.length === 1 ? "single" : "import", payload });
  }

  /** Send everything waiting, oldest first. Throws when it should be tried again later. */
  async sendAll() {
    for (const service of SERVICES) {
      for (;;) {
        const cred = await this.secret(CREDENTIAL[service]);
        if (!cred) break;
        const rows = this.pendingFor(service, service === "lastfm" ? LASTFM_BATCH : LB_BATCH);
        if (!rows.length) break;
        try {
          if (service === "lastfm") await this.lastfmScrobble(cred, rows);
          else await this.lbSubmit(cred, rows);
          await this.markSent(service, rows);
        } catch (e) {
          if (e instanceof Rejected) {
            this.log(`${NAMES[service]} refused ${rows.length} scrobbles, skipping them: ${e.message}`);
            await this.markSent(service, rows);
          } else if (e instanceof Auth) {
            // Signed out: keep nothing for it, and say so.
            await this.disconnect(service);
            throw new Error(`${e.message.replace(/\.?$/, ".")} Sign in again in Settings → Plugins.`);
          } else {
            throw e;
          }
        }
      }
    }
  }

  /** Flush after `delay` ms (now by default), unless a flush is already waiting sooner. */
  flushSoon(delay = 0) {
    if (this.stopped) return;
    if (this.timer) this.clearTimer(this.timer);
    this.timer = this.setTimer(() => {
      this.timer = null;
      void this.flush();
    }, delay);
  }

  /** One pass over the queue; schedules a retry with backoff if it failed. */
  async flush() {
    if (this.flushing) return this.flushing;
    this.flushing = (async () => {
      try {
        await this.sendAll();
        this.error = null;
        this.retryDelay = 0;
      } catch (e) {
        this.error = e?.message ?? String(e);
        this.log(this.error);
        this.retryDelay = this.retryDelay ? Math.min(this.retryDelay * 2, RETRY_MAX) : RETRY_FIRST;
        if (this.queue.length) this.flushSoon(this.retryDelay);
      } finally {
        this.flushing = null;
        this.changed();
      }
    })();
    return this.flushing;
  }

  /** A song started playing: tell the services, best effort. */
  async nowPlaying(track) {
    const n = names(track);
    if (!n) return;
    const sk = await this.secret(CREDENTIAL.lastfm);
    if (sk) {
      const p = { artist: n.artist, track: n.title };
      if (track.album) p.album = track.album;
      if (track.duration > 0) p.duration = String(Math.round(track.duration));
      await this.lastfm("track.updateNowPlaying", p, sk).catch((e) => this.log(`now playing: ${e.message}`));
    }
    const token = await this.secret(CREDENTIAL.listenbrainz);
    if (token) {
      const row = { ...n, album: track.album || "", duration: track.duration || 0, trackNo: track.trackNo ?? null };
      const body = { listen_type: "playing_now", payload: [{ track_metadata: this.lbMetadata(row) }] };
      await this.lbPost(token, body).catch((e) => this.log(`now playing: ${e.message}`));
    }
  }

  // ---------- accounts ----------

  /** Start signing in to Last.fm: returns the token and the page where the user approves it. */
  async lastfmBegin() {
    const k = await this.apiKey();
    if (!k) throw new Error("No Last.fm API key.");
    const v = await this.lastfm("auth.getToken", {});
    if (!v.token) throw new Error("Last.fm sent no token.");
    return { token: v.token, url: `https://www.last.fm/api/auth/?api_key=${encodeURIComponent(k.key)}&token=${encodeURIComponent(v.token)}` };
  }

  /** Finish signing in once the user approved `token`. Returns the user name, or null while they haven't yet. */
  async lastfmFinish(token) {
    let v;
    try {
      v = await this.lastfm("auth.getSession", { token });
    } catch (e) {
      // 14: not authorized yet.
      if (e instanceof Rejected && e.code === 14) return null;
      throw e;
    }
    const key = v?.session?.key;
    if (!key) throw new Error("Last.fm sent no session key.");
    const name = v.session.name || "";
    await this.setSecret(CREDENTIAL.lastfm, key);
    await this.d.settings.set(USER.lastfm, name);
    this.changed();
    return name;
  }

  /** Check a ListenBrainz user token and keep it. Returns the user name. */
  async listenbrainzConnect(token) {
    token = token.trim();
    let r;
    try {
      r = await this.d.http(`${this.lbApi}/1/validate-token`, { headers: { Authorization: `Token ${token}` } });
    } catch (e) {
      throw new Error(`Couldn't reach ListenBrainz: ${e?.message ?? e}`);
    }
    let v = null;
    try {
      v = r.json();
    } catch {}
    if (v?.valid !== true) throw new Error("ListenBrainz didn't accept that token. Copy it from listenbrainz.org/settings.");
    const name = v.user_name || "";
    await this.setSecret(CREDENTIAL.listenbrainz, token);
    await this.d.settings.set(USER.listenbrainz, name);
    this.changed();
    return name;
  }

  /** Sign out of a service and drop what was waiting for it. */
  async disconnect(service) {
    await this.deleteSecret(CREDENTIAL[service]);
    await this.d.settings.set(USER[service], "");
    for (const e of this.queue) e.todo = e.todo.filter((s) => s !== service);
    this.queue = this.queue.filter((e) => e.todo.length);
    await this.save();
    this.changed();
  }

  /** Use the user's own Last.fm API account (when the build has none). */
  async setOwnKey(key, secret) {
    await this.setSecret("lastfmKey", key.trim());
    await this.setSecret("lastfmSecret", secret.trim());
    this.changed();
  }

  async forgetOwnKey() {
    await this.disconnect("lastfm");
    await this.deleteSecret("lastfmKey");
    await this.deleteSecret("lastfmSecret");
    this.changed();
  }

  stop() {
    this.stopped = true;
    if (this.timer) this.clearTimer(this.timer);
    this.timer = null;
    this.listeners.clear();
  }
}
