import { resolve } from 'path';

const appsDir = resolve(import.meta.dir, '..');
const envPath = resolve(appsDir, '.env');
export const DEV_RELAY_URL = 'localhost:17443';

export function ensureRelayUrlContents(
  original: string,
  relayUrl = DEV_RELAY_URL,
): { contents: string; changed: boolean; configuredUrl: string } {
  const normalized = original.replace(/\r\n/g, '\n');
  const lines = normalized.length > 0 ? normalized.split('\n') : [];
  let found = false;
  let configuredUrl = relayUrl;

  const updatedLines = lines.map((line) => {
    const match = line.match(/^\s*RMM_RELAY_URL\s*=\s*(.*?)\s*$/);
    if (!match) {
      return line;
    }
    found = true;
    const existing = match[1]?.trim();
    if (existing) {
      configuredUrl = existing;
      return line;
    }
    return `RMM_RELAY_URL=${relayUrl}`;
  });

  if (!found) {
    if (updatedLines.length > 0 && updatedLines[updatedLines.length - 1] !== '') {
      updatedLines.push('');
    }
    updatedLines.push(`RMM_RELAY_URL=${relayUrl}`);
  }

  const contents = updatedLines.join('\n');
  return { contents, changed: contents !== normalized, configuredUrl };
}

export async function ensureRelayUrl(relayUrl = DEV_RELAY_URL): Promise<boolean> {
  const original = await Bun.file(envPath)
    .text()
    .catch(() => '');
  const result = ensureRelayUrlContents(original, relayUrl);
  if (!result.changed) {
    console.log(`Relay URL already set: RMM_RELAY_URL=${result.configuredUrl}`);
    return false;
  }

  await Bun.write(envPath, result.contents);
  console.log(`Relay URL set: RMM_RELAY_URL=${result.configuredUrl}`);
  return true;
}

if (import.meta.main) {
  try {
    await ensureRelayUrl();
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
