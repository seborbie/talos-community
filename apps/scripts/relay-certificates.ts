import { stat } from 'node:fs/promises';
import { isAbsolute, resolve } from 'node:path';
import { isAbsolute as isAbsolutePosix, relative as relativePosix } from 'node:path/posix';

export const RELAY_CERTIFICATE_MOUNT = '/.certs';
export const DEFAULT_RELAY_CERTIFICATE_PATH = '/.certs/local-dev-relay-fullchain.pem';
export const DEFAULT_RELAY_KEY_PATH = '/.certs/local-dev-relay-key.pem';
export const DEFAULT_RELAY_CERTIFICATE_HOST_SOURCE = '../apps/certs/local-dev-relay-fullchain.pem';
export const DEFAULT_RELAY_KEY_HOST_SOURCE = '../apps/certs/local-dev-relay-key.pem';

export type RelayCertificateFiles = {
  certificateFile: string;
  keyFile: string;
};

type Environment = Readonly<Record<string, string | undefined>>;
type FileExists = (path: string) => boolean | Promise<boolean>;

function optionalTrimmed(value: string | undefined): string | undefined {
  const trimmed = value?.trim();
  return trimmed ? trimmed : undefined;
}

function validateContainerFile(containerPath: string, setting: string): void {
  const relativePath = relativePosix(RELAY_CERTIFICATE_MOUNT, containerPath);
  if (
    !relativePath ||
    relativePath === '..' ||
    relativePath.startsWith('../') ||
    isAbsolutePosix(relativePath)
  ) {
    throw new Error(
      `${setting} must name a file below ${RELAY_CERTIFICATE_MOUNT}/ because Compose mounts only that exact relay TLS file`,
    );
  }
}

function resolveComposeHostFile(
  configuredPath: string | undefined,
  defaultSource: string,
  repoRoot: string,
): string {
  const source = optionalTrimmed(configuredPath) ?? defaultSource;
  return isAbsolute(source) ? resolve(source) : resolve(repoRoot, 'infra', source);
}

export function resolveRelayCertificateFiles(
  environment: Environment,
  repoRoot: string,
): RelayCertificateFiles {
  const certificatePath =
    optionalTrimmed(environment.RMM_RELAY_TLS_CERT_PATH) ?? DEFAULT_RELAY_CERTIFICATE_PATH;
  const keyPath = optionalTrimmed(environment.RMM_RELAY_TLS_KEY_PATH) ?? DEFAULT_RELAY_KEY_PATH;
  validateContainerFile(certificatePath, 'RMM_RELAY_TLS_CERT_PATH');
  validateContainerFile(keyPath, 'RMM_RELAY_TLS_KEY_PATH');

  return {
    certificateFile: resolveComposeHostFile(
      environment.RMM_RELAY_TLS_CERT_HOST_PATH,
      DEFAULT_RELAY_CERTIFICATE_HOST_SOURCE,
      repoRoot,
    ),
    keyFile: resolveComposeHostFile(
      environment.RMM_RELAY_TLS_KEY_HOST_PATH,
      DEFAULT_RELAY_KEY_HOST_SOURCE,
      repoRoot,
    ),
  };
}

async function isRegularFile(path: string): Promise<boolean> {
  try {
    return (await stat(path)).isFile();
  } catch {
    return false;
  }
}

export async function requireRelayCertificates(
  environment: Environment,
  repoRoot: string,
  exists: FileExists = isRegularFile,
): Promise<RelayCertificateFiles> {
  const files = resolveRelayCertificateFiles(environment, repoRoot);
  const [certificateExists, keyExists] = await Promise.all([
    exists(files.certificateFile),
    exists(files.keyFile),
  ]);
  const missing = [
    certificateExists ? undefined : files.certificateFile,
    keyExists ? undefined : files.keyFile,
  ].filter((path): path is string => path !== undefined);

  if (missing.length > 0) {
    throw new Error(
      [
        'Relay TLS preflight failed; required host files are missing:',
        ...missing.map((path) => `- ${path}`),
        'Provide a certificate chain and private key matching RMM_RELAY_URL, or set RMM_RELAY_TLS_CERT_HOST_PATH and RMM_RELAY_TLS_KEY_HOST_PATH to exact host files.',
        'See apps/certs/README.md.',
      ].join('\n'),
    );
  }

  return files;
}
