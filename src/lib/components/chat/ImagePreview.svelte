<script lang="ts">
  import { untrack, getContext } from 'svelte';
  import ChevronLeft from '@lucide/svelte/icons/chevron-left';
  import ChevronRight from '@lucide/svelte/icons/chevron-right';
  import { IMAGE_GALLERY, imageKey, uniqueImages, type ImageGallery, type PreviewImage } from '$lib/chat/image-gallery';
  import { localChat } from '$lib/api/chat';
  import { readChatFile } from '$lib/chat/attachment-transfer';
  import { t } from '$lib/i18n';
  import Minus from '@lucide/svelte/icons/minus';
  import Square from '@lucide/svelte/icons/square';
  import Copy from '@lucide/svelte/icons/copy';
  import X from '@lucide/svelte/icons/x';
  import Grip from '@lucide/svelte/icons/grip';
  import LocalPathLink from './LocalPathLink.svelte';
  import { boundPreview, centerPreview, resizePreview, PREVIEW_HEADER_HEIGHT, type PreviewRect } from '$lib/chat/preview-window';
  let { src, name, openRequest = 0, thumbnail = true, workspaceId='', folderId='', chatId='', path='', images }: {images?:PreviewImage[];src: string; name: string; openRequest?:number; thumbnail?:boolean;workspaceId?:string;folderId?:string;chatId?:string;path?:string} = $props();
  let viewer = $state<HTMLDivElement>();
  let titlebar = $state<HTMLDivElement>();
  let visible = $state(false), minimized = $state(false), maximized = $state(false);
  let rect = $state<PreviewRect>({x:8,y:8,width:800,height:600});
  let restored: PreviewRect | null = null;
  let opener: HTMLElement | null = null;
  let gesture: {id:number; x:number; y:number; rect:PreviewRect; resize:boolean} | null = null;
  let zoom = $state(100);
  const gallery=getContext<ImageGallery|undefined>(IMAGE_GALLERY);
  let sequence=$state<PreviewImage[]>([]), index=$state(0), displayedSrc=$state(''), loading=$state(false), failed=$state(false), retry=$state(0);
  let canvas=$state<HTMLDivElement>();
  const original=$derived({workspaceId,folderId,chatId,path,name});
  const current=$derived(sequence[index]??original);
  $effect(()=>{
    const item=current, initial=original, initialSrc=src, isVisible=visible, attempt=retry;
    let cancelled=false, objectUrl='';
    displayedSrc='';failed=false;loading=false;
    if(canvas){canvas.scrollTop=0;canvas.scrollLeft=0;}
    if(isVisible){
      if(imageKey(item)===imageKey(initial))displayedSrc=initialSrc;
      else {
        loading=true;
        untrack(()=>{void (async()=>{
          try {
            const request=(args:Parameters<typeof localChat>[2])=>{if(cancelled)throw Error('Preview cancelled');return localChat(item.workspaceId,item.folderId,args);};
            if(item.file){
              const bytes=await readChatFile(request,item.chatId,item.file);
              if(cancelled)return;
              objectUrl=URL.createObjectURL(new Blob([bytes as BlobPart],{type:item.file.mime}));displayedSrc=objectUrl;
            }else{
              const result=await request({action:'read_artifact',chat_id:item.chatId,source_path:item.path});
              if(cancelled)return;
              if(!result.data_base64||!['image/png','image/jpeg','image/gif','image/webp'].includes(result.mime??''))throw Error('Preview unavailable');
              displayedSrc=`data:${result.mime};base64,${result.data_base64}`;
            }
          }catch{if(!cancelled)failed=true}finally{if(!cancelled)loading=false}
        })()});
      }
    }
    return()=>{cancelled=true;if(objectUrl)URL.revokeObjectURL(objectUrl)};
  });
  function navigate(step:number){
    const next=index+step;
    if(next<0||next>=sequence.length)return;
    index=next;zoom=100;
  }
  function viewerKeys(event:KeyboardEvent){
    if(event.key==='Escape'){event.preventDefault();event.stopPropagation();closeViewer();return;}
    if(event.defaultPrevented||minimized||event.altKey||event.ctrlKey||event.metaKey||(event.target as HTMLElement).closest('input,.window-titlebar,.resize-grip'))return;
    if(sequence.length>1&&(event.key==='ArrowLeft'||event.key==='ArrowRight')){event.preventDefault();event.stopPropagation();navigate(event.key==='ArrowLeft'?-1:1);}
  }
  const viewport = () => ({width:window.innerWidth,height:window.innerHeight});
  function openViewer() {
    if (!viewer) return;
    const candidates=uniqueImages(images??gallery?.()??[]);
    const start=candidates.findIndex(item=>imageKey(item)===imageKey(original));
    sequence=start<0?[original]:candidates;index=Math.max(0,start);zoom=100;
    if (!visible) {
      opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
      rect = centerPreview(viewport()); zoom = 100; maximized = false; restored = null;
      viewer.showPopover(); visible = true;
    }
    minimized = false; rect = boundPreview(rect,viewport());
    viewer.focus();
  }
  function closeViewer() {
    viewer?.hidePopover(); visible = false; gesture = null;
    if (opener?.isConnected) opener.focus();
  }
  function toggleMaximize() {
    gesture = null; minimized = false;
    if (maximized) { rect = boundPreview(restored ?? centerPreview(viewport()),viewport()); maximized = false; }
    else { restored = {...rect}; rect = boundPreview({x:8,y:8,...viewport()},viewport()); maximized = true; }
  }
  function toggleMinimize() {
    gesture = null;
    minimized = !minimized;
    rect = boundPreview(rect,viewport(),minimized);
    titlebar?.focus();
  }
  function resizeViewport() {
    if (!visible) return;
    rect = maximized ? boundPreview({x:8,y:8,...viewport()},viewport(),minimized) : boundPreview(rect,viewport(),minimized);
    if (restored) restored = boundPreview(restored,viewport());
  }
  function beginGesture(event: PointerEvent, resize = false) {
    if (event.button !== 0 || maximized || (resize && minimized)) return;
    if (!resize && (event.target as HTMLElement).closest('button,a,input')) return;
    event.preventDefault();
    (event.currentTarget as HTMLElement).focus();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    gesture = {id:event.pointerId,x:event.clientX,y:event.clientY,rect:{...rect},resize};
  }
  function moveGesture(event: PointerEvent) {
    if (!gesture || gesture.id !== event.pointerId) return;
    const dx=event.clientX-gesture.x,dy=event.clientY-gesture.y;
    rect = gesture.resize ? resizePreview(gesture.rect,dx,dy,viewport())
      : boundPreview({...gesture.rect,x:gesture.rect.x+dx,y:gesture.rect.y+dy},viewport(),minimized);
  }
  function endGesture(event: PointerEvent) {
    if (gesture?.id !== event.pointerId) return;
    gesture = null;
    const target=event.currentTarget as HTMLElement;
    if (target.hasPointerCapture(event.pointerId)) target.releasePointerCapture(event.pointerId);
  }
  function arrowGesture(event: KeyboardEvent, resize = false) {
    if (event.target !== event.currentTarget || maximized) return;
    const step=event.shiftKey?20:5;
    const dx=event.key==='ArrowLeft'?-step:event.key==='ArrowRight'?step:0;
    const dy=event.key==='ArrowUp'?-step:event.key==='ArrowDown'?step:0;
    if (!dx&&!dy) return;
    event.preventDefault();
    rect = resize ? resizePreview(rect,dx,dy,viewport()) : boundPreview({...rect,x:rect.x+dx,y:rect.y+dy},viewport(),minimized);
  }
  $effect(()=>{const request=openRequest;if(request>0&&viewer)untrack(openViewer)});
