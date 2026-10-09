export interface ArtifactPart { text:string; path?:string }
/** Only explicit workspace artifact images become actions; no HTML or URLs execute. */
export function artifactParts(text:string):ArtifactPart[] {
  const pattern=/\[([^\]\n]+)\]\((mcp-assistant\/artifacts\/[^\n)]+\.(?:png|jpe?g|gif|webp))\)|mcp-assistant\/artifacts\/[^\s`<>"'\])]+?\.(?:png|jpe?g|gif|webp)(?!\.[A-Za-z0-9_])(?=$|[\s`<>"'\]),;.!?，。；])/gi;
  const result:ArtifactPart[]=[];let offset=0;
  for(const match of text.matchAll(pattern)){
    const path=match[2]??match[0];
    if(path.split('/').some(part=>!part||part==='.'||part==='..')||/[\\:\x00-\x1f\x7f]/.test(path))continue;
    if(match.index>offset)result.push({text:text.slice(offset,match.index)});
    result.push({text:match[1]??path,path});offset=match.index+match[0].length;
  }
  if(offset<text.length)result.push({text:text.slice(offset)});
  return result;
}
