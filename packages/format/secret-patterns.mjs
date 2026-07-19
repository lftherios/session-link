/**
 * Credential patterns — the human-edited source. The language-neutral
 * artifact `secret-patterns.json` is GENERATED from this file
 * (`node scripts/golden.mjs --update`) and CI fails on drift; the Go
 * CLI/daemon compiles the JSON's sources with Go's regexp, so every
 * pattern must stay RE2-compatible: no lookahead, no backreferences.
 * The TS server (lib/secrets.ts) and the JS CLI import this module.
 *
 * Exported as regex sources, not RegExp objects: global regexes carry
 * lastIndex state, so each consumer constructs its own instances.
 *
 * Patterns are deliberately high-precision (prefixed token formats) to keep
 * false positives near zero; tune per incident, and prefer adding patterns
 * over loosening existing ones.
 */
export const SECRET_PATTERN_SOURCES = [
  { name: "provider_api_key", source: String.raw`\bsk-[A-Za-z0-9_-]{16,}\b` },
  // Stripe uses underscores (sk_live_…), which the hyphenated pattern above
  // misses; rk_ covers Stripe restricted keys.
  { name: "stripe_key", source: String.raw`\b(?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,}\b` },
  { name: "github_token", source: String.raw`\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{20,}\b` },
  { name: "github_fine_grained_pat", source: String.raw`\bgithub_pat_[A-Za-z0-9_]{20,}\b` },
  { name: "aws_access_key_id", source: String.raw`\bAKIA[0-9A-Z]{16}\b` },
  { name: "slack_token", source: String.raw`\bxox[baprs]-[A-Za-z0-9-]{10,}\b` },
  { name: "google_api_key", source: String.raw`\bAIza[0-9A-Za-z_-]{30,}\b` },
  { name: "private_key_block", source: String.raw`-----BEGIN [A-Z ]*PRIVATE KEY-----` },
];
