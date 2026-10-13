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
 * One short prompt for both channels ([接入AI] and seeds); only the address differs.
 * It hands over just the MCP URL. Rules and tools are explained by the server itself:
 * MCP clients get them from initialize/tools/list/skill.text, and a plain GET on the
 * same URL returns a guide page whose complete next-step URLs fetch-only AIs can open.
 * "The tools are inside the MCP, not your client" stops "I have no such tools" replies.
 */
export function connectPrompt(endpoint:string,_kind:'direct'|'seed'):string {
  return `请连接我的工具 ${endpoint} 。快速连接这个 sandbox 的 MCP（URL），明确使用规则，熟悉可用工具，做好处理接下来一系列操作的准备。工具在 MCP 内部，而非你的客户端内部，连上后使用，然后等待信息。`;
}
/** [接入AI] by URL: the address carries a credential bound to this chat. */
export function directConnectPrompt(address:string,folder:string,seed:SeedTicket):string {
  if(!seed.url_key||!/^[a-f0-9]{64}$/.test(seed.url_key)||seed.expires_at<=Date.now())throw Error('Invalid or expired direct connect URL');
  const pairing=pairingAddress(address,folder),endpoint=pairing.slice(0,pairing.lastIndexOf('/seed-library/'))+'/seeds/'+encodeURIComponent(folder)+'/'+seed.url_key;
  return connectPrompt(endpoint,'direct');
}
