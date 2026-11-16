import { TagEdit, Track } from "./api";
import { artistOf } from "./store";

export type Category = "split" | "tracknums" | "covers" | "duplicates" | "metadata" | "partial";

export const CATEGORIES: Record<Category, string> = {
  split: "Split albums",
  tracknums: "Track numbers",
  covers: "Covers",
  duplicates: "Duplicates",
  metadata: "Missing year or genre",
  partial: "Incomplete albums",
};

export interface Issue {
  category: Category;
  title: string;
  artist: string;
  summary: string;
  details: string[];
  ids: number[];
  /** Tags that fix it, written to every song in `ids`. */
  fix?: TagEdit;
}

const VARIOUS = "Various Artists";
const norm = (s: string) => s.toLowerCase().split(/\s+/).filter(Boolean).join(" ");
const folder = (p: string) => p.slice(0, Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\")));
const songs = (n: number) => `${n} song${n === 1 ? "" : "s"}`;
const cap = (items: string[], max = 10) => (items.length > max ? [...items.slice(0, max), `… and ${items.length - max} more`] : items);

/** The most common non-empty value; ties go to the first in sorted order. */
function majority(values: string[]): string {
  const counts = new Map<string, number>();
  for (const v of values) if (v) counts.set(v, (counts.get(v) ?? 0) + 1);
  let best = "";
  let top = 0;
  for (const [v, c] of [...counts].sort(([a], [b]) => (a < b ? -1 : 1))) {
    if (c > top) [best, top] = [v, c];
  }
  return best;
}

function group<K>(tracks: Track[], key: (t: Track) => K | null): Map<K, Track[]> {
  const out = new Map<K, Track[]>();
  for (const t of tracks) {
    const k = key(t);
    if (k === null) continue;
    const list = out.get(k);
    if (list) list.push(t);
    else out.set(k, [t]);
  }
  return out;
}

/**
 * One album's songs shown as several, because their album artist (or the
 * album name's spelling) differs. Songs in one folder with one album name are
 * taken to be one album.
 */
function splitAlbums(tracks: Track[]): Issue[] {
  const issues: Issue[] = [];
  for (const list of group(tracks, (t) => (t.album ? `${folder(t.path)}\u0000${norm(t.album)}` : null)).values()) {
    const parts = group(list, (t) => `${t.album}\u0000${artistOf(t)}`);
    if (parts.size < 2) continue;
    const artists = group(list, (t) => norm(t.artist));
    const topShare = Math.max(...[...artists.values()].map((a) => a.length)) / list.length;
    const various = !list.some((t) => t.albumArtist) && artists.size >= 3 && topShare < 0.5;
    const target = {
      album: majority(list.map((t) => t.album)),
      albumArtist: majority(list.map((t) => t.albumArtist)) || (various ? VARIOUS : majority(list.map((t) => t.artist))),
      year: majority(list.map((t) => (t.year ? String(t.year) : ""))),
    };
    const fix: TagEdit = {};
    if (list.some((t) => t.album !== target.album)) fix.album = target.album;
    if (list.some((t) => t.albumArtist !== target.albumArtist)) fix.albumArtist = target.albumArtist;
    if (target.year && list.some((t) => String(t.year ?? "") !== target.year)) fix.year = target.year;
    issues.push({
      category: "split",
      title: target.album,
      artist: target.albumArtist,
      summary: `Shows as ${parts.size} albums. ${various ? "Looks like a compilation with no album artist." : "The album artist or the album name's spelling differs between songs."}`,
      details: cap([...parts.values()].map((p) => `“${p[0].album}” by ${artistOf(p[0]) || "no artist"} (${songs(p.length)})`)),
      ids: list.map((t) => t.id),
      fix,
    });
  }
  return issues;
}

function trackNumbers(title: string, artist: string, list: Track[]): Issue[] {
  const out: Issue[] = [];
  const ids = list.map((t) => t.id);
  const missing = list.filter((t) => t.trackNo === null);
  const slots = group(list, (t) => (t.trackNo === null ? null : `${t.discNo ?? 1}:${t.trackNo}`));
  const twice = [...slots.values()].filter((g) => g.length > 1);
  const parts: string[] = [];
  const details: string[] = [];
  if (missing.length) {
    parts.push(`${songs(missing.length)} without a track number`);
    details.push("No track number: " + cap(missing.map((t) => t.title)).join("; "));
  }
  if (twice.length) {
    parts.push(`${twice.length} number${twice.length === 1 ? "" : "s"} used twice`);
    for (const g of twice.slice(0, 10)) details.push(`Disc ${g[0].discNo ?? 1}, track ${g[0].trackNo}: ` + g.map((t) => t.title).join(" / "));
    if (list.every((t) => t.discNo === null)) details.push("If this album has several discs, the disc numbers may be missing.");
  }
  if (parts.length) out.push({ category: "tracknums", title, artist, summary: parts.join(", ") + ".", details, ids });

  const discs = new Map<number, Set<number>>();
  for (const t of list) {
    if (t.trackNo === null) continue;
    const d = t.discNo ?? 1;
    if (!discs.has(d)) discs.set(d, new Set());
    discs.get(d)!.add(t.trackNo);
  }
  const gaps: string[] = [];
  let absent = 0;
  for (const [disc, nums] of [...discs].sort(([a], [b]) => a - b)) {
    const top = Math.max(...nums);
    const holes = Array.from({ length: top }, (_, i) => i + 1).filter((n) => !nums.has(n));
    if (!holes.length) continue;
    absent += holes.length;
    gaps.push(`${discs.size > 1 ? `Disc ${disc}` : "Album"}: ${nums.size} of ${top} songs, missing ${cap(holes.map(String), 15).join(", ")}`);
  }
  if (absent) out.push({ category: "partial", title, artist, summary: `Looks incomplete: ${absent} track number${absent === 1 ? "" : "s"} missing.`, details: gaps, ids });
  return out;
}

function duplicates(title: string, artist: string, list: Track[]): Issue[] {
  const found: Track[][] = [];
  for (const same of group(list, (t) => norm(t.title)).values()) {
    if (same.length < 2) continue;
    const sorted = [...same].sort((a, b) => a.duration - b.duration);
    let run = [sorted[0]];
    for (const t of sorted.slice(1)) {
      if (t.duration - run[run.length - 1].duration <= 2) run.push(t);
      else {
        if (run.length > 1) found.push(run);
        run = [t];
      }
    }
    if (run.length > 1) found.push(run);
  }
  if (!found.length) return [];
  return [
    {
      category: "duplicates",
      title,
      artist,
      summary: `${songs(found.length)} more than once, with the same title and length.`,
      details: found.slice(0, 10).flatMap((g) => [`“${g[0].title}” ×${g.length}:`, ...g.map((t) => `    ${t.path}`)]),
      ids: found.flat().map((t) => t.id),
    },
  ];
}

/** Every check over `tracks` (files on this computer), in category order. */
export function checkLibrary(tracks: Track[]): Issue[] {
  const split = splitAlbums(tracks);
  // A split album's parts look incomplete and untagged on their own; fixing the split fixes those.
  const inSplit = new Set(split.flatMap((i) => i.ids));
  const rest: Issue[] = [];
  for (const list of group(tracks, (t) => (inSplit.has(t.id) ? null : `${t.album}\u0000${artistOf(t)}`)).values()) {
    const title = list[0].album || "Unknown album";
    const artist = artistOf(list[0]);
    if (list[0].album) rest.push(...trackNumbers(title, artist, list));
    if (!list.some((t) => t.hasArt)) rest.push({ category: "covers", title, artist, summary: "No cover art.", details: [], ids: list.map((t) => t.id) });
    rest.push(...duplicates(title, artist, list));
    const gaps = [...(list.some((t) => t.year) ? [] : ["year"]), ...(list.some((t) => t.genre) ? [] : ["genre"])];
    if (gaps.length) rest.push({ category: "metadata", title, artist, summary: `No ${gaps.join(" or ")} on any song.`, details: [], ids: list.map((t) => t.id) });
  }
  const order = Object.keys(CATEGORIES);
  rest.sort((a, b) => order.indexOf(a.category) - order.indexOf(b.category) || norm(a.title).localeCompare(norm(b.title)));
  return [...split.sort((a, b) => norm(a.title).localeCompare(norm(b.title))), ...rest];
}
