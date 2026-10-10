import type { ChatFile } from '../api/chat';

export const MAX_ASSET_IMAGE_BYTES = 4 * 1024 * 1024;
export const MAX_ASSET_TEXT_BYTES = 256 * 1024;
const imageTypes: Record<string,string> = {png:'image/png',jpg:'image/jpeg',jpeg:'image/jpeg',gif:'image/gif',webp:'image/webp',avif:'image/avif',bmp:'image/bmp'};
export function assetPreview(file: ChatFile, registered: boolean): {kind:'image';mime:string} | {kind:'text'} | null {
  const extension=(file.name.split('.').at(-1)??'').toLowerCase();
  const mime=Object.values(imageTypes).includes(file.mime)?file.mime:imageTypes[extension];
  if(mime && (!registered || file.size<=MAX_ASSET_IMAGE_BYTES))return {kind:'image',mime};
  if(registered && file.size<=MAX_ASSET_TEXT_BYTES && /^(md|txt|log|csv|html?|svg|[cm]?js|jsx|tsx?|json|css|ya?ml|toml|xml|py|rs|sh|sql)$/.test(extension))return {kind:'text'};
  return null;
}
