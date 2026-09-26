/**
 * Mirrors `normalize_cloudflare_token` in src-tauri/src/tunnel/cloudflare.rs:
 * accepts what users copy from the Cloudflare dashboard, e.g.
 * `cloudflared.exe service install eyJ...` or `cloudflared tunnel run --token eyJ...`,
 * and returns only the token.
 */
export function normalizeCloudflareToken(raw: string): string {
  const trimmed = raw.trim();
  const words = trimmed.split(/\s+/).map(unquote).filter(Boolean);
  if (words.length <= 1) {
    const only = words[0] ?? "";
    return only.startsWith("--token=") ? unquote(only.slice("--token=".length)) : only;
  }
  for (let index = 0; index < words.length; index += 1) {
    const word = words[index];
    if (word.startsWith("--token=")) return unquote(word.slice("--token=".length));
    if (word === "--token" && words[index + 1]) return words[index + 1];
  }
  const token = [...words].reverse().find(looksLikeTunnelToken);
  return token ?? trimmed;
}

function unquote(word: string): string {
  return word.replace(/^["'`]+|["'`]+$/g, "");
}

function looksLikeTunnelToken(word: string): boolean {
  return word.length >= 40 && word.startsWith("eyJ") && /^[A-Za-z0-9+/=_-]+$/.test(word);
}

/** Tunnel id encoded in the token (`{"a","t","s"}`), or null if it is not a tunnel token. */
export function cloudflareTokenTunnelId(raw: string): string | null {
  const token = normalizeCloudflareToken(raw).replace(/=+$/, "").replace(/-/g, "+").replace(/_/g, "/");
  if (!token) return null;
  try {
    const padded = token + "=".repeat((4 - (token.length % 4)) % 4);
    const value = JSON.parse(atob(padded)) as Record<string, unknown>;
    return typeof value.t === "string" && value.t && "a" in value && "s" in value ? value.t : null;
  } catch {
    return null;
  }
}
