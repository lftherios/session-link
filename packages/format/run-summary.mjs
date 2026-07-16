/**
 * One-line stats for a run's link preview — the text that makes a shared
 * session.link unfurl as "gpt-4o · 84 spans · $0.31 · 2 errors" instead of a
 * bare title. All values are plain text; the caller renders them into escaped
 * metadata (names are attacker-controlled — never interpolate into markup).
 */
export function runSummary(run) {
  const models = [];
  let cost = 0;
  let sawCost = false;
  let errors = 0;
  for (const s of run.spans ?? []) {
    if (s.status === "error") errors++;
    if (s.type === "llm_call") {
      const label = `${s.model?.provider ? s.model.provider + "/" : ""}${s.model?.id ?? ""}`;
      if (label && !models.includes(label)) models.push(label);
      if (s.usage?.cost_usd != null) {
        sawCost = true;
        cost += s.usage.cost_usd;
      }
    }
  }
  if (run.metrics?.cost_usd != null) {
    sawCost = true;
    cost = run.metrics.cost_usd;
  }

  const n = (run.spans ?? []).length;
  const parts = [
    models.length
      ? models.slice(0, 3).join(", ") + (models.length > 3 ? ` +${models.length - 3}` : "")
      : null,
    `${n} span${n === 1 ? "" : "s"}`,
    sawCost ? `$${cost.toFixed(cost < 1 ? 4 : 2)}` : null,
    errors ? `${errors} error${errors === 1 ? "" : "s"}` : null,
    isoDate(run.created_at),
  ].filter(Boolean);

  return {
    title: (typeof run.name === "string" && run.name.trim()) || "Shared session",
    description: parts.join(" · "),
  };
}

function isoDate(iso) {
  // Format without Date parsing (attacker-controlled, may be any string):
  // accept a leading YYYY-MM-DD, otherwise omit.
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(typeof iso === "string" ? iso : "");
  if (!m) return null;
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const mon = months[Number(m[2]) - 1];
  return mon ? `${mon} ${Number(m[3])}, ${m[1]}` : null;
}
