import { api, artUrl, Repeat, Track } from "../api";
import { store } from "../store";
import { $, fmtTime, h, stars } from "../util";
import { saveSetting } from "./layout";
import { openArtViewer } from "./artviewer";
import { openPlayerBarCustomizer } from "./customize";
import { icon, setIcon } from "./icons";
import { mountLyrics } from "./lyrics";
import { run } from "./actions";
import { revealPlaying } from "./tracklist";
import { openMenu } from "./menu";
import { mountQualityBadge } from "./quality";

/** Transport buttons, in their default order. `layout.pbButtons` picks which show and where. */
export const PB_BUTTONS = [
  { id: "shuffle", label: "Shuffle" },
  { id: "back", label: "Back 10 seconds" },
  { id: "prev", label: "Previous" },
  { id: "play", label: "Play/Pause" },
  { id: "next", label: "Next" },
  { id: "forward", label: "Forward 10 seconds" },
  { id: "repeat", label: "Repeat" },
];
export const DEFAULT_PB_BUTTONS = ["shuffle", "prev", "play", "next", "repeat"];

/** Parts of the bar that can be hidden (`layout.pbHidden`). */
export const PB_PARTS = [
  { id: "art", label: "Artwork, title and artist" },
  { id: "seek", label: "Seek bar and times" },
  { id: "volume", label: "Volume" },
];

const words = (v: string | undefined) => (v ?? "").split(" ").filter(Boolean);

export function pbButtons(): string[] {
  const known = new Set(PB_BUTTONS.map((b) => b.id));
  const ids = words(store.settings["layout.pbButtons"]).filter((id) => known.has(id));
  if (!ids.length) return DEFAULT_PB_BUTTONS;
  // Play/Pause can't be hidden.
  return ids.includes("play") ? ids : [...ids.slice(0, Math.ceil(ids.length / 2)), "play", ...ids.slice(Math.ceil(ids.length / 2))];
}

export const pbHidden = () => words(store.settings["layout.pbHidden"]);

const REPEAT_NEXT: Record<Repeat, Repeat> = { off: "all", all: "one", one: "off" };
const REPEAT_CMD: Record<Repeat, string> = { off: "repeatOff", all: "repeatAll", one: "repeatOne" };

function artImg(cls: string) {
  const img = h("img", { class: cls, alt: "" });
  img.addEventListener("error", () => img.classList.add("none"));
  img.addEventListener("load", () => img.classList.remove("none"));
  return img;
}

function setArt(img: HTMLImageElement, t: Track | undefined) {
  if (t?.hasArt) {
    const url = artUrl(t.id);
    if (img.dataset.src !== url) {
      img.dataset.src = url;
      img.src = url;
    }
  } else {
    img.dataset.src = "";
    img.removeAttribute("src");
    img.classList.add("none");
  }
}

