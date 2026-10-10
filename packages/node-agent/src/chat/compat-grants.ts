import { randomBytes, randomUUID } from 'node:crypto';

export interface CompatGrant {
  key:string; id:string; profile:string; folder:string; root:string; chat:string;
  attempt:string; expires_at:number; attachment?:string; busy:boolean;
}
/** Independent capabilities; never serialized into downloaded responses or chat storage. */
export class CompatGrantRegistry {
  private entries = new Map<string, CompatGrant>();
  constructor(private now:()=>number = Date.now) {}
  private prune() { for (const [key,g] of this.entries) if (g.expires_at <= this.now()) this.entries.delete(key); }
  find(profile:string, folder:string, chat:string, attempt?:string) {
    this.prune(); return [...this.entries.values()].find(g=>g.profile===profile&&g.folder===folder&&g.chat===chat&&(attempt===undefined||g.attempt===attempt));
  }
  revoke(profile:string, folder?:string, chat?:string) {
    for (const [key,g] of this.entries) if(g.profile===profile&&(!folder||g.folder===folder)&&(!chat||g.chat===chat)) this.entries.delete(key);
  }
  issue(profile:string, folder:string, root:string, chat:string, attempt:string) {
    this.prune(); this.revoke(profile,folder,chat);
    if(this.entries.size>=256) throw new Error('Compatibility grant limit reached');
    const g:CompatGrant={key:randomBytes(32).toString('hex'),id:randomUUID(),profile,folder,root,chat,attempt,expires_at:this.now()+30*60_000,busy:false};
    this.entries.set(g.key,g);return g;
  }
  get(key:string, profile:string) {
    this.prune(); const g=this.entries.get(key);
    if(!g||g.profile!==profile) throw new Error('Compatibility authorization invalid, expired or revoked');
    return g;
  }
  publicGrant(g:CompatGrant) {return {key:g.key,grant_id:g.id,expires_at:g.expires_at};}
}
export const compatGrants = new CompatGrantRegistry();
