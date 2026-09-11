export type ViewUnit = { id: string; span_id: string; role: string; kind: string; text: string; prompt_ids?: string[]; prompt_incomplete?: boolean; unavailable?: boolean };
export type ViewItem = { id: string; start?: number; end?: number };
export type ViewDraft = { title: string; note: string; items: ViewItem[]; primary: string };
export type SavedView = { title: string; url: string };

export const belongsTo = (unit: ViewUnit, prefix: string) => unit.id === prefix || unit.id.startsWith(prefix + "-");
export const unitLabel = (unit: ViewUnit) => ({ tool_call: "Tool arguments", tool_result: "Tool result", thinking: "Recorded reasoning", error: "Recorded error", data: "Recorded data", source_reference: "Source reference", unavailable: "Unavailable content" }[unit.kind] ?? (unit.role === "user" ? "Human input" : unit.role === "assistant" ? "Agent response" : unit.role === "system" ? "Provided context" : unit.role));

export async function viewRequest<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const response = await fetch(path, { method, headers: { "x-slink": "1", "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error?.message ?? "Could not prepare the view. Please try again.");
  return data as T;
}

// Match complete source addresses, never span-wide payloads. Definition cards
// are offered separately when a passage needs them; whole text retains its own.
export const unitsFor = (units: ViewUnit[], prefixes: string[]) => units.filter(unit => !unit.unavailable && unit.kind !== "source_reference" && prefixes.some(prefix => belongsTo(unit, prefix)));