export function mountPlayerBar() {
  const root = $("#playerbar");
  const art = artImg("pb-art");
  const title = h("div", { class: "pb-title" });
  const artist = h("div", { class: "pb-artist" });
  const playBtn = h("button", { class: "pb-btn play", title: "Play/Pause (Space)", onclick: () => run(() => api.cmd("toggle")) }, icon("play"));
  const shuffleBtn = h("button", { class: "pb-btn small", title: "Shuffle" }, icon("shuffle"));
  const repeatBtn = h("button", { class: "pb-btn small", title: "Repeat" }, icon("repeat"));
  const pos = h("span", { class: "pb-time" }, "0:00");
  const dur = h("span", { class: "pb-time" }, "0:00");
  const seek = h("input", { type: "range", class: "seek", min: 0, max: 1000, value: 0, step: 1 });
  const volume = h("input", { type: "range", class: "volume", min: 0, max: 100, value: 80, title: "Volume" });
  const volIcon = h("button", { class: "pb-btn small", title: "Mute" }, icon("volume"));

  const buttons: Record<string, HTMLElement> = {
    shuffle: shuffleBtn,
    back: h("button", { class: "pb-btn", title: "Back 10 seconds", onclick: () => run(() => api.cmd("seekBy", -10)) }, icon("back")),
    prev: h("button", { class: "pb-btn", title: "Previous", onclick: () => run(() => api.cmd("prev")) }, icon("prev")),
    play: playBtn,
    next: h("button", { class: "pb-btn", title: "Next", onclick: () => run(() => api.cmd("next")) }, icon("next")),
    forward: h("button", { class: "pb-btn", title: "Forward 10 seconds", onclick: () => run(() => api.cmd("seekBy", 10)) }, icon("forward")),
    repeat: repeatBtn,
  };
  const controls = h("div", { class: "pb-controls" });
  const left = h(
    "div",
    { class: "pb-left", title: "Show playing track", onclick: () => revealPlaying() },
    art,
    h("div", { class: "pb-meta" }, h("div", { class: "pb-title-row" }, title, mountQualityBadge()), artist),
  );
  const seekRow = h("div", { class: "pb-seek" }, pos, seek, dur);
  const right = h("div", { class: "pb-right" }, volIcon, volume);
  root.append(left, h("div", { class: "pb-center" }, controls, seekRow), right);

  const renderLayout = () => {
    controls.replaceChildren(...pbButtons().map((id) => buttons[id]));
    const hidden = pbHidden();
    left.classList.toggle("hidden", hidden.includes("art"));
    seekRow.classList.toggle("hidden", hidden.includes("seek"));
    right.classList.toggle("hidden", hidden.includes("volume"));
  };
  root.addEventListener("contextmenu", (e) => {
    e.preventDefault();
    openMenu(e.clientX, e.clientY, [{ label: "Customize player bar…", action: openPlayerBarCustomizer }]);
  });

  shuffleBtn.addEventListener("click", () => run(() => api.cmd("shuffle", store.queue.shuffle ? 0 : 1)));
  repeatBtn.addEventListener("click", () => {
    const next = REPEAT_NEXT[store.queue.repeat];
    store.queue.repeat = next;
    renderQueueState();
    run(() => api.cmd(REPEAT_CMD[next]));
  });

  // Seeking: don't let ticks fight the slider while it's being dragged.
  let dragging = false;
  seek.addEventListener("input", () => {
    dragging = true;
    pos.textContent = fmtTime((Number(seek.value) / 1000) * store.tick.duration);
  });
  seek.addEventListener("change", () => {
    dragging = false;
    run(() => api.cmd("seek", (Number(seek.value) / 1000) * store.tick.duration));
  });

  let lastVolume = 0.8;
  const setVolume = (v: number) => {
    volume.value = String(Math.round(v * 100));
    setIcon(volIcon, v === 0 ? "mute" : v < 0.5 ? "volumeLow" : "volume");
    run(() => api.cmd("volume", v));
  };
  volume.addEventListener("input", () => setVolume(Number(volume.value) / 100));
  volIcon.addEventListener("click", () => {
    const v = Number(volume.value) / 100;
    if (v > 0) {
      lastVolume = v;
      setVolume(0);
    } else setVolume(lastVolume || 0.8);
  });

  const renderQueueState = () => {
    const q = store.queue;
    shuffleBtn.classList.toggle("on", q.shuffle);
    repeatBtn.classList.toggle("on", q.repeat !== "off");
    setIcon(repeatBtn, q.repeat === "one" ? "repeatOne" : "repeat");
    repeatBtn.title = `Repeat: ${q.repeat}`;
    if (!dragging) {
      volume.value = String(Math.round(q.volume * 100));
      setIcon(volIcon, q.volume === 0 ? "mute" : q.volume < 0.5 ? "volumeLow" : "volume");
    }
  };

  let lastId: number | null = -1;
  const renderTick = () => {
    const t = store.tick;
    setIcon(playBtn, t.playing ? "pause" : "play");
    if (!dragging) {
      seek.value = String(t.duration ? Math.round((t.position / t.duration) * 1000) : 0);
      pos.textContent = fmtTime(t.position);
    }
    dur.textContent = fmtTime(t.duration);
    if (t.trackId !== lastId) {
      lastId = t.trackId;
      renderTrack();
    }
  };

  const renderTrack = () => {
    const t = store.nowPlaying;
    title.textContent = t?.title ?? "Not playing";
    artist.textContent = t ? [t.artist, t.album].filter(Boolean).join(" — ") : "";
    setArt(art, t);
    document.title = t ? `${t.title} — ${t.artist} · Honeycomb` : "Honeycomb";
  };

  store.on("tick", renderTick);
  store.on("queue", renderQueueState);
  store.on("tracks", renderTrack);
  store.on("settings", renderLayout);
  renderLayout();
  renderQueueState();
  renderTrack();
}

