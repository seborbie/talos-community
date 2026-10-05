const BINFMT_IMAGE =
  'docker.io/tonistiigi/binfmt@sha256:400a4873b838d1b89194d982c45e5fb3cda4593fbfd7e08a02e76b03b21166f0';
const BUILDKIT_IMAGE =
  'moby/buildkit@sha256:cec9f139f45e93c5c69c60f8b07cfad9f43f4ef6b6a6cd917527fea5ff2e3dea';

function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

export function communityContainerBuildInputFailures(workflow: string): string[] {
  let parsed: unknown;
  try {
    parsed = Bun.YAML.parse(workflow);
  } catch {
    return ['candidate workflow must be valid YAML'];
  }

  const images = record(record(record(parsed)?.jobs)?.images);
  if (!Array.isArray(images?.steps)) {
    return ['candidate workflow must retain the images job and its build steps'];
  }

  const failures: string[] = [];
  if (record(images.env)?.DOCKER_CONFIG !== '${{ runner.temp }}/talos-community-docker') {
    failures.push('images job must use the isolated Community Docker configuration');
  }
  const steps = images.steps.map(record);
  const acquisitions = steps.filter((step) => step?.id === 'verified-buildx');
  const acquisition = acquisitions[0];
  if (
    acquisitions.length !== 1 ||
    acquisition?.run !== 'bash scripts/install-community-buildx.sh' ||
    acquisition.shell !== 'bash' ||
    'if' in acquisition ||
    'continue-on-error' in acquisition ||
    'env' in acquisition ||
    'working-directory' in acquisition
  ) {
    failures.push('images job must unconditionally run the verified Buildx acquisition helper');
  }
  for (const [action, expected] of [
    ['docker/setup-qemu-action', { image: BINFMT_IMAGE, platforms: 'arm64' }],
    [
      'docker/setup-buildx-action',
      {
        'cache-binary': false,
        driver: 'docker-container',
        'driver-opts': 'image=' + BUILDKIT_IMAGE,
      },
    ],
  ] as const) {
    const callers = steps.filter(
      (step) => typeof step?.uses === 'string' && step.uses.startsWith(action + '@'),
    );
    if (callers.length !== 1) {
      failures.push('images job must invoke exactly one ' + action + ' step');
      continue;
    }

    const step = callers[0]!;
    if ('if' in step || 'continue-on-error' in step || 'env' in step) {
      failures.push('images job must not conditionally skip ' + action);
    }
    const inputs = record(step.with);
    if (action === 'docker/setup-buildx-action') {
      if (inputs && 'version' in inputs) {
        failures.push(
          'docker/setup-buildx-action must retain the checksum-verified existing plugin',
        );
      }
      if (!acquisition || steps.indexOf(acquisition) + 1 !== steps.indexOf(step)) {
        failures.push('verified Buildx acquisition must immediately precede builder setup');
      }
    }
    for (const [key, value] of Object.entries(expected)) {
      if (inputs?.[key] !== value) {
        failures.push(action + ' must pin ' + key + ' to ' + value);
      }
    }
  }

  return failures;
}
