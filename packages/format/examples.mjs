import chat from "./examples/chat.json" with { type: "json" };
import agentEval from "./examples/agent-eval.json" with { type: "json" };

/**
 * Canonical session/v0 examples — a plain chat and an agent eval with a
 * score. Stand-in for the hosted lookup; also the homepage gallery data.
 */
export const EXAMPLES = { chat, "agent-eval": agentEval };
export const EXAMPLE_IDS = Object.keys(EXAMPLES);
