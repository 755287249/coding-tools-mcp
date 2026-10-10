/** A top-layer list stays outside composer scroll clipping, while the editor remains scrollable. */
export function anchoredChoices(node:HTMLElement,getAnchor:()=>HTMLElement|undefined) {
 let observer:ResizeObserver|undefined;let destroyed=false;
 const place=()=>{
  const anchor=getAnchor();if(!anchor)return;
  const box=anchor.getBoundingClientRect(),viewport=window.visualViewport;
  const left=viewport?.offsetLeft??0,top=viewport?.offsetTop??0;
  const width=viewport?.width??window.innerWidth,height=viewport?.height??window.innerHeight;
  const availableAbove=Math.max(0,box.top-top-16),availableBelow=Math.max(0,top+height-box.bottom-16);
  const above=availableAbove>=Math.min(160,availableBelow);
  node.style.width=`${Math.min(340,width-16)}px`;
  node.style.maxHeight=`${Math.max(40,Math.min(260,above?availableAbove:availableBelow))}px`;
  node.style.left=`${Math.max(left+8,Math.min(box.left,left+width-node.offsetWidth-8))}px`;
  node.style.top=`${Math.max(top+8,Math.min(above?box.top-node.offsetHeight-8:box.bottom+8,top+height-node.offsetHeight-8))}px`;
 };
 queueMicrotask(()=>{if(destroyed)return;node.showPopover();place();observer=new ResizeObserver(place);observer.observe(node);const anchor=getAnchor();if(anchor)observer.observe(anchor);});
 window.addEventListener('resize',place);window.addEventListener('scroll',place,true);
 window.visualViewport?.addEventListener('resize',place);window.visualViewport?.addEventListener('scroll',place);
 return {destroy(){destroyed=true;observer?.disconnect();window.removeEventListener('resize',place);window.removeEventListener('scroll',place,true);window.visualViewport?.removeEventListener('resize',place);window.visualViewport?.removeEventListener('scroll',place);if(node.matches(':popover-open'))node.hidePopover();}};
}
