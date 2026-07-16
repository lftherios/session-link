import assert from "node:assert/strict";
import { test } from "node:test";
import { runSummary } from "@session-link/format/run-summary";

test("runSummary: models, span count, cost, errors, date", () => {
  const { title, description } = runSummary({
    name: "math-agent / q117",
    created_at: "2026-07-16T10:00:00Z",
    spans: [
      { id: "s1", type: "llm_call", model: { id: "gpt-4o", provider: "openai" }, usage: { cost_usd: 0.2 } },
      { id: "s2", type: "llm_call", model: { id: "gpt-4o", provider: "openai" }, usage: { cost_usd: 0.11 }, status: "error" },
      { id: "s3", type: "tool_call" },
    ],
  });
  assert.equal(title, "math-agent / q117");
  assert.equal(description, "openai/gpt-4o · 3 spans · $0.3100 · 1 error · Jul 16, 2026");
});

test("runSummary: run-level metrics.cost_usd wins; missing name falls back", () => {
  const { title, description } = runSummary({
    created_at: "2026-01-02T00:00:00Z",
    metrics: { cost_usd: 5 },
    spans: [{ id: "s1", type: "llm_call", model: { id: "claude" } }],
  });
  assert.equal(title, "Shared session");
  assert.match(description, /claude · 1 span · \$5\.00 · Jan 2, 2026/);
});

test("runSummary: caps model list at 3 with +N", () => {
  const spans = ["a", "b", "c", "d", "e"].map((m, i) => ({
    id: `s${i}`,
    type: "llm_call",
    model: { id: m },
  }));
  const { description } = runSummary({ created_at: "2026-07-16T00:00:00Z", spans });
  assert.match(description, /^a, b, c \+2 · 5 spans/);
});

test("runSummary: garbage date and empty run degrade cleanly", () => {
  const { title, description } = runSummary({ created_at: "not-a-date", spans: [] });
  assert.equal(title, "Shared session");
  assert.equal(description, "0 spans"); // no models, no cost, no date
});

test("runSummary: an attacker-controlled name is returned verbatim (escaping is the renderer's job)", () => {
  const { title } = runSummary({
    name: "<script>alert(1)</script>",
    created_at: "2026-07-16T00:00:00Z",
    spans: [],
  });
  // runSummary does not escape — it must not; Next escapes when rendering the
  // <title>/meta. The XSS smoke test asserts the rendered page escapes it.
  assert.equal(title, "<script>alert(1)</script>");
});
