import { head, unb64, unpack, utf8, verify, hash, type Signed } from "./identity-crypto";
export type Device = { id: string; name: string; sign: string; box: string; wrap: string };
export type IdentityState = { account: string;seq:number;prev:string;kind:"init"|"approve"|"recover"|"revoke"|"sharing";signer:string;root:string;recovery:string;recovery_box:string;recovery_wrap:string;epoch:number;devices:Device[];inbox?:string };
export type VaultSnapshot = {account:string;revision:number;head:string;epoch:number;signer:string;data:string};
export type IdentityRecord = {events:Signed[];vault:Signed};
export type DeviceRequest = {account:string;root:string;name:string;sign:string;box:string;nonce:string;created:number};
export type Receipt = {server:string;account:string;key:string;sha256:string;url:string};
export type KeyVault = {version:1|2;shares:Receipt[];inboxes?:Record<string,string>};
export const deviceID = (sign:string,box:string)=>hash(utf8(sign+"."+box));
export const keyContext = (s:IdentityState)=>`${s.account}/${s.root}/${s.epoch}`;
function wrapper(value:string){if(typeof value!=="string"||value.length!==124||value[43]!==".")throw new Error("Invalid key wrapper");unb64(value.slice(0,43),32);unb64(value.slice(44),60)}
export async function verifyHistory(events:Signed[],account:string):Promise<{state:IdentityState;head:string}>{
 if(!Array.isArray(events)||!events.length||events.length>512)throw new Error("Invalid device history");
 let previous:IdentityState|undefined,previousHead="";
 const inboxes=new Set<string>();
 const bad=()=>{throw new Error("Invalid signed device history")};
 for(let i=0;i<events.length;i++){
  const event=events[i],next=unpack<IdentityState>(event);
  if(next.account!==account||next.seq!==i||next.prev!==previousHead||!Number.isSafeInteger(next.epoch)||next.epoch<1||!Array.isArray(next.devices)||!next.devices.length||next.devices.length>32)bad();
  unb64(next.root,32);unb64(next.recovery,32);unb64(next.recovery_box,92);wrapper(next.recovery_wrap);if(next.inbox)unb64(next.inbox,32);
  const seen=new Set<string>();
  for(const d of next.devices){unb64(d.sign,32);unb64(d.box,32);wrapper(d.wrap);if(d.id!==await deviceID(d.sign,d.box)||seen.has(d.id)||typeof d.name!=="string"||utf8(d.name).length>80)bad();seen.add(d.id)}
  if(!previous){if(next.kind!=="init"||next.signer!=="root"||next.epoch!==1||next.devices.length!==1)bad();await verify("event",event,next.root)}
  else{
   if(next.root!==previous.root||next.recovery!==previous.recovery||next.recovery_box!==previous.recovery_box)bad();
   const signer=previous.devices.find(d=>d.id===next.signer);
   if(next.kind==="recover"?next.signer!=="root":!signer)bad();
   await verify("event",event,next.kind==="recover"?previous.root:signer!.sign);
   let added=0,removed=0;
   for(const d of next.devices){const old=previous.devices.find(p=>p.id===d.id);if(!old){added++;continue}if(d.sign!==old.sign||d.box!==old.box||d.name!==old.name||(next.kind!=="revoke"&&d.wrap!==old.wrap))bad()}
   for(const d of previous.devices)if(!seen.has(d.id))removed++;
   if(next.kind!=="revoke"&&next.kind!=="sharing"&&(next.inbox??"")!==(previous.inbox??""))bad();
   if(next.kind==="sharing"){if(previous.inbox||!next.inbox||added!==0||removed!==0||next.epoch!==previous.epoch||next.recovery_wrap!==previous.recovery_wrap)bad()}
   else if(next.kind==="approve"||next.kind==="recover"){if(added!==1||removed!==0||next.epoch!==previous.epoch||next.recovery_wrap!==previous.recovery_wrap)bad()}
   else if(next.kind==="revoke"){
    if(previous.inbox?(!next.inbox||next.inbox===previous.inbox):!!next.inbox)bad();
    if(added!==0||removed!==1||next.epoch!==previous.epoch+1||!seen.has(next.signer)||next.recovery_wrap===previous.recovery_wrap)bad();
    for(const d of next.devices)if(d.wrap===previous.devices.find(p=>p.id===d.id)?.wrap)bad();
   }else bad();
  }
  // Every incoming-share key is introduced once; a retired key never returns.
  if(next.inbox&&next.inbox!==previous?.inbox){if(inboxes.has(next.inbox))bad();inboxes.add(next.inbox)}
  previous=next;previousHead=await head(event);
 }
 return {state:previous!,head:previousHead};
}
export async function verifyRecord(record:IdentityRecord,account:string):Promise<{state:IdentityState;vault:VaultSnapshot;head:string}>{
 const history=await verifyHistory(record?.events,account),vault=unpack<VaultSnapshot>(record.vault),{state}=history;
 if(vault.account!==account||vault.head!==history.head||vault.epoch!==state.epoch||!Number.isSafeInteger(vault.revision)||vault.revision<1||typeof vault.data!=="string"||vault.data.length>6*1024*1024||unb64(vault.data).length<28)throw new Error("Invalid signed vault");
 const device=state.devices.find(d=>d.id===vault.signer);
 if(vault.signer==="root"?!["init","recover"].includes(state.kind):!device)throw new Error("Unapproved vault writer");
 await verify("vault",record.vault,vault.signer==="root"?state.root:device!.sign);
 return {...history,vault};
}
