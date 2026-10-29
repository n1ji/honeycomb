// The quality badge in the player bar ("FLAC 24/96"), and the signal path
// popover it opens: the file, then every step between it and the speakers.

import { Signal, Track } from "../api";
import { store } from "../store";
import { h } from "../util";

type Source = NonNullable<Signal["source"]>;
type Stage = { state: "ok" | "change" | "info"; title: string; detail: string };

const LOSSLESS_FORMATS = new Set(["FLAC", "WAV", "AIF", "AIFF", "ALAC", "APE", "WV"]);

/** 44100 → "44.1", 96000 → "96". */
const khz = (rate: number) => String(+(rate / 1000).toFixed(1));

const channelName = (n: number) => ({ 1: "Mono", 2: "Stereo", 6: "5.1", 8: "7.1" })[n] ?? `${n} channels`;

/** cpal's sample format names: "f32" → "32-bit float", "i16" → "16-bit". */
function formatName(f: string) {
  const m = /^([fiu])(\d+)$/.exec(f);
  if (!m) return f;
  return m[1] === "f" ? `${m[2]}-bit float` : `${m[2]}-bit`;
}

/** The playing file as the engine probed it, or as the library knows it. */
function source(t: Track): Source {
  const s = store.signal;
  if (s?.source && s.trackId === t.id) return s.source;
  return {
    codec: t.format,
    lossless: LOSSLESS_FORMATS.has(t.format.toUpperCase()),
    bits: t.bitDepth,
    rate: t.sampleRate,
    channels: null,
  };
}

export function badge(t: Track): { text: string; tier: "hires" | "lossless" | "lossy" } {
  const s = source(t);
  if (!s.lossless) return { text: [s.codec, t.bitrate].filter(Boolean).join(" "), tier: "lossy" };
  const bits = s.bits ?? t.bitDepth;
  const rate = s.rate ?? t.sampleRate;
  const text = bits && rate ? `${s.codec} ${bits}/${khz(rate)}` : rate ? `${s.codec} ${khz(rate)} kHz` : s.codec;
  return { text, tier: (bits ?? 0) > 16 || (rate ?? 0) > 48000 ? "hires" : "lossless" };
}

/** Each step from the file to the output, and whether it changes the audio. */
export function stages(t: Track, sig: Signal | null, volume: number): Stage[] {
  const s = source(t);
  const out = sig?.trackId === t.id ? sig.output : null;
  const rate = s.rate ?? t.sampleRate;
  const bits = s.bits ?? t.bitDepth;
  const list: Stage[] = [];

  list.push({
    state: s.lossless ? "ok" : "info",
    title: s.lossless ? "Lossless file" : "Lossy file",
    detail: [
      s.codec,
      bits && `${bits}-bit`,
      !s.lossless && t.bitrate && `${t.bitrate} kbps`,
      rate && `${khz(rate)} kHz`,
      s.channels && channelName(s.channels),
    ]
      .filter(Boolean)
      .join(" · "),
  });

  if (!out) {
    list.push({ state: "info", title: "Output closed", detail: "It opens again when playback starts." });
    return list;
  }

  if (rate && rate !== out.rate) {
    list.push({ state: "change", title: "Resampled", detail: `${khz(rate)} kHz → ${khz(out.rate)} kHz, as the output can't run at the file's rate` });
  } else {
    list.push({ state: "ok", title: "No resampling", detail: `Played at ${khz(out.rate)} kHz, the file's own rate` });
  }

  if (s.channels && s.channels > 2 && out.channels === 2) {
    list.push({ state: "change", title: "Downmixed", detail: `${channelName(s.channels)} folded down to stereo` });
  } else if (s.channels) {
    list.push({ state: "ok", title: "Channels as they are", detail: channelName(s.channels) });
  }

  if (volume >= 1) {
    list.push({ state: "ok", title: "Volume at 100%", detail: "The samples go out as they are" });
  } else {
    list.push({
      state: "change",
      title: `Volume at ${Math.round(volume * 100)}%`,
      detail: "Turned down digitally. Leave it at 100% and use your speakers' volume to keep the samples untouched",
    });
  }

  const outBits = Number(/^i(\d+)$/.exec(out.format)?.[1] ?? 0);
  list.push({
    state: outBits && bits && bits > outBits ? "change" : "ok",
    title: "Output",
    detail: [out.device ?? "Default device", `${khz(out.rate)} kHz`, formatName(out.format), channelName(out.channels)].join(" · ") +
      (outBits && bits && bits > outBits ? `. The device takes ${outBits}-bit, so ${bits}-bit audio is cut down` : ""),
  });
  return list;
}

