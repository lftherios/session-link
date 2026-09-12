// Wire primitives shared by browser identity and named grants; Go implements
// the same domains in internal/identity. Only standard Web Crypto algorithms.
export const utf8 = (s: string): Uint8Array<ArrayBuffer> => new TextEncoder().encode(s);
export function b64(b: Uint8Array): string { let s=""; for(let i=0;i<b.length;i+=8192)s+=String.fromCharCode(...b.subarray(i,i+8192)); return btoa(s).replace(/\+/g,"-").replace(/\//g,"_").replace(/=+$/,""); }
export function unb64(s: string,n=0): Uint8Array<ArrayBuffer> {
 if(typeof s!=="string"||!/^[A-Za-z0-9_-]*$/.test(s))throw new Error("Invalid key encoding");
 const b=Uint8Array.from(atob(s.replace(/-/g,"+").replace(/_/g,"/")),c=>c.charCodeAt(0));
 if(b64(b)!==s||(n&&b.length!==n))throw new Error("Invalid key encoding");return b;
}
export const random = (n=32): Uint8Array<ArrayBuffer> => crypto.getRandomValues(new Uint8Array(n));
export function concat(...parts: Uint8Array[]): Uint8Array<ArrayBuffer> {const b=new Uint8Array(parts.reduce((n,p)=>n+p.length,0));let offset=0;for(const p of parts){b.set(p,offset);offset+=p.length}return b;}
export const hash = async (b: Uint8Array<ArrayBuffer>) => b64(new Uint8Array(await crypto.subtle.digest("SHA-256",b)));
export const hexHash = async (b: Uint8Array<ArrayBuffer>) => Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256",b)),v=>v.toString(16).padStart(2,"0")).join("");
export type Signed = { payload: string; signature: string };
export const unpack = <T>(s: Signed): T => JSON.parse(new TextDecoder("utf-8",{fatal:true}).decode(unb64(s.payload))) as T;
export const head = async(s: Signed)=>hash(unb64(s.payload));
export async function sign(kind: string,value: unknown,key: CryptoKey): Promise<Signed>{
 const raw=utf8(JSON.stringify(value));return {payload:b64(raw),signature:b64(new Uint8Array(await crypto.subtle.sign("Ed25519",key,concat(utf8(`slink/${kind}/v1\0`),raw))))};
}
export async function verify(kind: string,s: Signed,publicKey: string): Promise<void>{
 const key=await crypto.subtle.importKey("raw",unb64(publicKey,32),"Ed25519",false,["verify"]);
 if(!await crypto.subtle.verify("Ed25519",key,unb64(s.signature,64),concat(utf8(`slink/${kind}/v1\0`),unb64(s.payload))))throw new Error(`Invalid ${kind} signature`);
}
export async function privateKey(algorithm: "Ed25519"|"X25519",seed: Uint8Array<ArrayBuffer>):Promise<CryptoKey>{
 if(seed.length!==32)throw new Error("Invalid private key");
 const prefix=Uint8Array.from([0x30,0x2e,2,1,0,0x30,5,6,3,0x2b,0x65,algorithm==="Ed25519"?0x70:0x6e,4,0x22,4,0x20]);
 return crypto.subtle.importKey("pkcs8",concat(prefix,seed),algorithm,false,algorithm==="Ed25519"?["sign"]:["deriveBits"]);
}
export async function pair(algorithm: "Ed25519"|"X25519",extractable=false):Promise<{private: CryptoKey;public: string;seed?: Uint8Array<ArrayBuffer>}>{
 const keys=await crypto.subtle.generateKey(algorithm,extractable,algorithm==="Ed25519"?["sign","verify"]:["deriveBits"]) as CryptoKeyPair;
 const seed=extractable?unb64((await crypto.subtle.exportKey("jwk",keys.privateKey)).d!,32):undefined;
 return {private:keys.privateKey,public:b64(new Uint8Array(await crypto.subtle.exportKey("raw",keys.publicKey))),seed};
}
export async function seal(key: Uint8Array<ArrayBuffer>,plain: Uint8Array<ArrayBuffer>,context: string):Promise<string>{
 const k=await crypto.subtle.importKey("raw",key,"AES-GCM",false,["encrypt"]),nonce=random(12);
 return b64(concat(nonce,new Uint8Array(await crypto.subtle.encrypt({name:"AES-GCM",iv:nonce,additionalData:utf8(context)},k,plain))));
}
export async function unseal(key: Uint8Array<ArrayBuffer>,box: string,context: string):Promise<Uint8Array<ArrayBuffer>>{
 const bytes=unb64(box);if(bytes.length<28)throw new Error("Truncated encrypted key");
 const k=await crypto.subtle.importKey("raw",key,"AES-GCM",false,["decrypt"]);
 try{return new Uint8Array(await crypto.subtle.decrypt({name:"AES-GCM",iv:bytes.slice(0,12),additionalData:utf8(context)},k,bytes.slice(12)))}catch{throw new Error("Could not unlock encrypted keys")}
}
async function derived(privateKey: CryptoKey,publicKey: string,info: string):Promise<Uint8Array<ArrayBuffer>>{
 const pub=await crypto.subtle.importKey("raw",unb64(publicKey,32),"X25519",false,[]);
 const secret=await crypto.subtle.deriveBits({name:"X25519",public:pub},privateKey,256);
 const material=await crypto.subtle.importKey("raw",secret,"HKDF",false,["deriveBits"]);
 return new Uint8Array(await crypto.subtle.deriveBits({name:"HKDF",hash:"SHA-256",salt:new Uint8Array(),info:utf8(info)},material,256));
}
export async function wrap(publicKey: string,key: Uint8Array<ArrayBuffer>,context: string):Promise<string>{
 const ephemeral=await pair("X25519"),info=`slink/wrap/v1/${context}/${ephemeral.public}/${publicKey}`;
 return ephemeral.public+"."+await seal(await derived(ephemeral.private,publicKey,info),key,info);
}
export async function unwrap(privateKey: CryptoKey,publicKey: string,box: string,context: string):Promise<Uint8Array<ArrayBuffer>>{
 if(box.length!==124||box[43]!==".")throw new Error("Invalid key wrapper");
 const info=`slink/wrap/v1/${context}/${box.slice(0,43)}/${publicKey}`;
 const key=await unseal(await derived(privateKey,box.slice(0,43),info),box.slice(44),info);if(key.length!==32)throw new Error("Invalid vault key");return key;
}
export async function confirmation(s: Signed):Promise<string>{
 const bytes=unb64(await head(s)),alphabet="ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";let bits=0,value=0,out="";
 for(const byte of bytes){value=(value<<8)|byte;bits+=8;while(bits>=5&&out.length<12){bits-=5;out+=alphabet[(value>>>bits)&31]}}
 return `${out.slice(0,4)}-${out.slice(4,8)}-${out.slice(8)}`;
}
export async function bindingProof(secret: string,s: Signed):Promise<string>{
 const k=await crypto.subtle.importKey("raw",unb64(secret,32),{name:"HMAC",hash:"SHA-256"},false,["sign"]);
 return b64(new Uint8Array(await crypto.subtle.sign("HMAC",k,concat(utf8("slink/recipient-binding/v1\0"),unb64(s.payload)))));
}
