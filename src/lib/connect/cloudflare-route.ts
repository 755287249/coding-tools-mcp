/**
 * Values for Cloudflare's "Add published application" route dialog
 * (Networking › Tunnels › tunnel › Routes › Add route › Published application).
 *
 * A token-based (remotely managed) tunnel takes its ingress from the dashboard,
 * so the route there must point at this workspace's local MCP port.
 */

export const CLOUDFLARE_TUNNELS_URL = "https://dash.cloudflare.com/?to=/:account/tunnels";

export interface HostnameSplit {
  /** Cloudflare "Subdomain" field; empty for the zone apex. */
  subdomain: string;
  /** Cloudflare "Domain" drop-down value (the zone). */
  zone: string;
}

const LABEL = /^(?!-)[a-z0-9-]{1,63}(?<!-)$/;

/**
 * Second-level labels that are public suffixes themselves (e.g. `example.com.cn`,
 * `foo.co.uk`) plus popular free-domain suffixes, where the zone keeps 3 labels.
 */
const MULTI_LABEL_SUFFIXES = new Set([
  "com", "net", "org", "edu", "gov", "mil", "int", "co", "ac", "or", "ne", "go", "gob", "nom", "ltd", "plc",
  "sch", "info", "biz",
]);
const FREE_DOMAIN_SUFFIXES = new Set([
  "cc.cd", "us.kg", "xx.kg", "dpdns.org", "qzz.io", "eu.org", "pp.ua", "ggff.net", "nyc.mn", "ccwu.cc", "de5.net",
]);

/** Accepts `mcp.example.com`, `https://mcp.example.com/mcp`, … and returns the bare hostname. */
export function routeHostname(value: string): string | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  let host: string;
  try {
    host = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`).hostname;
  } catch {
    return null;
  }
  host = host.toLowerCase().replace(/\.$/, "");
  const labels = host.split(".");
  if (labels.length < 2 || !labels.every((label) => LABEL.test(label))) return null;
  if (/^\d+$/.test(labels[labels.length - 1])) return null; // IPv4 literal
  return host;
}

/** Every possible subdomain / zone split, shortest zone first. */
export function hostnameSplits(host: string): HostnameSplit[] {
  const labels = host.split(".");
  const splits: HostnameSplit[] = [];
  for (let zoneLabels = 2; zoneLabels <= labels.length; zoneLabels += 1) {
    splits.push({
      subdomain: labels.slice(0, labels.length - zoneLabels).join("."),
      zone: labels.slice(labels.length - zoneLabels).join("."),
    });
  }
  return splits;
}

/** Best guess of the Cloudflare zone; the user can override it in the guide. */
export function guessZone(host: string): string {
  const labels = host.split(".");
  if (labels.length <= 2) return host;
  const lastTwo = labels.slice(-2).join(".");
  const tld = labels[labels.length - 1];
  const second = labels[labels.length - 2];
  const threeLabels =
    FREE_DOMAIN_SUFFIXES.has(lastTwo) || (tld.length === 2 && MULTI_LABEL_SUFFIXES.has(second));
  return threeLabels ? labels.slice(-3).join(".") : lastTwo;
}

export function splitForZone(host: string, zone: string): HostnameSplit {
  const match = hostnameSplits(host).find((split) => split.zone === zone);
  return match ?? { subdomain: "", zone: host };
}

/**
 * Service URL for the route. Uses an IP literal rather than `localhost`, which can
 * resolve to ::1 first while the MCP server listens on IPv4 only.
 */
export function routeServiceUrl(port: number, bindAddress?: string): string {
  const bind = (bindAddress ?? "").trim().replace(/^\[|\]$/g, "");
  let host = "127.0.0.1";
  if (bind === "::" || bind === "::0" || bind === "::1") host = "[::1]";
  else if (bind.includes(":")) host = `[${bind}]`;
  else if (bind && bind !== "0.0.0.0") host = bind;
  return `http://${host}:${port}`;
}
