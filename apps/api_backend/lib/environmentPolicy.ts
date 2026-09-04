import { isIP } from 'node:net';

export type EnvironmentSource = Readonly<Record<string, string | undefined>>;

// These values have all appeared in public repository examples or historical local bootstrap
// defaults. They are documentation markers, not credentials. Keep this list centralized so every
// startup path applies the same fail-closed policy without ever logging the supplied values.
const KNOWN_PUBLIC_EXAMPLE_CREDENTIALS = new Set([
  'dev-only-jwt-secret-change-me',
  'replace-with-enrollment-token',
  'replace_with_a_long_random_string',
  'replace_with_long_random_string',
  'replace_with_openai_api_key',
  'replace_with_shared_agent_token',
  'replace_with_shared_dev_key',
  'replace_with_shared_rmm_server_key',
  'replace_with_shared_service_key',
  'talos_dev_agent_token_01',
  'talos_dev_jwt_secret_minimum_32_chars_long_',
  'talos_dev_local_service_key_01',
  'your-secret-key',
]);

export const CREDENTIAL_ENVIRONMENT_VARIABLES = [
  'JWT_SECRET',
  'APP_ENCRYPTION_KEY',
  'SERVICE_KEY',
  'API_SERVICE_KEY',
  'RMM_SERVER_API_KEY',
  'RMM_TELEMETRY_SERVICE_KEY',
  'TALOS_AI_RUNNER_SERVICE_KEY',
  'TALOS_AI_RUNNER_RMM_SERVER_KEY',
  'RMM_AGENT_TOKEN',
  'OPENAI_API_KEY',
] as const;

export const API_REQUIRED_ENVIRONMENT_VARIABLES = [
  'JWT_SECRET',
  'APP_ENCRYPTION_KEY',
  'TOKEN_TTL',
  'MACHINE_TOKEN_TTL',
] as const;

export const COMMUNITY_REQUIRED_ENVIRONMENT_VARIABLES = [
  ...API_REQUIRED_ENVIRONMENT_VARIABLES,
  'RMM_SERVER_API_KEY',
] as const;

const EXAMPLE_PREFIX = /^(?:replace[-_ ]?with(?:[-_ ]|$)|your[-_ ].*(?:key|secret|token)$)/i;

export function isKnownPublicExampleCredential(value: unknown): boolean {
  if (typeof value !== 'string') return false;
  const normalized = value.trim().toLowerCase();
  return (
    normalized.length > 0 &&
    (KNOWN_PUBLIC_EXAMPLE_CREDENTIALS.has(normalized) || EXAMPLE_PREFIX.test(normalized))
  );
}

export function findMissingEnvironmentVariables(
  source: EnvironmentSource,
  required: readonly string[],
): string[] {
  return required.filter((name) => !source[name]?.trim()).sort();
}

export function findUnsafeCredentialVariables(source: EnvironmentSource): string[] {
  return CREDENTIAL_ENVIRONMENT_VARIABLES.filter((name) =>
    isKnownPublicExampleCredential(source[name]),
  ).sort();
}

export function assertSecureEnvironment(
  source: EnvironmentSource,
  required: readonly string[],
  context: string,
): void {
  const missing = findMissingEnvironmentVariables(source, required);
  const unsafe = findUnsafeCredentialVariables(source);
  const problems: string[] = [];

  if (missing.length > 0) {
    problems.push(`missing required variables: ${missing.join(', ')}`);
  }
  if (unsafe.length > 0) {
    problems.push(`public example credentials are still configured: ${unsafe.join(', ')}`);
  }
  const jwtSecret = source.JWT_SECRET?.trim();
  const appEncryptionKey = source.APP_ENCRYPTION_KEY?.trim();
  if (jwtSecret && appEncryptionKey && jwtSecret === appEncryptionKey) {
    problems.push('APP_ENCRYPTION_KEY must be independent from JWT_SECRET');
  }
  if (problems.length > 0) {
    throw new Error(
      `${context} configuration is unsafe (${problems.join('; ')}). ` +
        'Generate unique credentials for this installation; credential values were not logged.',
    );
  }
}

const NAMED_PROXY_RANGES = new Set(['loopback', 'linklocal', 'uniquelocal']);

function isValidProxyAddress(value: string): boolean {
  if (NAMED_PROXY_RANGES.has(value.toLowerCase())) return true;

  const slashIndex = value.lastIndexOf('/');
  const address = slashIndex === -1 ? value : value.slice(0, slashIndex);
  const prefix = slashIndex === -1 ? undefined : value.slice(slashIndex + 1);
  const family = isIP(address);
  if (family === 0) return false;
  if (prefix === undefined) return true;
  if (!/^\d+$/.test(prefix)) return false;

  const prefixLength = Number(prefix);
  return prefixLength >= 0 && prefixLength <= (family === 4 ? 32 : 128);
}

/**
 * Parse an explicit proxy address/CIDR allowlist for Express. The safe default is no trusted
 * proxies. Hop counts and blanket `true` are deliberately rejected because a directly reachable
 * API can otherwise let clients choose the left-most X-Forwarded-For value used by rate limits.
 */
export function parseApiTrustedProxies(raw: string | undefined): false | string[] {
  const value = raw?.trim();
  if (!value || value.toLowerCase() === 'false' || value === '0') return false;

  const proxies = Array.from(
    new Set(
      value
        .split(',')
        .map((entry) => entry.trim())
        .filter((entry) => entry.length > 0)
        .map((entry) => {
          const normalized = entry.toLowerCase();
          return NAMED_PROXY_RANGES.has(normalized) ? normalized : entry;
        }),
    ),
  );

  if (proxies.length === 0 || proxies.some((proxy) => !isValidProxyAddress(proxy))) {
    throw new Error(
      'API_TRUSTED_PROXIES must be a comma-separated allowlist of IP addresses, CIDRs, ' +
        'loopback, linklocal, or uniquelocal; blanket trust and hop counts are not accepted',
    );
  }
  return proxies;
}