/** Right-hand panel: big artwork, details and what's up next. */
export function mountNowPlaying() {
  const root = $("#nowplaying");
  const art = artImg("np-art");
  art.title = "Click to enlarge";
  art.addEventListener("click", () => {
    const t = store.nowPlaying;
    if (t?.hasArt) openArtViewer(t);
  });
  const details = h("div", { class: "np-details" });
  const upNext = h("div", { class: "np-next" });
  const lyricsBox = h("div", { class: "np-lyrics" });
  // Lyrics and the up-next list share the space below the details.
  const tabs = h("div", { class: "np-tabs" });
  const tab = (key: string, label: string) =>
    h("button", { type: "button", class: "np-tab", "data-tab": key, onclick: () => run(() => showTab(key, true)) }, label);
  tabs.append(tab("lyrics", "Lyrics"), tab("next", "Next Up"));
  const showTab = async (key: string, save = false) => {
    for (const b of tabs.querySelectorAll<HTMLElement>(".np-tab")) b.classList.toggle("active", b.dataset.tab === key);
    lyricsBox.style.display = key === "lyrics" ? "" : "none";
    upNext.style.display = key === "next" ? "" : "none";
    if (save) await saveSetting("npTab", key);
  };
  root.append(art, details, tabs, lyricsBox, upNext);
  void showTab(store.settings.npTab === "next" ? "next" : "lyrics");
  mountLyrics(lyricsBox);

  const render = () => {
    root.style.display = store.settings.nowPlayingPanel === "off" ? "none" : "";
    const t = store.nowPlaying;
    setArt(art, t);
    if (!t) {
      details.replaceChildren(h("div", { class: "muted" }, "Nothing playing"));
    } else {
      details.replaceChildren(
        h("div", { class: "np-title" }, t.title),
        h("div", { class: "np-artist" }, t.artist),
        h("div", { class: "np-album" }, [t.album, t.year].filter(Boolean).join(" · ")),
        stars(t.rating, "np-rating"),
        h(
          "div",
          { class: "np-tech muted" },
          [t.format, t.bitDepth && `${t.bitDepth}-bit`, t.bitrate && `${t.bitrate} kbps`, t.sampleRate && `${(t.sampleRate / 1000).toFixed(1)} kHz`]
            .filter(Boolean)
            .join(" · "),
        ),
        h("div", { class: "np-tech muted" }, `Played ${t.playCount} time${t.playCount === 1 ? "" : "s"}`),
      );
    }
    const q = store.queue;
    const start = q.current === null ? 0 : q.current + 1;
    const next = q.trackIds.slice(start, start + 12);
    upNext.replaceChildren(
      ...(next.length
        ? next.map((id, i) => {
            const nt = store.tracks.get(id);
            return h(
              "div",
              { class: "np-next-item", title: "Play", onclick: () => run(() => api.cmd("jump", start + i)) },
              h("span", { class: "np-next-title" }, nt?.title ?? "?"),
              h("span", { class: "np-next-artist muted" }, nt?.artist ?? ""),
            );
          })
        : [h("div", { class: "muted" }, "End of queue")]),
    );
  };

  let lastId: number | null = -1;
  store.on("tick", () => {
    if (store.tick.trackId !== lastId) {
      lastId = store.tick.trackId;
      render();
    }
  });
  store.on("queue", render);
  store.on("tracks", render);
  store.on("settings", render);
  render();
}
