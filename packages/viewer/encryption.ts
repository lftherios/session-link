// Interoperable with go/internal/sealed. The key belongs only in a link
// fragment or client key storage, never an HTTP path, query or request body.
export const MAX_PLAINTEXT = 25 * 1024 * 1024;
export const HEADER_SIZE = 24;
const magic = new Uint8Array([83, 76, 73, 78, 75, 69, 0, 1]);

export function encodeKey(key: Uint8Array): string {
  return btoa(String.fromCharCode(...key)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
export function decodeKey(key: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]{43}$/.test(key)) throw new Error("This link has an invalid encryption key.");
  const bytes = Uint8Array.from(atob(key.replace(/-/g, "+").replace(/_/g, "/") + "="), c => c.charCodeAt(0));
  if (encodeKey(bytes) !== key) throw new Error("This link has an invalid encryption key.");
  return bytes;
}
export function validateEnvelope(data: Uint8Array): void {
  if (data.length < HEADER_SIZE + 16 || data.length > HEADER_SIZE + MAX_PLAINTEXT + 16 ||
    !magic.every((v, i) => data[i] === v) || !data.slice(8, 12).every(v => v === 0)) {
    throw new Error("This encrypted document is invalid or uses an unsupported format.");
  }
}
export async function encryptDocument(plain: Uint8Array<ArrayBuffer>): Promise<{ data: Uint8Array<ArrayBuffer>; key: string }> {
  if (plain.length > MAX_PLAINTEXT) throw new Error("This share exceeds the 25 MiB limit.");
  const keyBytes = crypto.getRandomValues(new Uint8Array(32));
  const header = new Uint8Array(HEADER_SIZE); header.set(magic); crypto.getRandomValues(header.subarray(12));
  const key = await crypto.subtle.importKey("raw", keyBytes, "AES-GCM", false, ["encrypt"]);
  const ciphertext = await crypto.subtle.encrypt({ name: "AES-GCM", iv: header.slice(12), additionalData: header, tagLength: 128 }, key, plain);
  const data = new Uint8Array(HEADER_SIZE + ciphertext.byteLength); data.set(header); data.set(new Uint8Array(ciphertext), HEADER_SIZE);
  return { data, key: encodeKey(keyBytes) };
}
export async function decryptDocument(data: Uint8Array<ArrayBuffer>, encodedKey: string): Promise<Uint8Array<ArrayBuffer>> {
  validateEnvelope(data);
  const key = await crypto.subtle.importKey("raw", decodeKey(encodedKey), "AES-GCM", false, ["decrypt"]);
  try {
    return new Uint8Array(await crypto.subtle.decrypt({ name: "AES-GCM", iv: data.slice(12, HEADER_SIZE), additionalData: data.slice(0, HEADER_SIZE), tagLength: 128 }, key, data.slice(HEADER_SIZE)));
  } catch { throw new Error("This document could not be decrypted. The link may be incomplete or the document may have changed."); }
}

// Navigation can change the selected passage without dropping its content key.
export function withAnchor(href: string, span: string): string {
  const url = new URL(href), params = new URLSearchParams(url.hash.slice(1));
  for (const name of ["span", "message", "exchange"]) params.delete(name);
  if (span) params.set("span", span);
  url.hash = params.toString();
  return url.href;
}
