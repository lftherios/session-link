/**
 * Session segmentation for the always-on tap.
 *
 * A persistent recorder sees one continuous stream of provider calls from many
 * agents over time; it has to carve that stream into sessions. The rule:
 *
 *   - a call's session KEY is the `x-slink-session` header when the client
 *     sends one (exact and concurrency-safe — cooperating clients like the pi
 *     extension or `slink dev` set it); otherwise the shared "ambient" bucket.
 *   - a key's session ROLLS OVER after `idleMs` of silence, so a fresh burst of
 *     activity after a gap starts a new session.
 *
 * Ambient (header-less) segmentation is idle-gap only, so genuinely concurrent
 * header-less agents merge into one session — a documented limitation, and the
 * header is the upgrade path to exact boundaries.
 */

export const SESSION_HEADER = "x-slink-session";
export const LABEL_HEADER = "x-slink-label";
export const AMBIENT_KEY = "ambient";
export const DEFAULT_IDLE_MS = 15 * 60 * 1000; // 15 minutes

/** Session key for a request: the client's session header, or "ambient". */
export function sessionKeyFrom(headers = {}) {
  const raw = headers[SESSION_HEADER];
  const v = Array.isArray(raw) ? raw[0] : raw;
  return v && String(v).trim() ? String(v).trim() : AMBIENT_KEY;
}

/** Optional human label a cooperating client can attach to name the session. */
export function labelFrom(headers = {}) {
  const raw = headers[LABEL_HEADER];
  const v = Array.isArray(raw) ? raw[0] : raw;
  return v && String(v).trim() ? String(v).trim() : undefined;
}

export class SessionRouter {
  /**
   * @param {object} opts
   * @param {(name?: string) => any} opts.newSession  create a fresh session object
   * @param {(session: any) => void}  opts.onFinalize  called once when a session ends
   * @param {number} [opts.idleMs]  silence after which a key rolls over
   * @param {() => number} [opts.now]  clock in ms (injectable for tests)
   */
  constructor({ newSession, onFinalize, idleMs = DEFAULT_IDLE_MS, now = () => Date.now() }) {
    this.newSession = newSession;
    this.onFinalize = onFinalize;
    this.idleMs = idleMs;
    this.now = now;
    this.open = new Map(); // key -> { session, lastMs }
  }

  /**
   * The session a call on `key` belongs to. Reuses the open session if it's
   * still fresh; otherwise finalizes the stale one and starts a new session
   * (named `name`, if given).
   */
  route(key, name) {
    const t = this.now();
    const cur = this.open.get(key);
    if (cur && t - cur.lastMs <= this.idleMs) {
      cur.lastMs = t;
      return cur.session;
    }
    if (cur) this.#end(key); // stale — finalize before starting fresh
    const session = this.newSession(name);
    this.open.set(key, { session, lastMs: t });
    return session;
  }

  /** Bump a key's activity time without routing (e.g. a mid-stream heartbeat). */
  touch(key) {
    const cur = this.open.get(key);
    if (cur) cur.lastMs = this.now();
  }

  /** Finalize every session idle past the threshold. Call on a timer. */
  sweep() {
    const t = this.now();
    for (const [key, cur] of this.open) {
      if (t - cur.lastMs > this.idleMs) this.#end(key);
    }
  }

  /** Finalize everything still open (shutdown). */
  finalizeAll() {
    for (const key of [...this.open.keys()]) this.#end(key);
  }

  /** Number of currently-open sessions. */
  get size() {
    return this.open.size;
  }

  #end(key) {
    const cur = this.open.get(key);
    if (!cur) return;
    this.open.delete(key);
    this.onFinalize(cur.session);
  }
}
