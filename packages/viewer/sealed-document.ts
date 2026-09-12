import type { Run } from "@session-link/format";
import { decryptDocument, MAX_PLAINTEXT, HEADER_SIZE } from "./encryption";
import validate from "./validate-session.js";

export async function readCiphertext(response: Response): Promise<Uint8Array<ArrayBuffer>> {
 const reader = response.body?.getReader();
 if (!reader) throw new Error("The share could not be downloaded.");
 const chunks: Uint8Array[] = []; let size = 0;
 try {
  while (true) {
   const part = await reader.read(); if (part.done) break;
   size += part.value.length;
   if (size > MAX_PLAINTEXT + HEADER_SIZE + 16) throw new Error("This share exceeds the supported size.");
   chunks.push(part.value);
  }
 } finally { await reader.cancel().catch(() => {}); }
 const bytes = new Uint8Array(size); let offset = 0;
 for (const part of chunks) { bytes.set(part, offset); offset += part.length; }
 return bytes;
}
export async function openDocument(bytes: Uint8Array<ArrayBuffer>, key: string): Promise<Run> {
 const plain = await decryptDocument(bytes, key);
 const doc = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(plain));
 if (!validate(doc)) throw new Error("The decrypted document is not a valid session.");
 const ids = new Map<string, string | undefined>();
 for (const span of doc.spans) { if (ids.has(span.id)) throw new Error("The document contains duplicate steps."); ids.set(span.id, span.parent_id ?? undefined); }
 const checked = new Set<string>();
 for (const id of ids.keys()) {
  const seen = new Set<string>(); let current: string | undefined = id;
  while (current && ids.has(current) && !checked.has(current)) {
   if (seen.has(current)) throw new Error("The document contains a cyclic step tree.");
   seen.add(current); current = ids.get(current);
  }
  for (const item of seen) checked.add(item);
 }
 return doc;
}
