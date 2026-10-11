import {writable,get} from 'svelte/store';
import {distributionCall,downloadAppUpdate,type AppRelease} from '$lib/api/distribution';
import {createUpdateDownload,downloadBusy} from '$lib/update-download';
import {getBackend} from '$lib/backend';
import {localDesktop} from '$lib/api/distribution';
import {showToast} from '$lib/stores/toast';
import {t} from '$lib/i18n';
export const appUpdate=writable<{release:AppRelease|null;busy:boolean;error:string;checkedRepo:string}>({release:null,busy:false,error:'',checkedRepo:''});
export const appDownload=createUpdateDownload(downloadAppUpdate);
export const DEFAULT_UPDATE_SOURCE='https://arena.755.cc.cd/ctmcp';
export function updateRepository(){try{const saved=localStorage.getItem('ctmcp-update-repo');return !saved||saved==='755287249/coding-tools-mcp'?DEFAULT_UPDATE_SOURCE:saved}catch{return DEFAULT_UPDATE_SOURCE}}
export async function checkAppUpdate(repo=updateRepository()){
 if(get(appUpdate).busy||downloadBusy(get(appDownload))||getBackend().capabilities.host==='node')return;
 appDownload.reset();
 appUpdate.set({release:null,busy:true,error:'',checkedRepo:repo});
 try{const release=await distributionCall<AppRelease>('check_app_update',{repo});appUpdate.set({release,busy:false,error:'',checkedRepo:repo});try{localStorage.setItem('ctmcp-update-repo',repo)}catch{}}
 catch(e){appUpdate.set({release:null,busy:false,error:String(e),checkedRepo:repo})}
}

export interface InstallOutcome {id:string;status:'installed'|'failed'|'rolled_back';version:string;message:string;finishedAt:number;runningExpectedVersion:boolean}
export const appInstallOutcome=writable<InstallOutcome|null>(null);
export async function loadInstallOutcome(){
 if(!localDesktop())return;
 try{
  const outcome=await distributionCall<InstallOutcome|null>('get_app_update_outcome');appInstallOutcome.set(outcome);
  if(!outcome)return;
  let seen=false;try{seen=localStorage.getItem('ctmcp-update-outcome')===outcome.id}catch{}
  if(!seen){showToast(get(t)(outcome.runningExpectedVersion?'updates.installedOutcome':outcome.status==='rolled_back'?'updates.rolledBackOutcome':'updates.failedOutcome')+' '+outcome.version,{kind:outcome.runningExpectedVersion?'success':'error',duration:10000});try{localStorage.setItem('ctmcp-update-outcome',outcome.id)}catch{}}
 }catch{/* Keep the update UI usable when an older host lacks this optional command. */}
}
