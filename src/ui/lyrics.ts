import { api, Lyrics } from "../api";
import { store } from "../store";
import { h } from "../util";

const SOURCES: Record<Lyrics["source"], string> = {
  embedded: "From the file's tags",
  file: "From a .lrc file",
  lrclib: "From LRCLIB",
  none: "",
};

/** After the user scrolls the lyrics, leave them alone for this long. */
const MANUAL_SCROLL_MS = 4000;

/**
 * Lyrics for the playing track. Synced lyrics highlight the current line and
 * keep it centred; clicking a line seeks there.
 */
export function mountLyrics(box: HTMLElement) {
  let trackId: number | null = null;
  let lyrics: Lyrics | null = null;
  let lineEls: HTMLElement[] = [];
  let current = -1;
  let manualUntil = 0;

  // Only real input counts as manual scrolling; our own smooth scrolls don't.
  const userScrolled = () => (manualUntil = Date.now() + MANUAL_SCROLL_MS);
  box.addEventListener("wheel", userScrolled, { passive: true });
  box.addEventListener("touchmove", userScrolled, { passive: true });
  box.addEventListener("pointerdown", (e) => {
    // Dragging the scrollbar (clicks on lines seek instead).
    if (e.target === box) userScrolled();
  });

  const message = (text: string) => box.replaceChildren(h("div", { class: "np-lyrics-msg muted" }, text));

  const render = (l: Lyrics) => {
    lyrics = l;
    current = -1;
    if (l.instrumental) return message("Instrumental");
    if (!l.lines.length) return message(store.settings.lyricsOnline === "off" ? "No lyrics in this file." : "No lyrics found.");
    lineEls = l.lines.map((line) => {
      const el = h("div", { class: `lyric${line.text ? "" : " gap"}` }, line.text || " ");
      if (l.synced && line.time !== null) {
        el.classList.add("timed");
        el.addEventListener("click", () => api.cmd("seek", line.time!));
      }
      return el;
    });
    box.replaceChildren(...lineEls, h("div", { class: "np-lyrics-src muted" }, SOURCES[l.source]));
    box.classList.toggle("synced", l.synced);
    box.scrollTop = 0;
    follow();
  };

  /** Highlight the line for the current position (synced lyrics only). */
  const follow = () => {
    if (!lyrics?.synced) return;
    const pos = store.tick.position + 0.15; // a touch early reads better
    let i = -1;
    for (let k = 0; k < lyrics.lines.length; k++) {
      const t = lyrics.lines[k].time;
      if (t !== null && t <= pos) i = k;
      else if (t !== null) break;
    }
    if (i === current) return;
    lineEls[current]?.classList.remove("now");
    current = i;
    const el = lineEls[i];
    if (!el) return;
    el.classList.add("now");
    if (Date.now() > manualUntil) {
      // Measure against the box itself; offsetTop depends on which ancestor is positioned.
      const offset = el.getBoundingClientRect().top - box.getBoundingClientRect().top;
      const top = box.scrollTop + offset - box.clientHeight / 2 + el.clientHeight / 2;
      box.scrollTo({ top: Math.max(0, top), behavior: "smooth" });
    }
  };

  const load = async (id: number | null) => {
    trackId = id;
    lyrics = null;
    lineEls = [];
    box.classList.remove("synced");
    if (id === null) return message("Nothing playing");
    message("Looking for lyrics…");
    try {
      const l = await api.lyrics(id);
      if (trackId === id) render(l);
    } catch (e) {
      if (trackId === id) message(`Couldn't load lyrics: ${e}`);
    }
  };

  store.on("tick", () => {
    if (store.tick.trackId !== trackId) void load(store.tick.trackId);
    else follow();
  });
  void load(store.tick.trackId);
}
