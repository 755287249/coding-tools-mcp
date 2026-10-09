import {artifactParts} from './artifact-links';
export interface ChatLinkPart { text:string; path?:string; href?:string; image?:boolean }
export function localPathTarget(value:string):string|null {
  let target=value.trim();
  if (/^file:/i.test(target)) {
    try {const url=new URL(target);if(url.hostname&&url.hostname!=='localhost')return null;target=decodeURIComponent(url.pathname);if(/^\/[A-Za-z]:\//.test(target))target=target.slice(1);}catch{return null}
  }
  if (!target || target.length>4096 || /[\x00-\x1f\x7f<>"|?*]/.test(target)) return null;
  const normalized=target.replace(/\\/g,'/');
  if(normalized.startsWith('//')||normalized.split('/').includes('..'))return null;
  if((/^[A-Za-z]:\//.test(normalized)?normalized.slice(2):normalized).includes(':'))return null;
  if(normalized!=='.'&&!normalized.includes('/')&&!/^[^\s]+\.[A-Za-z0-9]{1,12}$/.test(normalized))return null;
  return target;
}
function part(text:string,target:string):ChatLinkPart|null {
  if(/^https?:\/\//i.test(target)){try{const url=new URL(target);return {text,href:url.href}}catch{return null}}
  const path=localPathTarget(target);if(!path)return null;
  const image=artifactParts(path).some(token=>token.path===path);
  return {text,path,...(image?{image:true}:{})};
}
/** Labels/code delimiters preserve spaces; bare paths end at whitespace/punctuation. */
export function chatLinkParts(text:string,literal=false):ChatLinkPart[] {
  if(literal){const token=part(text,text);if(token)return [token];}
  const pattern=/\[([^\]\n]+)\]\((<[^>\n]+>|[^)\n]+)\)|(?<![\w:/\\])(?:https?:\/\/[^\s`<>"'\])，。；]+|file:\/\/\/[^\s`<>"'\])，。；]+|[A-Za-z]:[\\/][^\s`<>"'\])，。；]*|\/(?!\/)[^\s`<>"'\])，。；]+|(?:\.\/)?[A-Za-z0-9_.-]+[\\/][^\s`<>"'\])，。；]+)/gi;
  const result:ChatLinkPart[]=[];let offset=0;
  for(const match of text.matchAll(pattern)){
    let target=match[2]??match[0];if(target.startsWith('<')&&target.endsWith('>'))target=target.slice(1,-1);
    if(!match[2])target=target.replace(/[.,;!?]+$/,'');
    const token=part(match[1]??target,target);if(!token)continue;
    if(match.index>offset)result.push({text:text.slice(offset,match.index)});
    result.push(token);offset=match.index+(match[2]?match[0].length:target.length);
  }
  if(offset<text.length)result.push({text:text.slice(offset)});
  return result;
}
