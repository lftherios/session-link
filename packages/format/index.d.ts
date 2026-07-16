// Public type surface for @session-link/format. Hand-maintained so the
// package ships types without a build step (the runtime is index.mjs).
export * from "./types";
import type { Run } from "./types";

/** Validate a session/v0 document: schema + invariants (unique span ids,
 *  acyclic parent chains). Returns [] when valid, else error strings. */
export function validateRun(data: unknown): string[];

/** The invariants JSON Schema can't express, run alone. */
export function invariantErrors(run: unknown): string[];

/** One-line link-preview title + stats description for a session. */
export function runSummary(run: Run): { title: string; description: string };

/** Credential scan patterns (regex sources), shared by server and CLI. */
export const SECRET_PATTERN_SOURCES: ReadonlyArray<{ name: string; source: string }>;

/** Canonical session/v0 examples, keyed by id. */
export const EXAMPLES: Record<string, Run>;
export const EXAMPLE_IDS: string[];
