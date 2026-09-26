import { getBackend } from "$lib/backend";

export interface FrpProfileDto {
  id: string;
  name: string;
  server: string;
  serverPort: number;
  hasToken: boolean;
}

export interface FrpProfileInput {
  id: string;
  name: string;
  server: string;
  serverPort: number;
}

export async function listFrpProfiles(): Promise<FrpProfileDto[]> {
  return getBackend().settings.listFrpProfiles();
}

export async function saveFrpProfile(
  profile: FrpProfileInput,
  token?: string,
): Promise<FrpProfileDto> {
  return getBackend().settings.saveFrpProfile(profile, token);
}

export async function getLastWorkspaceId(): Promise<string> {
  return getBackend().settings.getLastWorkspaceId();
}

export async function setLastWorkspace(id: string): Promise<void> {
  return getBackend().settings.setLastWorkspace(id);
}

export async function deleteFrpProfile(id: string): Promise<void> {
  return getBackend().settings.deleteFrpProfile(id);
}

export interface ProxyConfigDto {
  mode: string;
  url: string;
  /** cloudflared transport: "http2" (default, TUN/VPN friendly) | "auto" | "quic". */
  tunnelProtocol?: string;
}

export async function getProxy(): Promise<ProxyConfigDto> {
  return getBackend().settings.getProxy();
}

export async function setProxy(proxy: ProxyConfigDto): Promise<void> {
  return getBackend().settings.setProxy(proxy);
}
