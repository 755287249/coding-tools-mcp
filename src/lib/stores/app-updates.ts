import {writable,get} from 'svelte/store';
import {distributionCall,type AppRelease} from '$lib/api/distribution';
import {getBackend} from '$lib/backend';
export const appUpdate=writable<{release:AppRelease|null;busy:boolean;error:string;checkedRepo:string}>({release:null,busy:false,error:'',checkedRepo:''});
export const DEFAULT_UPDATE_SOURCE='https://arena.755.cc.cd/ctmcp';
export function updateRepository(){try{const saved=localStorage.getItem('ctmcp-update-repo');return !saved||saved==='755287249/coding-tools-mcp'?DEFAULT_UPDATE_SOURCE:saved}catch{return DEFAULT_UPDATE_SOURCE}}
export async function checkAppUpdate(repo=updateRepository()){
 if(get(appUpdate).busy||getBackend().capabilities.host==='node')return;
 appUpdate.set({release:null,busy:true,error:'',checkedRepo:repo});
 try{const release=await distributionCall<AppRelease>('check_app_update',{repo});appUpdate.set({release,busy:false,error:'',checkedRepo:repo});try{localStorage.setItem('ctmcp-update-repo',repo)}catch{}}
 catch(e){appUpdate.set({release:null,busy:false,error:String(e),checkedRepo:repo})}
}
