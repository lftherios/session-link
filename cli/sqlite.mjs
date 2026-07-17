/**
 * Shared read-only SQLite opener for the DB-backed importers (opencode,
 * hermes). node:sqlite arrived in Node 22 (stable in 24); it's imported
 * dynamically so the rest of the CLI still runs on older Node, and so the
 * esbuild bundle keeps the builtin external rather than trying to inline it.
 */
export async function openSqlite(file) {
  let mod;
  try {
    mod = await import("node:sqlite");
  } catch {
    throw new Error("importing this harness needs Node ≥ 22 (built-in node:sqlite)");
  }
  return new mod.DatabaseSync(file, { readOnly: true });
}
