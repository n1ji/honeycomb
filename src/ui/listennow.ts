import { api, artUrl, Track } from "../api";
import { store } from "../store";
import { fmtTime, h, heart, stars } from "../util";
import { run, setLoved, setRating } from "./actions";
import { saveSetting } from "./layout";
import { mountLyrics } from "./lyrics";
import { openMenu } from "./menu";
import { offlineMark } from "./tracklist";

/** How many upcoming songs the Next Up list draws; the rest is a count. */
const NEXT_SHOWN = 50;

export interface NowLinks {
  album: (t: Track) => void;
  artist: (t: Track) => void;
  /** The track whose cover stands for `t`'s album, so list thumbnails share one image. */
  thumb: (t: Track) => number | null;
}

/** The big Now Playing view: cover, song, hearts and stars, with lyrics and Next Up beside it. */
export function mountNowView(root: HTMLElement, go: NowLinks) {
  const bg = h("img", { class: "ln-now-bg", alt: "" });
  const art = h("img", { class: "ln-now-art", alt: "" });
  const artBox = h("div", { class: "ln-now-artbox" }, art);
  const meta = h("div", { class: "ln-now-meta" });
  const lyricsBox = h("div", { class: "np-lyrics ln-lyrics" });
  const queue = h("div", { class: "ln-queue" });
  const tabs = h("div", { class: "np-tabs" });
  const tab = (key: string, label: string) =>
    h("button", { type: "button", class: "np-tab", "data-tab": key, onclick: () => run(() => showTab(key, true)) }, label);
  tabs.append(tab("lyrics", "Lyrics"), tab("next", "Next Up"));
  const showTab = async (key: string, save = false) => {
    for (const b of tabs.querySelectorAll<HTMLElement>(".np-tab")) b.classList.toggle("active", b.dataset.tab === key);
    lyricsBox.style.display = key === "lyrics" ? "" : "none";
    queue.style.display = key === "next" ? "" : "none";
    if (save) await saveSetting("npTab", key);
  };
  const el = h(
    "section",
    { class: "ln-now" },
    bg,
    h("button", { class: "ln-now-close", type: "button", title: "Back to the library (Esc)", onclick: () => close() }, "⌄"),
    h("div", { class: "ln-now-main" }, h("div", { class: "ln-now-stage" }, artBox, meta), h("div", { class: "ln-now-side" }, tabs, lyricsBox, queue)),
  );
  root.append(el);

  let open = false;
  let lyricsMounted = false;

  const setCover = (t: Track | undefined) => {
    const url = t?.hasArt ? artUrl(t.id) : "";
    artBox.classList.toggle("none", !url);
    for (const img of [art, bg]) {
      if (img.dataset.url === url) continue;
      img.dataset.url = url;
      if (url) img.src = url;
      else img.removeAttribute("src");
    }
  };

  const render = () => {
    if (!open) return;
    const t = store.nowPlaying;
    setCover(t);
    if (!t) {
      meta.replaceChildren(h("div", { class: "ln-now-title" }, "Nothing playing"), h("div", { class: "muted" }, "Pick an album and press play."));
    } else {
      const link = (text: string, fn: () => void) =>
        h("span", { class: "ln-link", onclick: () => { close(); fn(); } }, text);
      const rating = stars(t.rating, "np-rating ln-now-stars");
      rating.addEventListener("click", (e) => {
        const star = (e.target as HTMLElement).dataset.star;
        if (!star) return;
        // Server songs keep whole stars; files take halves from a star's left side.
        const box = (e.target as HTMLElement).getBoundingClientRect();
        const half = !store.remote.has(t.id) && (e as MouseEvent).clientX < box.left + box.width / 2;
        const picked = Number(star) * 2 - (half ? 1 : 0);
        run(() => setRating([t.id], picked === t.rating ? 0 : picked));
      });
      const love = heart(t.loved, "np-heart ln-now-heart");
      love.addEventListener("click", () => run(() => setLoved([t.id], !t.loved)));
      meta.replaceChildren(
        h("div", { class: "ln-now-title" }, offlineMark(t.id), t.title),
        h("div", { class: "ln-now-artist" }, link(t.artist || "Unknown artist", () => go.artist(t))),
        h("div", { class: "ln-now-album muted" }, link(t.album || "Unknown album", () => go.album(t)), t.year ? ` · ${t.year}` : ""),
        h("div", { class: "np-marks" }, rating, love),
        h(
          "div",
          { class: "np-tech muted" },
          [t.format, t.bitDepth && `${t.bitDepth}-bit`, t.bitrate && `${t.bitrate} kbps`, t.sampleRate && `${(t.sampleRate / 1000).toFixed(1)} kHz`]
            .filter(Boolean)
            .join(" · "),
        ),
      );
    }

    const q = store.queue;
    const start = q.current === null ? 0 : q.current + 1;
    const rest = q.trackIds.length - start;
    const rows = q.trackIds.slice(start, start + NEXT_SHOWN).map((id, i) => {
      const nt = store.tracks.get(id);
      const thumb = nt ? go.thumb(nt) : null;
      const row = h(
        "div",
        { class: "ln-row", title: "Play", onclick: () => run(() => api.cmd("jump", start + i)) },
        thumb !== null ? h("img", { class: "ln-row-art", alt: "", loading: "lazy", src: artUrl(thumb) }) : h("span", { class: "ln-row-art" }),
        h("div", { class: "ln-row-main" }, h("div", { class: "ln-row-title" }, nt?.title ?? "?"), h("div", { class: "ln-row-sub" }, nt?.artist ?? "")),
        h("span", { class: "ln-row-time" }, nt ? fmtTime(nt.duration) : ""),
      );
      row.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        openMenu(e.clientX, e.clientY, [
          { label: "Play now", action: () => run(() => api.cmd("jump", start + i)) },
          { label: "Remove from Next Up", action: () => run(() => api.removeFromQueue([start + i])) },
        ]);
      });
      return row;
    });
    queue.replaceChildren(
      ...(rows.length ? rows : [h("div", { class: "np-lyrics-msg muted" }, "Nothing else is queued.")]),
      ...(rest > NEXT_SHOWN ? [h("div", { class: "ln-queue-more muted" }, `and ${(rest - NEXT_SHOWN).toLocaleString()} more`)] : []),
    );
  };

  function close() {
    open = false;
    el.classList.remove("open");
  }

  let lastId: number | null = -1;
  store.on("tick", () => {
    if (store.tick.trackId !== lastId) {
      lastId = store.tick.trackId;
      render();
    }
  });
  store.on("queue", render);
  store.on("tracks", render);
  store.on("mode", close);
  void showTab(store.settings.npTab === "next" ? "next" : "lyrics");

  return {
    isOpen: () => open,
    close,
    toggle() {
      if (open) return close();
      open = true;
      // Mounted on first use, so Advanced-only sessions never draw it.
      if (!lyricsMounted) {
        lyricsMounted = true;
        mountLyrics(lyricsBox);
      }
      el.classList.add("open");
      render();
    },
  };
}
