import { readFile } from "node:fs/promises";
import { gzipSync } from "node:zlib";
import { SECRET_PATTERN_SOURCES } from "@session-link/format/secret-patterns";
import { validateRun } from "@session-link/format/validate";
import { assembleSpool, readConfig } from "./store.mjs";

/**
 * The publish core, shared by `slink push` and the Publish button in
 * `slink open`. Mirrors the server's ingest gate client-side, in the
 * same order it re-checks on arrival: validate, then secret scan — before
 * a single byte leaves the machine.
 */

export function scanForSecrets(text) {
  const hits = [];
  for (const { name, source } of SECRET_PATTERN_SOURCES) {
    for (const m of text.matchAll(new RegExp(source, "g"))) {
      hits.push({ pattern: name, preview: `${m[0].slice(0, 8)}…(${m[0].length} chars)` });
      if (hits.length >= 10) return hits;
    }
  }
  return hits;
}

export async function resolveTarget(flags = {}) {
  const config = await readConfig();
  return {
    // Default to the hosted service; local dev overrides via --server,
    // SLINK_SERVER, or `slink login` (keep in sync with the help text in
    // slink.mjs).
    server: flags.server ?? process.env.SLINK_SERVER ?? config.server ?? "https://session.link",
    apiKey: flags.key ?? process.env.SLINK_API_KEY ?? config.api_key,
  };
}

/** Read + validate + scan a run file. Never uploads. */
export async function inspectRunFile(file) {
  // A still-recording session may exist only as a spool (or a stale
  // snapshot) — materialize so `slink push <path>` works mid-session.
  await assembleSpool(file).catch(() => null);
  let text;
  try {
    text = await readFile(file, "utf8");
  } catch {
    return { failure: `cannot read ${file}` };
  }
  let data;
  try {
    data = JSON.parse(text);
  } catch {
    return { failure: `${file} is not valid JSON` };
  }
  const errors = validateRun(data);
  const secrets = errors.length ? [] : scanForSecrets(text);
  return {
    text,
    data,
    bytes: Buffer.byteLength(text),
    models: [
      ...new Set(
        (data.spans ?? [])
          .filter((s) => s.type === "llm_call")
          .map((s) => s.model?.id)
          .filter(Boolean),
      ),
    ],
    errors,
    secrets,
  };
}

/** POST to the ingest API; gzips anything nontrivial (~20:1 on trace JSON). */
export async function uploadRun(text, { server, apiKey }) {
  const headers = { "content-type": "application/json" };
  if (apiKey) headers.authorization = `Bearer ${apiKey}`;
  let body = text;
  if (Buffer.byteLength(text) > 64 * 1024) {
    body = gzipSync(Buffer.from(text));
    headers["content-encoding"] = "gzip";
  }
  let res;
  try {
    res = await fetch(`${server}/api/runs`, { method: "POST", headers, body });
  } catch (e) {
    return { ok: false, status: 0, out: { error: { code: "unreachable", message: `cannot reach ${server}: ${e?.message ?? e}` } } };
  }
  const out = await res.json().catch(() => null);
  return { ok: res.ok, status: res.status, out };
}
