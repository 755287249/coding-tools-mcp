export interface PluginPermissions { hosts?: string[]; seedLibrary?: boolean; progress?: boolean; storage?: boolean }
export interface PluginManifest { id: string; name: string; version: string; description?: string; permissions?: PluginPermissions }
export interface PluginPackage { format: 1; manifest: PluginManifest; code: string }
export interface PluginHttpRequest { url: string; method?: 'GET'|'POST'|'PUT'|'PATCH'|'DELETE'; headers?: Record<string,string>; body?: string }
export interface PluginHttpResponse { status: number; headers: Record<string,string|string[]>; body: string }