</script>
<svelte:window onresize={resizeViewport}/>
{#if thumbnail}<button type="button" class="thumbnail" onclick={openViewer} title={$t('chat.66')}><img {src} alt={name}/></button>{/if}
<div bind:this={viewer} class="image-window" class:minimized popover="manual" role="dialog" aria-modal="false" aria-label={current.name} tabindex="-1"
  style:left={`${rect.x}px`} style:top={`${rect.y}px`} style:width={`${rect.width}px`} style:height={`${minimized?PREVIEW_HEADER_HEIGHT:rect.height}px`}
  onkeydown={viewerKeys}>
  <div bind:this={titlebar} class="window-titlebar" role="toolbar" tabindex="0" aria-label={$t('chat.previewMove')} title={$t('chat.previewMove')}
    onpointerdown={event=>beginGesture(event)} onpointermove={moveGesture} onpointerup={endGesture} onpointercancel={endGesture} onlostpointercapture={()=>gesture=null}
    onkeydown={event=>arrowGesture(event)} ondblclick={event=>{if(!(event.target as HTMLElement).closest('button,a'))toggleMaximize()}}>
    <strong>{current.name}</strong>
    <button type="button" title={$t(minimized?'Restore':'Minimize')} aria-label={$t(minimized?'Restore':'Minimize')} onclick={toggleMinimize}><Minus size={15}/></button>
    <button type="button" title={$t(maximized?'Restore':'Maximize')} aria-label={$t(maximized?'Restore':'Maximize')} onclick={toggleMaximize}>{#if maximized}<Copy size={14}/>{:else}<Square size={14}/>{/if}</button>
    <button type="button" class="close" title={$t('chat.67')} aria-label={$t('chat.67')} onclick={closeViewer}><X size={17}/></button>
  </div>
  {#if !minimized}
    <div class="controls">
      <label>{$t('chat.68')} <input type="range" min="25" max="400" step="25" bind:value={zoom}/> <output>{zoom}%</output></label>
      <button type="button" class="fit" onclick={()=>zoom=100}>{$t('chat.69')}</button>
      {#if sequence.length>1}<output class="image-count" aria-live="polite">{index+1} / {sequence.length}</output>{/if}
      {#if current.workspaceId&&current.folderId&&current.chatId&&current.path}<LocalPathLink workspaceId={current.workspaceId} folderId={current.folderId} chatId={current.chatId} path={current.path} label={$t('chat.showInFolder')}/>{/if}
    </div>
    <div class="canvas-shell">
      <div class="canvas" bind:this={canvas} aria-busy={loading}>
        {#if loading}<div class="preview-status" role="status">{$t('Loading…')}</div>
        {:else if failed}<div class="preview-status" role="alert"><span>{$t('chat.previewFailed')}</span><button type="button" onclick={()=>retry++}>{$t('Try again')}</button></div>
        {:else if displayedSrc}<img src={displayedSrc} alt={current.name} style:width={`${zoom}%`} style:height={`${zoom}%`} draggable="false" onerror={()=>failed=true}/>{/if}
      </div>
      {#if sequence.length>1}
        <button type="button" class="image-nav previous" title={$t('chat.previousImage')} aria-label={$t('chat.previousImage')} disabled={index===0} onclick={()=>navigate(-1)}><ChevronLeft size={26}/></button>
        <button type="button" class="image-nav next" title={$t('chat.nextImage')} aria-label={$t('chat.nextImage')} disabled={index===sequence.length-1} onclick={()=>navigate(1)}><ChevronRight size={26}/></button>
      {/if}
    </div>
    {#if !maximized}<button type="button" class="resize-grip" aria-label={$t('chat.previewResize')} title={$t('chat.previewResize')}
      onpointerdown={event=>beginGesture(event,true)} onpointermove={moveGesture} onpointerup={endGesture} onpointercancel={endGesture} onlostpointercapture={()=>gesture=null}
      onkeydown={event=>arrowGesture(event,true)}><Grip size={15}/></button>{/if}
  {/if}
</div>
<style>
.thumbnail{width:100%;text-align:left;cursor:zoom-in}.thumbnail img{max-width:100%;max-height:300px;object-fit:contain;border-radius:7px}
.image-window{position:fixed;inset:auto;margin:0;padding:0;box-sizing:border-box;max-width:calc(100vw - 16px);max-height:calc(100dvh - 16px);overflow:hidden;border:1px solid var(--color-border);border-radius:12px;background:rgb(var(--glass-base,24 24 24) / .98);color:var(--color-text);box-shadow:0 20px 70px #0009;font-size:12px;text-align:left}
.image-window:popover-open{display:flex;flex-direction:column}.image-window::backdrop{background:transparent;pointer-events:none}.image-window:focus{outline:none}
.window-titlebar{display:flex;align-items:center;flex:none;height:40px;min-height:40px;padding:0 4px 0 13px;border-bottom:1px solid var(--color-border);cursor:move;touch-action:none;user-select:none;background:var(--card-bg)}
.window-titlebar strong{flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;margin-right:10px;font-weight:500}.window-titlebar button{width:32px;height:30px;display:grid;place-items:center;flex:none;border-radius:5px}.window-titlebar button:hover{background:var(--surface-hover)}.window-titlebar .close:hover{background:#bd3c3c;color:white}
.controls{display:flex;flex-wrap:wrap;align-items:center;gap:8px 14px;padding:9px 13px;border-bottom:1px solid var(--color-border);flex:none;font-size:11px}.controls label{display:flex;align-items:center;gap:8px}.controls input{width:clamp(65px,10vw,140px)}.controls output{min-width:34px;font-variant-numeric:tabular-nums}.fit{padding:4px 6px;border-radius:5px}.fit:hover{background:var(--surface-hover)}
.canvas-shell{position:relative;display:flex;flex:1;min-height:0}.canvas{overflow:auto;flex:1;min-width:0;min-height:0;background:repeating-conic-gradient(#8882 0% 25%,#8881 0% 50%) 50%/20px 20px}.canvas img{display:block;object-fit:contain;max-width:none;max-height:none;margin:auto}
.image-nav{position:absolute;top:50%;transform:translateY(-50%);display:grid;place-items:center;width:44px;height:52px;border:1px solid #ffffff45;border-radius:12px;background:#171717c9;color:white;box-shadow:0 2px 10px #0004}.image-nav:hover:not(:disabled){background:#333e}.image-nav:disabled{opacity:.28;cursor:default}.previous{left:8px}.next{right:8px}.preview-status{height:100%;display:flex;gap:12px;align-items:center;justify-content:center;flex-direction:column;padding:20px 60px;text-align:center}.preview-status button{padding:8px 12px;border:1px solid var(--color-border);border-radius:7px}.image-count{white-space:nowrap}
.resize-grip{position:absolute;right:0;bottom:0;display:grid;place-items:center;width:24px;height:24px;border-radius:7px 0 0 0;background:var(--card-bg);color:var(--color-text-muted);cursor:nwse-resize;touch-action:none}
button{cursor:pointer}button:focus-visible,.window-titlebar:focus-visible{outline:2px solid var(--primary);outline-offset:-2px}.minimized .window-titlebar{border-bottom:0}
</style>
