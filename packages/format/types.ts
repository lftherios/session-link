/**
 * Run format v0 — canonical, framework-neutral representation of an LLM run.
 *
 * Design principles:
 *  1. Minimal required surface — a valid run is `{ schema, created_at, spans }`.
 *  2. Raw payloads are sacred — normalization may be lossy; `raw` never is.
 *  3. Flat span list — `parent_id` links form the tree; flat lists stream,
 *     append, and virtualize trivially. The viewer builds the tree.
 *  4. Open wire format — session.schema.json accepts unknown span types, part
 *     types, roles, and extra fields; they must survive storage round-trips.
 *     These TS types model the vocabulary this codebase understands, so
 *     runtime values can be wider — renderers must degrade to a JSON
 *     fallback, never drop (see RunViewer). Closed unions are kept only for
 *     controlled vocabulary the viewer's semantics depend on (status,
 *     fidelity).
 *  5. Large payloads go by reference — a `BlobHash` points into the
 *     `attachments` manifest (bytes upload separately). Identical bytes hash
 *     identically, so repeated context deduplicates instead of growing
 *     quadratically across turns.
 */

export const SCHEMA_VERSION = "session/v0" as const;

/** Content address of a payload in the `attachments` manifest. */
export type BlobHash = `sha256:${string}`;

/**
 * How faithfully the capture reflects what crossed the provider boundary.
 * exact = captured at the boundary (wire bytes or in-process SDK hook);
 * reconstructed = assembled after the fact from transcripts or logs;
 * partial = known gaps or truncation. Keep the original capture method
 * (e.g. OpenTraces "transcript_reconstruction") in metadata/extensions.
 */
export type Fidelity = "exact" | "reconstructed" | "partial";

export interface Run {
  schema: typeof SCHEMA_VERSION;
  /** Client-supplied idempotency key. The server assigns the public short id. */
  id?: string;
  name?: string;
  /** When the run happened (not when it was uploaded). ISO 8601. */
  created_at: string;
  source?: RunSource;
  /** Ties runs into a collection (e.g. one eval); `item` names the sample. */
  group?: { id: string; name?: string; item?: string };
  tags?: string[];
  /** Free-form scalars. Viewer conventions: git_sha, branch, framework. */
  metadata?: Record<string, unknown>;
  /**
   * Named, independently versioned envelopes preserved verbatim — analyses
   * and enrichments layered on a run without touching session/v0 itself.
   * Key convention: "<namespace>.<name>.v<N>", e.g. "opentraces.run_intel.v1".
   */
  extensions?: Record<string, unknown>;
  /** Evaluations attached to the whole run. */
  scores?: Score[];
  /** Rollups. Omit to let the server compute from spans; advisory if supplied. */
  metrics?: Metrics;
  spans: Span[];
  /**
   * Manifest of content-addressed payloads referenced from content parts,
   * `*_ref` fields, or `raw_ref`. Bytes upload separately.
   */
  attachments?: Record<string, AttachmentMeta>;
}

export interface RunSource {
  /** Known kinds: sdk, otlp, proxy, import, api. Open on the wire. */
  kind: "sdk" | "otlp" | "proxy" | "import" | "api" | (string & {});
  /** e.g. "inspect", "lm-eval-harness", "local-proxy@0.1.0" */
  label?: string;
  /** Default capture fidelity for the whole run; spans may override. */
  fidelity?: Fidelity;
}

export interface Score {
  name: string;
  value: number | string | boolean;
  /** Known scorers: human, code, model. Open on the wire. */
  scorer?: "human" | "code" | "model" | (string & {});
  comment?: string;
}

export interface Metrics {
  input_tokens?: number;
  output_tokens?: number;
  cost_usd?: number;
  latency_ms?: number;
}

export type Span =
  | LlmCallSpan
  | ToolCallSpan
  | RetrievalSpan
  | AgentSpan
  | CustomSpan;

export interface SpanBase {
  /**
   * Unique within the run. Deep links and comments anchor to `#span=<id>`,
   * so the grammar is URL-fragment-safe (`[A-Za-z0-9._-]{1,64}`) and stable.
   */
  id: string;
  parent_id?: string | null;
  name?: string;
  started_at?: string;
  ended_at?: string;
  status?: "ok" | "error";
  /** Overrides the run-level source.fidelity for this span. */
  fidelity?: Fidelity;
  error?: { message: string; type?: string };
  metadata?: Record<string, unknown>;
  /** Verbatim provider/framework payloads. Never mutated, always preserved. */
  raw?: { request?: unknown; response?: unknown; [key: string]: unknown };
  /**
   * The raw envelope stored as a content-addressed attachment instead of
   * (or alongside) inline — for payloads past the inline budget.
   */
  raw_ref?: BlobHash;
}

export interface LlmCallSpan extends SpanBase {
  type: "llm_call";
  model: { id: string; provider?: string };
  /** Sampling params as sent: temperature, max_tokens, top_p, ... */
  params?: Record<string, unknown>;
  /**
   * At least one of `messages` / `messages_ref` is present. The `*_ref`
   * fields hold context layers as content-addressed attachments (a JSON
   * array/string of the same shape), so repeated conversation prefixes
   * deduplicate instead of growing quadratically.
   */
  input: {
    system?: string;
    system_ref?: BlobHash;
    messages?: Message[];
    messages_ref?: BlobHash;
    tools?: ToolDef[];
    tools_ref?: BlobHash;
  };
  /** Outputs stay inline — they're what people discuss, and grow linearly. */
  output: { messages: Message[] };
  usage?: Usage;
}

export interface Usage {
  input_tokens?: number;
  output_tokens?: number;
  cache_read_tokens?: number;
  cost_usd?: number;
}

export interface ToolCallSpan extends SpanBase {
  type: "tool_call";
  input: {
    name: string;
    arguments?: unknown;
    /** Links to the assistant tool_call content part that requested this. */
    tool_call_id?: string;
  };
  output?: {
    result?: unknown;
    /** Large results (file dumps, page snapshots) stored by content address. */
    result_ref?: BlobHash;
    is_error?: boolean;
  };
}

export interface RetrievalSpan extends SpanBase {
  type: "retrieval";
  input: { query: string };
  output?: { documents: Array<{ text?: string; score?: number; ref?: string }> };
}

export interface AgentSpan extends SpanBase {
  type: "agent";
  input?: unknown;
  output?: unknown;
}

export interface CustomSpan extends SpanBase {
  type: "custom";
  input?: unknown;
  output?: unknown;
}

/** A pragmatic union of the OpenAI and Anthropic message shapes. */
export interface Message {
  /** Known roles: system, user, assistant, tool. Open on the wire. */
  role: "system" | "user" | "assistant" | "tool" | (string & {});
  /** Always an array; normalizers wrap bare strings in a single text part. */
  content: ContentPart[];
  name?: string;
}

export type ContentPart =
  | { type: "text"; text: string }
  | { type: "thinking"; text: string }
  | { type: "image"; attachment?: BlobHash; url?: string; mime?: string }
  | { type: "tool_call"; id: string; name: string; arguments?: unknown }
  | {
      type: "tool_result";
      tool_call_id: string;
      content: ContentPart[];
      is_error?: boolean;
    }
  | { type: "data"; data: unknown }
  /** An oversized payload of any kind stored in the attachments manifest. */
  | { type: "blob"; attachment: BlobHash; mime?: string; name?: string };

export interface ToolDef {
  name: string;
  description?: string;
  input_schema?: unknown;
}

export interface AttachmentMeta {
  mime: string;
  name?: string;
  /** bytes */
  size?: number;
}
