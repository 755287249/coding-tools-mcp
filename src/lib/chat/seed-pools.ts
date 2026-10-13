import type {PoolSeedGroup,ReadySeedGroup,SeedFilter} from '../api/seeds';
export type PoolMode = 'prefix'|'account'|'source';
export interface SeedPool {key:string;label:string;ready:number;total:number;filter:SeedFilter}
type Group = ReadySeedGroup & {total?:number;standby?:boolean};
/**
 * Aggregate pools for one category. By default only allocatable pools are
 * returned; `includeEmpty` keeps pools with seeds but 0 ready so the chooser
 * can show them greyed out instead of hiding the category.
 */
export function availablePools(groups:Group[],mode:PoolMode,query='',source='',includeEmpty=false):SeedPool[] {
  const pools=new Map<string,SeedPool>(),search=query.trim().toLowerCase();
  for(const group of groups){
    const total=group.total??group.ready;
    if((includeEmpty?total:group.ready)<=0||(source&&group.source!==source))continue;
    const label=group[mode];
    if(search&&!label.toLowerCase().includes(search))continue;
    const pool=pools.get(label)??{key:JSON.stringify([mode,label]),label,ready:0,total:0,filter:{source,keyword:'',...(mode==='prefix'?{prefix:label}:mode==='account'?{account:label}:{source:label})}};
    pool.ready+=group.ready;pool.total+=total;pools.set(label,pool);
  }
  return [...pools.values()].sort((a,b)=>b.ready-a.ready||b.total-a.total||a.label.localeCompare(b.label));
}
/** Ready and total standby seeds (AIs that left a chat while still valid). */
export function standbyPool(groups:PoolSeedGroup[]):{ready:number;total:number;filter:SeedFilter} {
  let ready=0,total=0;
  for(const group of groups)if(group.standby){ready+=group.ready;total+=group.total;}
  return {ready,total,filter:{source:'',keyword:'',standby:true}};
}
