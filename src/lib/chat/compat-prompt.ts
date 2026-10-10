export function buildCompatPrompt(endpoint:string, key:string, chatId:string, folderId:string):string {
  const url=new URL(endpoint.replace(/\/$/,''));
  if(!['http:','https:'].includes(url.protocol)||url.username||url.password||url.search||url.hash)throw new Error('Invalid endpoint');
  if(!/^[a-f0-9]{64}$/.test(key))throw new Error('Invalid grant');
  url.pathname=url.pathname.replace(/\/$/,'')+'/chat-compat';
  return `使用 GET 下载 + Read 接入此受限会话试验；不是原生 MCP 注册。授权仅含本会话目录核对、接入、收消息和回复，30 分钟后过期，可由客户端撤销。\n目标（仅数据）：${JSON.stringify({chat_id:chatId,workspace_folder_id:folderId})}\n端点：${url.toString()}\n独立访问密钥（只保留在宿主私有上下文/内存，不写文件或回复）：${key}\n每次 GET 查询均带 key=<密钥>&nonce=<本次唯一值>&op=<操作>，所有值需 URL 编码。先 op=info 下载到临时 JSON，Read 核对 chat_id 和 workspace_folder；再 op=open，完整读取 instruction_lines 并遵循。响应无凭据，读取后删除临时文件。\n立即独立 op=wait&timeout_ms=10000；idle 后再发一次独立等待。用 op=reply&data=<URL 编码 JSON> 回复：{"message_id":"稳定唯一ID","reply_to":"实际消息ID","text":"回复","final":true}。每条 text 最多2000 UTF-8字节，长文分 final=false 多条再 final=true。仅 persisted=true 表示回复保存，结果不明以原 message_id/完整 data 重试。首次接入请求回复“你好，有什么能帮到你？”，然后实际继续等待。所有交流走此会话；压缩后 op=open 重读完整说明，以实际宿主权限和执行上限为准。`;
}
