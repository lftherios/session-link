import { b64, unb64, unpack, head, verify, privateKey, unwrap, type Signed } from "./identity-crypto";
import { verifyHistory } from "./identity-protocol";
import type { UnlockedIdentity } from "./browser-identity";

export type Invitation = { id: string; email: string; commitment: string };
type Authored = { account: string; root: string; head: string; signer: string };
// The sender signs one statement per recipient; a recipient receives only its
// own, so the payload never names anyone else.
export type NamedInvite = Authored & { sha256: string; inbox: string; epoch: number; wrap: string; invitation: Invitation };
export type RecipientClaim = Authored & { share_id: string; invite_id: string; sha256: string; owner: string; owner_root: string; inbox: string; epoch: number };
type NamedGrant = Authored & { share_id: string; invite_id: string; sha256: string; recipient: string; recipient_root: string; inbox: string; epoch: number; wrap: string; claim_hash: string };
export type NamedShare = { id: string; owner: boolean; sha256: string; statement: Signed; owner_history: Signed[]; invitation?: { id: string; email: string; status: string }; claim?: Signed & { proof: string }; grant?: Signed; grant_history?: Signed[] };

async function authored<T extends Authored>(kind: string, signed: Signed, history: Signed[]): Promise<T> {
 const value = unpack<T>(signed), checked = await verifyHistory(history, value.account);
 if (checked.state.root !== value.root || checked.head !== value.head) throw new Error("The share's signing identity changed.");
 const device = checked.state.devices.find(d => d.id === value.signer);
 if (!device) throw new Error("The share was signed by an unapproved device.");
 await verify(kind, signed, device.sign);
 return value;
}
export async function verifyInvite(share: NamedShare): Promise<NamedInvite> {
 const p = await authored<NamedInvite>("named-invite", share.statement, share.owner_history);
 const state = (await verifyHistory(share.owner_history, p.account)).state;
 if (p.sha256 !== share.sha256 || !/^[a-f0-9]{64}$/.test(p.sha256) || p.inbox !== state.inbox || p.epoch !== state.epoch) throw new Error("Invalid private-sharing invitation.");
 const r = p.invitation;
 if (!r || typeof r !== "object" || typeof r.email !== "string" || !r.email) throw new Error("Invalid invitation.");
 unb64(r.id, 24); unb64(r.commitment, 32);
 // Consistency with the metadata the server returned alongside it; the secret
 // commitment below and the recipient's own signed claim are the real bindings.
 if (!share.owner && share.invitation && (r.id !== share.invitation.id || r.email !== share.invitation.email)) throw new Error("The invitation does not match this share.");
 return p;
}
export async function verifyClaim(share: NamedShare, p: NamedInvite, identity: UnlockedIdentity): Promise<RecipientClaim> {
 if (!share.claim) throw new Error("Accept the invitation first.");
 const value = unpack<RecipientClaim>(share.claim);
 const history: Signed[] = [];
 for (const event of identity.record.events) { history.push(event); if (await head(event) === value.head) break; }
 const claim = await authored<RecipientClaim>("recipient-claim", share.claim, history);
 if (claim.account !== identity.account || claim.root !== identity.state.root || claim.share_id !== share.id || claim.invite_id !== share.invitation?.id || claim.sha256 !== p.sha256 || claim.owner !== p.account || claim.owner_root !== p.root || p.invitation.id !== claim.invite_id) throw new Error("The invitation does not match your approved identity.");
 const then = (await verifyHistory(history, identity.account)).state;
 if (claim.inbox !== then.inbox || claim.epoch !== then.epoch) throw new Error("The invitation's encryption identity changed.");
 return claim;
}
const context = (sha: string, account: string, root: string, epoch: number) => `named/${sha}/${account}/${root}/${epoch}`;
export async function namedContentKey(share: NamedShare, identity: UnlockedIdentity): Promise<string> {
 const policy = await verifyInvite(share);
 let inbox: string, box: string, epoch: number;
 if (share.owner) {
  if (policy.account !== identity.account || policy.root !== identity.state.root) throw new Error("This share belongs to another encryption identity.");
  inbox = policy.inbox; box = policy.wrap; epoch = policy.epoch;
 } else {
  await verifyClaim(share, policy, identity);
  if (!share.grant || !share.grant_history) throw new Error("Waiting for the sender to grant access.");
  const grant = await authored<NamedGrant>("named-grant", share.grant, share.grant_history);
  if (grant.account !== policy.account || grant.root !== policy.root || grant.share_id !== share.id || grant.invite_id !== share.invitation?.id || grant.sha256 !== policy.sha256 || grant.recipient !== identity.account || grant.recipient_root !== identity.state.root || grant.claim_hash !== await head(share.claim!)) throw new Error("The decryption grant does not match this share and recipient.");
  if (share.grant_history.length < share.owner_history.length) throw new Error("The sender's history conflicts with its invitation.");
  for (let i = 0; i < share.owner_history.length; i++) if (await head(share.grant_history[i]) !== await head(share.owner_history[i])) throw new Error("The sender's history conflicts with its invitation.");
  // A grant may predate a device revocation. Its private key remains in the
  // approved account's encrypted keyring so recovery can still open old shares.
  let bound = false;
  for (const event of identity.record.events) { const state = unpack<{inbox?: string; epoch: number}>(event); if (state.inbox === grant.inbox && state.epoch === grant.epoch) bound = true; }
  if (!bound) throw new Error("The decryption grant names an unknown incoming-share key.");
  inbox = grant.inbox; box = grant.wrap; epoch = grant.epoch;
 }
 const seed = identity.data.inboxes?.[inbox];
 if (!seed) throw new Error("This browser does not have the share's incoming key. Refresh device access and try again.");
 return b64(await unwrap(await privateKey("X25519", unb64(seed, 32)), inbox, box, context(policy.sha256, identity.account, identity.state.root, epoch)));
}
