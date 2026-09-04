export type OperatorContent = {
  name: string | null;
  supportUrl: string | null;
  termsUrl: string | null;
  privacyUrl: string | null;
  sourceUrl: string | null;
};

type PublicEnvironment = Readonly<Record<string, string | undefined>>;

const MAX_OPERATOR_NAME_LENGTH = 160;
const MAX_OPERATOR_URL_LENGTH = 2_048;

function optionalText(value: string | undefined, maxLength: number): string | null {
  const normalized = value?.trim();
  if (!normalized || normalized.length > maxLength) return null;
  return normalized;
}

export function safeOperatorUrl(value: string | undefined): string | null {
  const normalized = optionalText(value, MAX_OPERATOR_URL_LENGTH);
  if (!normalized) return null;

  try {
    const parsed = new URL(normalized);
    if (
      (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') ||
      !parsed.hostname ||
      parsed.username ||
      parsed.password
    ) {
      return null;
    }
    return parsed.toString();
  } catch {
    return null;
  }
}

export function resolveOperatorContent(environment: PublicEnvironment): OperatorContent {
  return {
    name: optionalText(environment.PUBLIC_OPERATOR_NAME, MAX_OPERATOR_NAME_LENGTH),
    supportUrl: safeOperatorUrl(environment.PUBLIC_SUPPORT_URL),
    termsUrl: safeOperatorUrl(environment.PUBLIC_TERMS_URL),
    privacyUrl: safeOperatorUrl(environment.PUBLIC_PRIVACY_URL),
    sourceUrl:
      safeOperatorUrl(environment.PUBLIC_SOURCE_URL) ??
      'https://github.com/seborbie/talos-community',
  };
}
