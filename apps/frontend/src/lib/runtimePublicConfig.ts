export type PublicEnvironment = Readonly<Record<string, string | undefined>>;

export type RuntimePublicServiceUrls = {
  apiUrl: string;
  rmmApiUrl: string | null;
};

function httpUrl(value: string | undefined, name: string, required: boolean): string | null {
  const normalized = value?.trim();
  if (!normalized) {
    if (required) throw new Error(`${name} is not configured`);
    return null;
  }

  let parsed: URL;
  try {
    parsed = new URL(normalized);
  } catch {
    throw new Error(`${name} must be an absolute HTTP(S) URL`);
  }
  if (
    (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') ||
    !parsed.hostname ||
    parsed.username ||
    parsed.password ||
    parsed.hash
  ) {
    throw new Error(`${name} must be an absolute HTTP(S) URL without credentials or a fragment`);
  }
  return normalized;
}

/** Resolve public service endpoints at adapter runtime so one released image works on any domain. */
export function resolveRuntimePublicServiceUrls(
  environment: PublicEnvironment,
): RuntimePublicServiceUrls {
  return {
    apiUrl: httpUrl(environment.PUBLIC_API_URL, 'PUBLIC_API_URL', true) as string,
    rmmApiUrl: httpUrl(environment.PUBLIC_RMM_API_URL, 'PUBLIC_RMM_API_URL', false),
  };
}
