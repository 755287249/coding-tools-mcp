import {pairingAddress} from './seed-pairing';
import type {SeedTicket} from '$lib/api/seeds';
/** Manual enrollment is a folder/seed capability, never a general OAuth credential. */
export function seedEnrollmentPrompt(address:string,folder:string,seed:SeedTicket):string {
  if(!/^[A-Za-z0-9_-]{1,80}$/.test(seed.seed_id)||!/^[a-f0-9]{64}$/.test(seed.ticket)||seed.expires_at<=Date.now())throw Error('Invalid or expired seed enrollment');
  const pairing=pairingAddress(address,folder),endpoint=pairing.slice(0,pairing.lastIndexOf('/seed-library/'))+'/seeds/'+encodeURIComponent(folder)+'/'+(seed.url_key??seed.seed_id);
  if(seed.url_key!==undefined){
    if(!/^[a-f0-9]{64}$/.test(seed.url_key))throw Error('Invalid seed URL');
    return connectPrompt(endpoint,'seed');
  }
  return `接入当前项目的种子库，保持待命，分配后继续该本地对话。\n来源：${seed.source||'custom'}；备注前缀：${seed.prefix||'未分类'}\nMCP 接入地址：${endpoint}\n一次性入库票据：${seed.ticket}\n票据仅存 HTTP 客户端内存，不写文件、日志或回复。使用 User-Agent: Coding-Tools-MCP/1.0，以 Authorization: Bearer <入库票据> POST JSON-RPC initialize；从 result._meta.seed_access_token 读取专用令牌并保存在客户端内存，后续请求使用该令牌。MCP-Protocol-Version: 2025-03-26。初始化响应丢失时一分钟内可用原票据重试。\n按顺序 notifications/initialized（无 id）→ tools/list → list_workspace_folders 核对 ${folder} → seed_wait(timeout_ms:25000)。每个外层工具调用只等待一次；idle 后继续独立等待。收到 assigned，使用返回的 chat_id 和 attachment_id 调用 chat_open，完整读取 skill.text 和记录，再立即 chat_wait。\n保持 chat_wait → 工作 → chat_reply → chat_wait；全部成果通过此 MCP 指定工作区保存。接替时检查已有计划、文件、完成回复和运行中的操作，避免重复副作用。不得改用其他种子身份；retired、权限撤销、用户明确停止或宿主执行上限时停止。`;
}

/**
 * One prompt for both channels ([接入AI] and seeds); only the address and the
 * first step differ. Written for the widest range of AIs: MCP clients, AIs that
 * can run code (HTTP POST), and AIs that can only read web pages (GET mode).
 */
export function connectPrompt(endpoint:string,kind:'direct'|'seed'):string {
  const after=kind==='direct'?'连上后 seed_connect，再 chat_wait 等我的消息':'连上后 seed_connect 入库，再 chat_wait 等分配到我的对话';
  return `${endpoint}
这是我电脑上 Coding Tools MCP 应用为我的项目生成的连接（已含认证，1 小时内首次连接）。请连上，在应用对话里和我协作：有 MCP 客户端就加为 Streamable HTTP 服务器；能运行代码就向它 POST JSON-RPC；只能读网页就直接打开它，按页面的下一步网址操作。seed_connect、chat_wait、chat_reply 和文件/命令工具都在这个服务器上，不在你的客户端里。${after}，完成后 chat_reply 回复并继续 chat_wait；idle 表示我还没发新消息。`;
}
/** [接入AI] by URL: the address carries a credential bound to this chat. */
export function directConnectPrompt(address:string,folder:string,seed:SeedTicket):string {
  if(!seed.url_key||!/^[a-f0-9]{64}$/.test(seed.url_key)||seed.expires_at<=Date.now())throw Error('Invalid or expired direct connect URL');
  const pairing=pairingAddress(address,folder),endpoint=pairing.slice(0,pairing.lastIndexOf('/seed-library/'))+'/seeds/'+encodeURIComponent(folder)+'/'+seed.url_key;
  return connectPrompt(endpoint,'direct');
}
