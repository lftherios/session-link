// Fictional examples for product screenshots. Rendered by the real local viewer.
// No private transcripts, real research claims, or customer data.
const message = (role, text) => ({ role, content: [{ type: "text", text }] });
const at = (minute, second = 0) => new Date(Date.UTC(2026, 8, 12, 9, minute, second)).toISOString();
const exchange = (id, minute, prompt, response, activity = []) => ({
  id, parent_id: "root", type: "llm_call", model: { id: "sample-model" }, started_at: at(minute), ended_at: at(minute, 42),
  input: { messages: [message("user", prompt)] },
  output: { messages: [...activity, message("assistant", response)] },
});
const session = (name, harness, id, spans) => ({
  schema: "session/v0", name, created_at: at(0),
  source: { kind: "import", harness, fidelity: "reconstructed" },
  metadata: { session_id: id },
  spans: [{ id: "root", type: "agent", name }, ...spans],
});

export const sessions = [
  session("A shorter path to the first useful result", "claude-code", "demo-research", [
    exchange("brief", 0, "Compare the onboarding flows in the Atlas and Beacon sample notes. Where do people get stuck?",
      "Atlas asks people to connect a workspace before they can try it. Beacon starts with a sample project. The main difference is how much setup comes before the first useful result."),
    exchange("evidence", 2, "Separate what the notes show from what we still need to test.",
      "The notes show a difference in setup order. They do not establish which flow converts better. We would need to measure completion and time to the first result."),
    exchange("scope", 4, "What is the smallest change we could try?",
      "Offer a sample project as the first step. Keep connecting a real workspace available, and ask for it when someone is ready to use their own data."),
    exchange("recommendation", 6, "Write a recommendation for the team. Include the comparison and what we should test next.",
      "## Let people try it before they configure it\n\nStart with a sample project that produces a useful result. Ask people to connect their workspace once they know what the product can do.\n\n| First-run experience | Atlas | Beacon |\n| --- | --- | --- |\n| Starting point | Connect a workspace | Open a sample project |\n| Before the first result | Account and integration setup | One guided action |\n| Bring your own data | Required upfront | Available after the preview |\n\n### A small experiment\n\nAdd **Try a sample project** beside the workspace setup. Measure time to the first result and how many people then connect their own data.\n\nThe sample notes support testing this change; they do not yet establish an improvement in conversion.",
      [message("assistant", "I compared the two sample onboarding notes and checked which observations support the recommendation."),
        { role: "assistant", content: [{ type: "tool_call", id: "read-notes", name: "read_file", arguments: { path: "research/onboarding-notes.md" } }] },
        { role: "tool", content: [{ type: "tool_result", tool_call_id: "read-notes", content: [{ type: "text", text: "Atlas: account → workspace integration → first result.\nBeacon: sample project → guided action → optional workspace integration.\nNo conversion or retention measurements are included in these sample notes." }] }] }]),
  ]),
  session("Why failed requests kept retrying", "codex", "demo-debugging", [
    exchange("reproduce", 0, "Find out why the request worker keeps retrying a rejected request.",
      "The worker retries every response outside the success range. That includes validation failures, where repeating the same request cannot help."),
    exchange("change", 2, "Which failures should be retried?",
      "Retry temporary server failures and rate limits, with a fixed attempt limit. Return validation and authentication failures to the caller immediately."),
    exchange("verify", 4, "Check the retry cases and explain the result for the review.",
      "## Retry only when another attempt can help\n\nThe worker now stops on validation and authentication errors. Rate limits and temporary server failures use a bounded retry.\n\n- **400 and 401:** return the error immediately.\n- **429 and 503:** retry up to three times.\n- **Attempt limit reached:** return the last failure.\n\nThe recorded test output covers all five cases, including the retry limit.",
      [{ role: "assistant", content: [{ type: "tool_call", id: "retry-tests", name: "run_command", arguments: { command: "npm test -- retry.test.ts" } }] },
        { role: "tool", content: [{ type: "tool_result", tool_call_id: "retry-tests", content: [{ type: "text", text: "PASS retry.test.ts\n  ✓ returns validation errors without retrying\n  ✓ returns authentication errors without retrying\n  ✓ retries rate-limited requests\n  ✓ retries temporary server failures\n  ✓ stops after three attempts\n\nTests: 5 passed, 5 total" }] }] }]),
  ]),
  session("Make the empty state useful", "pi", "demo-empty-state", [
    exchange("empty-state", 0, "Help someone understand what to do when they open an empty project.",
      "Give the empty state one clear next step: **Create your first project**. Add a short explanation of what happens next and a sample project people can explore."),
    exchange("empty-copy", 2, "Draft the copy and explain the choice.",
      "## Start with something people can do\n\n**Your next idea starts here.**\n\nCreate a project to bring your notes, tasks, and decisions together.\n\nPrimary action: **Create a project**\n\nSecondary action: **Explore a sample**\n\nThe copy describes the next useful action and gives people a way to look around before committing."),
  ]),
];
