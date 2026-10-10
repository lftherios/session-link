import {b64,unb64,utf8,random,concat,pair,privateKey,seal,unseal,wrap,unwrap,sign,verify,unpack,head,hash,confirmation,type Signed} from "./identity-crypto";
import {deviceID,keyContext,verifyRecord,type Device,type DeviceRequest,type IdentityRecord,type IdentityState,type KeyVault,type VaultSnapshot} from "./identity-protocol";

type IdentityUpdate={expected_head:string;expected_revision:number;event?:Signed;vault:Signed};
type LocalDevice={sign:CryptoKey;box:CryptoKey;signPublic:string;boxPublic:string;root:string;seq:number;head:string;revision:number;pendingRecovery?:string;pendingSetup?:IdentityUpdate};
export type IdentityResponse={account:string;record:IdentityRecord|null;requests:Signed[]};
export type IdentityStatus={account:string;state:"setup"|"locked"|"ready";device_id?:string;devices:{id:string;name:string;current:boolean}[];requests:{id:string;name:string}[];confirmation?:string;recovery_key?:string;recovery_pending:boolean;sharing_ready?:boolean};
export type UnlockedIdentity={account:string;device:LocalDevice;deviceID:string;record:IdentityRecord;state:IdentityState;vault:VaultSnapshot;key:Uint8Array<ArrayBuffer>;data:KeyVault;head:string};
export type IdentityInput={name?:string;id?:string;code?:string;recovery_key?:string};
export async function browserRequest<T>(path:string,method="GET",body?:unknown):Promise<T>{
 const response=await fetch(path,{method,credentials:"same-origin",cache:"no-store",headers:{"x-slink":"1",...(body===undefined?{}:{"content-type":"application/json"})},body:body===undefined?undefined:JSON.stringify(body)});
 const text=await response.text();if(text.length>12*1024*1024)throw new Error("Identity response exceeds the supported limit");
 let data;try{data=JSON.parse(text)}catch{throw new Error("Could not read the server response")}
 if(!response.ok)throw Object.assign(new Error(data.error?.message??`Request failed (${response.status})`),{status:response.status});return data as T;
}
async function database():Promise<IDBDatabase>{return new Promise((resolve,reject)=>{const r=indexedDB.open("slink-device-v1",1);r.onupgradeneeded=()=>r.result.createObjectStore("devices");r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(new Error("Browser key storage is unavailable"))})}
async function readLocal(account:string):Promise<LocalDevice|undefined>{const db=await database();try{return await new Promise((resolve,reject)=>{const request=db.transaction("devices","readonly").objectStore("devices").get(account);request.onsuccess=()=>resolve(request.result);request.onerror=()=>reject(request.error)})}finally{db.close()}}
async function saveLocal(account:string,device:LocalDevice):Promise<void>{const db=await database();try{await new Promise<void>((resolve,reject)=>{const transaction=db.transaction("devices","readwrite");transaction.objectStore("devices").put(device,account);transaction.oncomplete=()=>resolve();transaction.onerror=transaction.onabort=()=>reject(new Error("Could not save browser device keys"))})}finally{db.close()}}
async function newDevice():Promise<LocalDevice>{try{const [s,b]=await Promise.all([pair("Ed25519"),pair("X25519")]);return {sign:s.private,box:b.private,signPublic:s.public,boxPublic:b.public,root:"",seq:-1,head:"",revision:0}}catch{throw new Error("This browser does not support private sharing. Use a current browser with Ed25519 and X25519 support.")}}
async function publicDevice(d:LocalDevice,name:string):Promise<Device>{return {id:await deviceID(d.signPublic,d.boxPublic),name,sign:d.signPublic,box:d.boxPublic,wrap:""}}
function nameFor(name?:string){let value=name?.trim()||"This browser";while(utf8(value).length>80)value=Array.from(value).slice(0,-1).join("");return value}
async function pin(account:string,device:LocalDevice,record:IdentityRecord,state:IdentityState,vault:VaultSnapshot){
 // Older clients retained the recovery token before they pinned a root.
 if(device.pendingRecovery){const parts=device.pendingRecovery.split(".");if(parts.length!==3||parts[0]!=="slr1"||parts[1]!==await hash(utf8(account+"\0"+state.root)))throw new Error("The server's recovery identity does not match your saved recovery key.")}
 if(device.root&&device.root!==state.root)throw new Error("The server's recovery identity changed. Refusing to replace your keys.");
 if(device.seq>=record.events.length||device.revision>vault.revision||(device.seq>=0&&device.head&&await head(record.events[device.seq])!==device.head))throw new Error("The server returned an older or conflicting key history.");
 device.root=state.root;device.seq=state.seq;device.head=vault.head;device.revision=vault.revision;delete device.pendingSetup;await saveLocal(account,device);
}
async function decodeVault(key:Uint8Array<ArrayBuffer>,state:IdentityState,vault:VaultSnapshot):Promise<KeyVault>{
 const raw=await unseal(key,vault.data,"slink/vault/v1/"+keyContext(state));if(raw.length>4*1024*1024)throw new Error("Key vault exceeds the supported size");
 const data=JSON.parse(new TextDecoder("utf-8",{fatal:true}).decode(raw)) as KeyVault;
 if(![1,2].includes(data.version)||!Array.isArray(data.shares)||data.shares.length>10000)throw new Error("Invalid key vault");
 for(const share of data.shares){unb64(share.key,32);const url=new URL(share.url);if(share.server!==location.origin||share.account!==state.account||!/^[a-f0-9]{64}$/.test(share.sha256)||url.origin!==location.origin||url.username||url.password||url.search||!/^\/s\/[23456789abcdefghjkmnpqrstuvwxyz]{14}$/.test(url.pathname)||url.hash!=="#key="+share.key)throw new Error("Invalid share receipt")}
 if(state.inbox){if(data.version!==2||!data.inboxes?.[state.inbox]||Object.keys(data.inboxes).length>512)throw new Error("Incoming-share keys are missing");for(const [pub,seed] of Object.entries(data.inboxes)){const base=new Uint8Array(32);base[0]=9;const publicBase=await crypto.subtle.importKey("raw",base,"X25519",false,[]);const derived=await crypto.subtle.deriveBits({name:"X25519",public:publicBase},await privateKey("X25519",unb64(seed,32)),256);if(b64(new Uint8Array(derived))!==pub)throw new Error("Incoming-share key does not match its public identity")}}
 else if(data.version!==1||Object.keys(data.inboxes??{}).length)throw new Error("Unbound incoming-share key");
 return data;
}
async function rotateInbox(state:IdentityState,data:KeyVault){const keys=await pair("X25519",true);state.inbox=keys.public;data.inboxes??={};data.inboxes[keys.public]=b64(keys.seed!);data.version=2}
async function prepareUpdate(record:IdentityRecord|null,state:IdentityState,data:KeyVault,key:Uint8Array<ArrayBuffer>,signer:CryptoKey,event?:Signed):Promise<IdentityUpdate>{
 const old=record?unpack<VaultSnapshot>(record.vault):null,raw=utf8(JSON.stringify(data));if(raw.length>4*1024*1024)throw new Error("Key vault exceeds the supported size");
 const value:VaultSnapshot={account:state.account,revision:(old?.revision??0)+1,head:event?await head(event):old?.head??"",epoch:state.epoch,signer:state.signer,data:await seal(key,raw,"slink/vault/v1/"+keyContext(state))};
 return {expected_head:old?.head??"",expected_revision:old?.revision??0,...(event?{event}:{}),vault:await sign("vault",value,signer)};
}
async function commit(record:IdentityRecord|null,state:IdentityState,data:KeyVault,key:Uint8Array<ArrayBuffer>,signer:CryptoKey,event?:Signed):Promise<IdentityResponse>{
 return browserRequest("/api/identity","POST",await prepareUpdate(record,state,data,key,signer,event));
}
export async function unlockedIdentity():Promise<UnlockedIdentity>{
 if(!navigator.locks)throw new Error("This browser does not support private device storage locks. Update your browser.");
 return navigator.locks.request("slink-identity",unlockCurrent);
}
async function unlockCurrent():Promise<UnlockedIdentity>{
 const remote=await browserRequest<IdentityResponse>("/api/identity");if(!remote.record)throw new Error("Set up private access first");
 const verified=await verifyRecord(remote.record,remote.account),d=await readLocal(remote.account);if(!d)throw new Error("Approve this browser or use your recovery key");
 await pin(remote.account,d,remote.record,verified.state,verified.vault);
 const id=await deviceID(d.signPublic,d.boxPublic),own=verified.state.devices.find(v=>v.id===id);if(!own)throw new Error("This browser needs device approval or recovery");
 const key=await unwrap(d.box,d.boxPublic,own.wrap,keyContext(verified.state)),data=await decodeVault(key,verified.state,verified.vault);
 return {account:remote.account,device:d,deviceID:id,record:remote.record,...verified,key,data};
}
export async function browserIdentity(action="status",input:IdentityInput={}):Promise<IdentityStatus>{
 if(!navigator.locks)throw new Error("This browser does not support private device storage locks. Update your browser.");
 return navigator.locks.request("slink-identity",()=>identityAction(action,input));
}
async function identityAction(action:string,input:IdentityInput):Promise<IdentityStatus>{
 let remote=await browserRequest<IdentityResponse>("/api/identity"),d=await readLocal(remote.account);
 const result:IdentityStatus={account:remote.account,state:"setup",devices:[],requests:[],recovery_pending:false};
 if(!remote.record){
  if(d?.root&&!d.pendingSetup)throw new Error("The server's encrypted identity is missing. Refusing to replace it.");
  if(action==="status")return result;if(action!=="setup")throw new Error("Set up private access first");
  d??=await newDevice();
  if(!d.pendingSetup){
   const [root,recovery]=await Promise.all([pair("Ed25519",true),pair("X25519",true)]),secret=random(),key=random();
   const state:IdentityState={account:remote.account,seq:0,prev:"",kind:"init",signer:"root",root:root.public,recovery:recovery.public,recovery_box:await seal(secret,concat(root.seed!,recovery.seed!),`slink/recovery/v1/${remote.account}/${root.public}`),recovery_wrap:"",epoch:1,devices:[]};
   const own=await publicDevice(d,nameFor(input.name));own.wrap=await wrap(own.box,key,keyContext(state));state.devices=[own];state.recovery_wrap=await wrap(state.recovery,key,keyContext(state));
   const data:KeyVault={version:2,shares:[]};await rotateInbox(state,data);
   const event=await sign("event",state,root.private);
   // Persist our genesis and exact encrypted request before contacting the
   // server. Retrying an interrupted setup must keep the same trust anchor.
   d.root=state.root;d.seq=0;d.head=await head(event);d.revision=1;
   d.pendingSetup=await prepareUpdate(null,state,data,key,root.private,event);
   d.pendingRecovery=`slr1.${await hash(utf8(state.account+"\0"+state.root))}.${b64(secret)}`;await saveLocal(remote.account,d);
  }
  const account=remote.account;
  remote=await browserRequest<IdentityResponse>("/api/identity","POST",d.pendingSetup);
  if(remote.account!==account||!remote.record)throw new Error("The server did not acknowledge the initial encrypted identity.");
 }
 const record=remote.record!,{state,vault}=await verifyRecord(record,remote.account);
 if(d)await pin(remote.account,d,record,state,vault);
 if(!d&&(action==="request"||action==="recover")){d=await newDevice();await pin(remote.account,d,record,state,vault)}
 const id=d?await deviceID(d.signPublic,d.boxPublic):"",own=state.devices.find(v=>v.id===id);
 result.sharing_ready=!!state.inbox;result.device_id=id;result.state=own?"ready":"locked";result.devices=state.devices.map(v=>({id:v.id,name:v.name,current:v.id===id}));result.recovery_pending=!!d?.pendingRecovery;
 for(const signed of remote.requests){const r=unpack<DeviceRequest>(signed);if(r.account!==state.account||r.root!==state.root||r.created<Date.now()-600000)continue;await verify("request",signed,r.sign);const requestID=await deviceID(r.sign,r.box);result.requests.push({id:requestID,name:r.name});if(requestID===id)result.confirmation=await confirmation(signed)}
 if(action==="request"&&!own){
  const request:DeviceRequest={account:state.account,root:state.root,name:nameFor(input.name),sign:d!.signPublic,box:d!.boxPublic,nonce:b64(random(24)),created:Date.now()};
  const signed=await sign("request",request,d!.sign);await browserRequest("/api/identity","POST",{request:signed});result.confirmation=await confirmation(signed);return result;
 }
 if(action==="recover"&&!own){
  const parts=input.recovery_key?.trim().split(".")??[];if(parts.length!==3||parts[0]!=="slr1"||parts[1]!==await hash(utf8(state.account+"\0"+state.root)))throw new Error("Recovery key does not match this account");
  const raw=await unseal(unb64(parts[2],32),state.recovery_box,`slink/recovery/v1/${state.account}/${state.root}`);if(raw.length!==64)throw new Error("Invalid recovery package");
  const root=await privateKey("Ed25519",raw.slice(0,32)),recovery=await privateKey("X25519",raw.slice(32));
  const proof=await sign("recovery-check",{account:state.account},root);await verify("recovery-check",proof,state.root);
  const key=await unwrap(recovery,state.recovery,state.recovery_wrap,keyContext(state)),data=await decodeVault(key,state,vault);
  d=await newDevice();await pin(remote.account,d,record,state,vault);
  const added=await publicDevice(d,nameFor(input.name));added.wrap=await wrap(added.box,key,keyContext(state));state.devices.push(added);state.kind="recover";state.signer="root";state.seq++;state.prev=vault.head;
  await commit(record,state,data,key,root,await sign("event",state,root));return identityAction("status",{});
 }
 if(!own){if(action!=="status")throw new Error("Approve this browser or use your recovery key");return result}
 const key=await unwrap(d!.box,d!.boxPublic,own.wrap,keyContext(state)),data=await decodeVault(key,state,vault);
 if(action==="setup"&&d!.pendingRecovery){result.recovery_key=d!.pendingRecovery;return result}
 if(action==="confirm-recovery"){delete d!.pendingRecovery;await saveLocal(state.account,d!);result.recovery_pending=false;return result}
 if(action==="status")return result;
 if(action==="sharing"&&state.inbox)return result;
 if(!["sharing","approve","revoke"].includes(action))throw new Error("Unknown device operation");
 state.signer=id;
 if(action==="sharing")await rotateInbox(state,data);
 if(action==="approve"){
  let pending:Signed|undefined;for(const signed of remote.requests){const request=unpack<DeviceRequest>(signed);if(await deviceID(request.sign,request.box)===input.id)pending=signed}
  if(!pending||(input.code??"").replace(/[\s-]/g,"").toUpperCase()!==(await confirmation(pending)).replace(/-/g,""))throw new Error("Device code does not match the new device");
  const request=unpack<DeviceRequest>(pending);if(request.account!==state.account||request.root!==state.root)throw new Error("Device request identity changed");await verify("request",pending,request.sign);
  state.devices.push({id:input.id!,name:request.name,sign:request.sign,box:request.box,wrap:await wrap(request.box,key,keyContext(state))});
 }
 let nextKey=key;
 if(action==="revoke"){
  if(input.id===id||!state.devices.some(v=>v.id===input.id))throw new Error("Revoke this browser from another approved device");
  state.devices=state.devices.filter(v=>v.id!==input.id);state.epoch++;nextKey=random();if(state.inbox)await rotateInbox(state,data);
  for(const device of state.devices)device.wrap=await wrap(device.box,nextKey,keyContext(state));state.recovery_wrap=await wrap(state.recovery,nextKey,keyContext(state));
 }
 state.kind=action as IdentityState["kind"];state.seq++;state.prev=vault.head;
 await commit(record,state,data,nextKey,d!.sign,await sign("event",state,d!.sign));return identityAction("status",{});
}
