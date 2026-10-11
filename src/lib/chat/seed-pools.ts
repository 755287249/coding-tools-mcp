import type {ReadySeedGroup,SeedFilter} from '../api/seeds';
export type PoolMode = 'prefix'|'account'|'source';
export interface SeedPool {key:string;label:string;ready:number;filter:SeedFilter}
/** Aggregate only allocatable inventory. Modal filters narrow the actual choice. */
export function availablePools(groups:ReadySeedGroup[],mode:PoolMode,query='',source=''):SeedPool[] {
  const pools=new Map<string,SeedPool>(),search=query.trim().toLowerCase();
  for(const group of groups){
    if(group.ready<=0||(source&&group.source!==source))continue;
    const label=group[mode];
    if(search&&!label.toLowerCase().includes(search))continue;
    const pool=pools.get(label)??{key:JSON.stringify([mode,label]),label,ready:0,filter:{source,keyword:'',...(mode==='prefix'?{prefix:label}:mode==='account'?{account:label}:{source:label})}};
    pool.ready+=group.ready;pools.set(label,pool);
  }
  return [...pools.values()].sort((a,b)=>b.ready-a.ready||a.label.localeCompare(b.label));
}