/** The badge element for the player bar. */
export function mountQualityBadge(): HTMLElement {
  const el = h("button", { type: "button", class: "quality", title: "Playback quality" });
  let pop: HTMLElement | null = null;

  const close = () => {
    pop?.remove();
    pop = null;
    el.classList.remove("open");
    document.removeEventListener("mousedown", outside, true);
    document.removeEventListener("keydown", onKey, true);
  };
  const outside = (e: MouseEvent) => {
    if (pop && !pop.contains(e.target as Node) && !el.contains(e.target as Node)) close();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") close();
  };

  const renderPop = () => {
    const t = store.nowPlaying;
    if (!pop) return;
    if (!t) return close();
    const list = stages(t, store.signal, store.queue.volume);
    const changes = list.filter((s) => s.state === "change").length;
    const lossy = list[0].state === "info";
    const summary =
      changes === 0
        ? lossy
          ? "Nothing changes the audio on its way out, though the file itself is lossy."
          : "Nothing changes the audio on its way out."
        : `${changes} step${changes === 1 ? "" : "s"} change${changes === 1 ? "s" : ""} the audio on its way out.`;
    pop.replaceChildren(
      h("div", { class: "quality-head" }, "Signal path"),
      ...list.map((s) =>
        h(
          "div",
          { class: `quality-stage ${s.state}` },
          h("span", { class: "quality-dot" }),
          h("div", {}, h("div", { class: "quality-title" }, s.title), h("div", { class: "quality-detail muted" }, s.detail)),
        ),
      ),
      h("div", { class: "quality-summary" }, summary),
      h("div", { class: "quality-note muted" }, "Your system's mixer still comes after Honeycomb, so this isn't bit-perfect playback."),
    );
    // Above the badge, kept inside the window.
    const r = el.getBoundingClientRect();
    const w = pop.offsetWidth;
    pop.style.left = `${Math.max(8, Math.min(r.left, innerWidth - w - 8))}px`;
    pop.style.bottom = `${innerHeight - r.top + 8}px`;
  };

  el.addEventListener("click", (e) => {
    e.stopPropagation(); // the bar's left side reveals the playing track
    if (pop) return close();
    pop = h("div", { class: "quality-pop" });
    document.body.append(pop);
    el.classList.add("open");
    document.addEventListener("mousedown", outside, true);
    document.addEventListener("keydown", onKey, true);
    renderPop();
  });

  const render = () => {
    const t = store.nowPlaying;
    el.hidden = !t;
    if (!t) return close();
    const b = badge(t);
    el.textContent = b.text;
    el.className = `quality ${b.tier}${pop ? " open" : ""}`;
    const changed = stages(t, store.signal, 1).some((s) => s.state === "change");
    el.classList.toggle("changed", changed);
    el.title = changed ? "Playback quality: the audio is resampled or changed on its way out" : "Playback quality";
    renderPop();
  };

  let lastId: number | null = -1;
  store.on("tick", () => {
    if (store.tick.trackId !== lastId) {
      lastId = store.tick.trackId;
      render();
    }
  });
  store.on("signal", render);
  store.on("tracks", render);
  store.on("queue", renderPop);
  render();
  return el;
}
