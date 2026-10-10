/** Swipe only on the transcript surface; native editors and horizontal content keep gestures. */
export function swipeDirection(dx:number,dy:number,elapsed:number): -1|1|null {
  return elapsed<=800 && Math.abs(dx)>=70 && Math.abs(dx)>Math.abs(dy)*1.8 ? dx<0?1:-1 : null;
}
export function conversationSwipe(node:HTMLElement, begin:()=>((direction:-1|1)=>void)|null) {
  let gesture:{x:number;y:number;time:number;run:(direction:-1|1)=>void}|null=null;
  function start(event:TouchEvent) {
    gesture=null;
    if(!matchMedia('(max-width:700px)').matches || event.touches.length!==1 || document.querySelector('dialog[open]') || window.getSelection()?.toString())return;
    const touch=event.touches[0]!;
    // Preserve browser edge gestures and all interactive controls.
    if(touch.clientX<24 || touch.clientX>innerWidth-24)return;
    let target=event.target instanceof Element?event.target:null;
    if(target?.closest('input,textarea,select,button,a,[contenteditable],pre,.code-preview,.table-scroll'))return;
    while(target&&target!==node){if(target.scrollWidth>target.clientWidth+1 && /auto|scroll/.test(getComputedStyle(target).overflowX))return;target=target.parentElement}
    const run=begin();if(run)gesture={x:touch.clientX,y:touch.clientY,time:event.timeStamp,run};
  }
  function move(event:TouchEvent){if(gesture&&(event.touches.length!==1||Math.abs(event.touches[0]!.clientY-gesture.y)>40))gesture=null}
  function end(event:TouchEvent){const current=gesture;gesture=null;if(!current||event.changedTouches.length!==1||window.getSelection()?.toString())return;const touch=event.changedTouches[0]!,direction=swipeDirection(touch.clientX-current.x,touch.clientY-current.y,event.timeStamp-current.time);if(direction)current.run(direction)}
  function cancel(){gesture=null}
  node.addEventListener('touchstart',start,{passive:true});node.addEventListener('touchmove',move,{passive:true});node.addEventListener('touchend',end,{passive:true});node.addEventListener('touchcancel',cancel);
  return {update(next:typeof begin){begin=next},destroy(){node.removeEventListener('touchstart',start);node.removeEventListener('touchmove',move);node.removeEventListener('touchend',end);node.removeEventListener('touchcancel',cancel)}};
}
