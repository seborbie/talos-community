import { isIP } from 'node:net';
import type { Request } from 'express';

function cleanString(value: unknown): string | null {
  if (typeof value !== 'string') return null;
  const trimmed = value.trim();
  return trimmed ? trimmed : null;
}

function normalizeConfiguredPublicBaseUrl(value: string, settingName: string): string {
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error(`${settingName} must be an absolute HTTP(S) URL`);
  }

  if (
    (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') ||
    parsed.username ||
    parsed.password ||
    parsed.search ||
    parsed.hash
  ) {
    throw new Error(
      `${settingName} must be an absolute HTTP(S) URL without credentials, query, or fragment`,
    );
  }

  return parsed.toString().replace(/\/+$/, '');
}

function isLoopbackHostname(hostname: string): boolean {
  const normalized = hostname.replace(/^\[|\]$/g, '').toLowerCase();
  if (normalized === 'localhost' || normalized === '::1') return true;
  return isIP(normalized) === 4 && normalized.startsWith('127.');
}

function formatHostname(hostname: string): string {
  if (hostname.startsWith('[') && hostname.endsWith(']')) return hostname;
  return isIP(hostname) === 6 ? `[${hostname}]` : hostname;
}

/**
 * Resolve the client address through Express's configured `trust proxy` policy. Callers must not
 * inspect X-Forwarded-For themselves because that would bypass the address/CIDR allowlist.
 */
export function getTrustedClientIp(req: Request): string | null {
  return cleanString(req.ip) || cleanString(req.socket?.remoteAddress);
}

/**
 * Resolve a public base URL from explicit deployment configuration first, then from Express's
 * proxy-aware protocol and hostname accessors. A non-default loopback port is retained for direct
 * local development. Reverse-proxy and non-loopback deployments should always configure the
 * public URL because the external port/path cannot be inferred safely from the backend socket.
 */
export function getTrustedRequestOrigin(
  req: Request,
  configuredBaseUrl?: string | null,
  settingName = 'public base URL',
): string {
  const configured = cleanString(configuredBaseUrl);
  if (configured) {
    return normalizeConfiguredPublicBaseUrl(configured, settingName);
  }

  const protocol = cleanString(req.protocol)?.toLowerCase();
  if (protocol !== 'http' && protocol !== 'https') {
    throw new Error('Unable to determine a trusted HTTP(S) request protocol');
  }

  const hostname = cleanString(req.hostname);
  if (!hostname) {
    throw new Error('Unable to determine a trusted request hostname');
  }

  const localPort = req.socket?.localPort;
  const useLocalPort =
    isLoopbackHostname(hostname) &&
    typeof localPort === 'number' &&
    Number.isInteger(localPort) &&
    localPort > 0 &&
    localPort <= 65_535 &&
    !((protocol === 'http' && localPort === 80) || (protocol === 'https' && localPort === 443));
  const candidate =
    `${protocol}://${formatHostname(hostname)}` + (useLocalPort ? `:${localPort}` : '');

  let parsed: URL;
  try {
    parsed = new URL(candidate);
  } catch {
    throw new Error('Unable to determine a trusted request origin');
  }
  if (
    parsed.pathname !== '/' ||
    parsed.search ||
    parsed.hash ||
    parsed.username ||
    parsed.password
  ) {
    throw new Error('Unable to determine a trusted request origin');
  }
  return parsed.origin;
}
