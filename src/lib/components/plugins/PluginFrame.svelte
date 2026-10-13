<script lang="ts">
  import { getBackend } from '$lib/backend';
  import { seedRequest } from '$lib/api/seeds';
  import { pluginProgress, pluginSlot, pluginStop, pluginStatus, setPluginStatus } from '$lib/plugins/runtime';
  import { onMount } from 'svelte';
  import { translate } from '$lib/i18n';
  import type { PluginPackage, PluginHttpRequest } from '$lib/plugins/types';
  import { mcpConnection } from '$lib/stores/app';
  import { appUrl } from '$lib/app-path';
  let { package: pluginPackage }: { package: PluginPackage } = $props();
  let frame: HTMLIFrameElement;
  const id=$derived(pluginPackage.manifest.id);
  const slot=$derived($pluginSlot?.id===id?$pluginSlot:null), rect=$derived(slot?.rect);
  const status=$derived($pluginStatus[id]?.state??'loading');
  let lastStop=0;
  // A frame that never reports back is almost always blocked by a security policy; say so instead of staying blank.
  onMount(()=>{setPluginStatus(id,{state:'loading'});const timer=setTimeout(()=>{if(($pluginStatus[id]?.state??'loading')==='loading')setPluginStatus(id,{state:'error',error:translate('plugins.frameTimeout')})},12000);// Re-send context when the client language or theme changes so plugins can follow it.
    const observer=new MutationObserver(()=>{if(($pluginStatus[id]?.state)==='ready')sendEvent('context',context())});observer.observe(document.documentElement,{attributes:true,attributeFilter:['lang','data-theme']});
    return()=>{clearTimeout(timer);observer.disconnect()}});
  function frameMessage(type:string,error?:string){const current=$pluginStatus[id]?.state;if(type==='error'){if(current!=='ready')setPluginStatus(id,{state:'error',error:String(error??'Plugin error')});}else if(type==='loaded'){if(current!=='error'&&current!=='ready')setPluginStatus(id,{state:'loaded'});}}
  $effect(()=>{const value=$pluginStop;if(value!==lastStop){lastStop=value;if($pluginProgress?.pluginId===id)sendEvent('stop',null)}});
  $effect(()=>{slot?.workspaceId;slot?.folderId;if(slot)sendEvent('context',context())});
  function context(){return {pluginId:id,workspaceId:slot?.workspaceId,folderId:slot?.folderId,publicMcpUrl:$mcpConnection?.tunnel.public_url,locale:document.documentElement.lang||navigator.language,theme:document.documentElement.dataset.theme??''}}
  function response(requestId:number,result?:unknown,error?:unknown){frame?.contentWindow?.postMessage({channel:'ctmcp-host',type:'response',id:requestId,result,error:error?String(error):undefined},'*')}
  function sendEvent(name:string,value:unknown){frame?.contentWindow?.postMessage({channel:'ctmcp-host',type:'event',name,value},'*')}
  function loadPlugin(){frame?.contentWindow?.postMessage({channel:'ctmcp-host',type:'load',code:pluginPackage.code},'*')}
  function hostAllowed(raw:string):boolean { try { const url=new URL(raw);return url.protocol==='https:'&&(pluginPackage.manifest.permissions?.hosts??[]).some(host=>host.toLowerCase()===url.host.toLowerCase()); } catch{return false} }
  async function request(method:string,params:any):Promise<unknown>{
    const permissions=pluginPackage.manifest.permissions??{};
    if(method==='ready'){setPluginStatus(id,{state:'ready'});queueMicrotask(()=>sendEvent('context',context()));return null} if(method==='context')return context();
    if(method==='http'){if(!hostAllowed(params?.url))throw Error(`Plugin ${id} is not allowed to access this host`);return getBackend().plugins.http(params as PluginHttpRequest)}
    if(method==='storage.get'){if(!permissions.storage)throw Error('Plugin storage permission denied');return getBackend().plugins.dataGet(id)}
    if(method==='storage.set'){if(!permissions.storage)throw Error('Plugin storage permission denied');return getBackend().plugins.dataSet(id,params)}
    if(method==='seedLibrary'){if(!permissions.seedLibrary||!slot)throw Error('Seed library permission denied or no project selected');return seedRequest(slot.workspaceId,slot.folderId,params)}
    if(method==='progress'){if(!permissions.progress)throw Error('Progress permission denied');if(params===null)pluginProgress.set(null);else pluginProgress.set({pluginId:id,name:pluginPackage.manifest.name,total:Number(params.total)||0,done:Number(params.done)||0,status:params.status??'running',message:String(params.message??''),detail:params.detail,minimized:!!params.minimized});return null}
    throw Error(`Unknown plugin RPC method: ${method}`);
  }
  function message(event:MessageEvent){const value=event.data;if(event.source!==frame?.contentWindow)return;if(value?.channel==='ctmcp-plugin-frame'){if(value.type==='ready')loadPlugin();else frameMessage(value.type,value.error);return}if(value?.channel!=='ctmcp-plugin'||value.type!=='request')return;void request(value.method,value.params).then(result=>response(value.id,result),error=>response(value.id,undefined,error));}
</script>
<svelte:window onmessage={message}/>
<iframe bind:this={frame} title={pluginPackage.manifest.name} sandbox="allow-scripts" src={appUrl('/plugin-frame.html')} class:active={!!slot&&(status==='loaded'||status==='ready')} style={rect?`left:${rect.left}px;top:${rect.top}px;width:${rect.width}px;height:${rect.height}px`:''}></iframe>
<style>iframe{position:fixed;left:-10000px;top:-10000px;width:1px;height:1px;border:0;border-radius:12px;background:white;visibility:hidden;z-index:850}iframe.active{visibility:visible}</style>
