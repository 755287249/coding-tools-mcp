/** Open toward available inner space, then clamp both axes to the viewport. */
export function innerPopover(anchor: {left:number;right:number;top:number;bottom:number}, size:{width:number;height:number}, viewport:{width:number;height:number}) {
  const gap=8, width=Math.min(size.width,Math.max(0,viewport.width-2*gap)),height=Math.min(size.height,Math.max(0,viewport.height-2*gap));
  const right=anchor.right+gap;
  const left=right+width<=viewport.width-gap ? right : anchor.left-width-gap>=gap ? anchor.left-width-gap : Math.max(gap,viewport.width-width-gap);
  const top=anchor.top+height<=viewport.height-gap ? anchor.top : Math.max(gap,anchor.bottom-height);
  return {left:Math.max(gap,Math.min(left,viewport.width-width-gap)),top:Math.max(gap,Math.min(top,viewport.height-height-gap))};
}
