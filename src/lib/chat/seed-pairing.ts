export interface SeedPairing {address:string;token:string}
export function pairingAddress(value:string,folder:string):string {
  const url=new URL(value.trim());
  if(url.protocol!=='https:'&&!(url.protocol==='http:'&&['localhost','127.0.0.1','[::1]'].includes(url.hostname)))throw Error('Use HTTPS or a loopback address');
  if(url.username||url.password||url.search||url.hash)throw Error('Invalid pairing address');
  url.pathname=url.pathname.replace(/\/$/,'');
  if(url.pathname.endsWith('/mcp'))url.pathname+='/seed-library/'+encodeURIComponent(folder);
  if(!url.pathname.endsWith('/mcp/seed-library/'+encodeURIComponent(folder)))throw Error('Pairing address belongs to another project');
  return url.toString();
}
export const pairingKey=(workspace:string,folder:string)=>`ctmcp-seed-pairing:${JSON.stringify([workspace,folder])}`;
/** Explicitly saved browser-pairing credential; enrollment and website tokens never enter this store. */
export function loadPairing(key:string):SeedPairing {
  try{const value=JSON.parse(localStorage.getItem(key)||'{}');return {address:typeof value.address==='string'?value.address:'',token:typeof value.token==='string'?value.token:''};}catch{return {address:'',token:''};}
}
export function savePairing(key:string,value:SeedPairing):boolean {
  try{localStorage.setItem(key,JSON.stringify({address:value.address,token:value.token}));return true;}catch{return false;}
}
export function newPairingToken():string {return [...crypto.getRandomValues(new Uint8Array(32))].map(x=>x.toString(16).padStart(2,'0')).join('');}
