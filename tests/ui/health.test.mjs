import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { build } from "rolldown";

// health.ts is TypeScript with app imports; bundle it into one plain module first.
const out = await build({ input: new URL("../../src/health.ts", import.meta.url).pathname, write: false, output: { format: "esm" } });
const file = join(mkdtempSync(join(tmpdir(), "hc-health-")), "health.mjs");
writeFileSync(file, out.output[0].code);
const { checkLibrary } = await import(pathToFileURL(file).href);

let id = 0;
const song = (o) => ({ id: ++id, path: "/m/Al/x.flac", title: `Song ${id}`, artist: "Ann", albumArtist: "", album: "Al", year: 2001, genre: "Pop", trackNo: id, discNo: null, duration: 200 + id * 10, hasArt: true, inbox: false, ...o });

test("a split album is fixed with only the tags that differ", () => {
  const issues = checkLibrary([song({ albumArtist: "Ann" }), song({ albumArtist: "Ann" }), song({ albumArtist: "Ann & Bo" })]);
  const split = issues.filter((i) => i.category === "split");
  assert.equal(split.length, 1);
  assert.deepEqual(split[0].fix, { albumArtist: "Ann" });
  assert.equal(split[0].ids.length, 3);
});

test("an album artist someone set wins over songs without one", () => {
  const issues = checkLibrary([song({}), song({}), song({ albumArtist: "Ann & Bo" })]);
  assert.deepEqual(issues.find((i) => i.category === "split")?.fix, { albumArtist: "Ann & Bo" });
});

test("a compilation without album artists becomes Various Artists", () => {
  const issues = checkLibrary([song({ artist: "A" }), song({ artist: "B" }), song({ artist: "C" })]);
  assert.deepEqual(issues.find((i) => i.category === "split")?.fix, { albumArtist: "Various Artists" });
});
