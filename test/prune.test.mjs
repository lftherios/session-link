import assert from "node:assert/strict";
import { test } from "node:test";
import { planPrune } from "../cli/prune.mjs";

const DAY = 86_400_000;
const NOW = Date.parse("2026-07-17T12:00:00.000Z");
// captures are newest-first (as listCaptures returns them)
const cap = (id, { ageDays = 0, spans = 3, in_progress = false } = {}) => ({
  file: `/runs/${id}.json`,
  name: id,
  created_at: new Date(NOW - ageDays * DAY).toISOString(),
  spans,
  in_progress,
});

test("planPrune: drops captures older than the age window, keeps recent", () => {
  const caps = [cap("a", { ageDays: 1 }), cap("b", { ageDays: 20 }), cap("c", { ageDays: 40 }), cap("d", { ageDays: 90 })];
  const { remove, kept } = planPrune(caps, { now: NOW, olderThanMs: 30 * DAY });
  assert.deepEqual(remove.map((c) => c.name), ["c", "d"]);
  assert.deepEqual(kept.map((c) => c.name), ["a", "b"]);
});

test("planPrune: --keep caps the count to the newest N (list is newest-first)", () => {
  const caps = ["a", "b", "c", "d", "e"].map((id, i) => cap(id, { ageDays: i }));
  const { remove } = planPrune(caps, { now: NOW, keep: 2 });
  assert.deepEqual(remove.map((c) => c.name), ["c", "d", "e"]);
});

test("planPrune: --empty drops captures with only the root span", () => {
  const caps = [cap("full", { spans: 5 }), cap("empty", { spans: 1 }), cap("also-empty", { spans: 0 })];
  const { remove } = planPrune(caps, { now: NOW, empty: true });
  assert.deepEqual(remove.map((c) => c.name).sort(), ["also-empty", "empty"]);
});

test("planPrune: an in-progress capture is never pruned, even if old and empty", () => {
  const caps = [cap("live", { ageDays: 999, spans: 1, in_progress: true })];
  const { remove, kept } = planPrune(caps, { now: NOW, olderThanMs: 30 * DAY, empty: true });
  assert.equal(remove.length, 0);
  assert.equal(kept.length, 1);
});

test("planPrune: a capture with an unparseable timestamp is not removed by age", () => {
  const caps = [{ file: "/runs/x.json", name: "x", created_at: "not-a-date", spans: 3, in_progress: false }];
  const { remove } = planPrune(caps, { now: NOW, olderThanMs: 30 * DAY });
  assert.equal(remove.length, 0);
});

test("planPrune: age and keep combine (remove if either cap is exceeded)", () => {
  const caps = [cap("a", { ageDays: 1 }), cap("b", { ageDays: 2 }), cap("c", { ageDays: 40 })];
  const { remove } = planPrune(caps, { now: NOW, olderThanMs: 30 * DAY, keep: 1 });
  // "b" beyond keep=1, "c" also older than window
  assert.deepEqual(remove.map((c) => c.name).sort(), ["b", "c"]);
});

test("planPrune: no policy given removes nothing", () => {
  const caps = [cap("a", { ageDays: 500 }), cap("b", { spans: 1 })];
  const { remove } = planPrune(caps, { now: NOW });
  assert.equal(remove.length, 0);
});
