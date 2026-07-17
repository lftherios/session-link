import assert from "node:assert/strict";
import { test } from "node:test";
import { SessionRouter, sessionKeyFrom, labelFrom, AMBIENT_KEY } from "../cli/tap.mjs";

// A router wired to a fake clock + counting session factory, for deterministic tests.
function harness(idleMs = 1000) {
  let clock = 0;
  let n = 0;
  const finalized = [];
  const router = new SessionRouter({
    idleMs,
    now: () => clock,
    newSession: (name) => ({ id: ++n, name, calls: 0 }),
    onFinalize: (s) => finalized.push(s),
  });
  return { router, finalized, tick: (ms) => (clock += ms), at: (ms) => (clock = ms) };
}

test("sessionKeyFrom / labelFrom: header when present, ambient otherwise", () => {
  assert.equal(sessionKeyFrom({ "x-slink-session": "abc" }), "abc");
  assert.equal(sessionKeyFrom({ "x-slink-session": ["abc", "def"] }), "abc");
  assert.equal(sessionKeyFrom({ "x-slink-session": "  " }), AMBIENT_KEY);
  assert.equal(sessionKeyFrom({}), AMBIENT_KEY);
  assert.equal(labelFrom({ "x-slink-label": "eval run" }), "eval run");
  assert.equal(labelFrom({}), undefined);
});

test("route: calls within the idle window share one session", () => {
  const { router, finalized, tick } = harness(1000);
  const a = router.route("ambient", "first");
  a.calls++;
  tick(500);
  const b = router.route("ambient");
  b.calls++;
  assert.equal(a, b, "same session object");
  assert.equal(a.calls, 2);
  assert.equal(finalized.length, 0);
  assert.equal(router.size, 1);
});

test("route: a call after the idle gap rolls over to a new session", () => {
  const { router, finalized, tick } = harness(1000);
  const a = router.route("ambient", "one");
  tick(1001); // just past idle
  const b = router.route("ambient", "two");
  assert.notEqual(a, b);
  assert.equal(finalized.length, 1, "the stale session was finalized");
  assert.equal(finalized[0], a);
  assert.equal(finalized[0].name, "one");
  assert.equal(b.name, "two");
  assert.equal(router.size, 1);
});

test("route: exactly at the idle threshold still reuses (<=, not <)", () => {
  const { router, finalized, tick } = harness(1000);
  const a = router.route("ambient");
  tick(1000);
  const b = router.route("ambient");
  assert.equal(a, b);
  assert.equal(finalized.length, 0);
});

test("route: distinct keys are concurrent, independent sessions", () => {
  const { router, finalized } = harness(1000);
  const a = router.route("sess-A", "A");
  const b = router.route("sess-B", "B");
  assert.notEqual(a, b);
  assert.equal(router.size, 2);
  router.route("sess-A"); // keep A alive, B untouched
  assert.equal(finalized.length, 0);
});

test("sweep: finalizes only sessions idle past the threshold", () => {
  const { router, finalized, at } = harness(1000);
  at(0);
  router.route("old", "old-name"); // last activity at 0
  at(800);
  router.route("fresh", "fresh-name"); // last activity at 800
  at(1200); // old idle = 1200 (> 1000); fresh idle = 400 (<= 1000)
  router.sweep();
  assert.equal(finalized.length, 1, "only the stale one");
  assert.equal(finalized[0].name, "old-name");
  assert.equal(router.size, 1, "fresh remains open");
});

test("finalizeAll: closes everything still open", () => {
  const { router, finalized } = harness(1000);
  router.route("a");
  router.route("b");
  router.route("c");
  assert.equal(router.size, 3);
  router.finalizeAll();
  assert.equal(finalized.length, 3);
  assert.equal(router.size, 0);
});

test("route: reusing a session does not double-finalize", () => {
  const { router, finalized, tick } = harness(1000);
  router.route("k");
  tick(100);
  router.route("k");
  tick(100);
  router.route("k");
  assert.equal(finalized.length, 0);
  router.finalizeAll();
  assert.equal(finalized.length, 1);
});
