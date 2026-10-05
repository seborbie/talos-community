import { describe, expect, test } from 'bun:test';
import { communityContainerBuildInputFailures } from './community-container-build-input-contract';

const candidate = await Bun.file(
  new URL('../../.github/workflows/community-release-candidate.yml', import.meta.url),
).text();

type Step = {
  id?: string;
  uses?: string;
  with?: Record<string, unknown>;
  if?: unknown;
  run?: string;
};
type Workflow = { jobs: Record<string, { steps: Step[]; env?: Record<string, unknown> }> };

function changed(mutator: (steps: Step[]) => void): string {
  const workflow = Bun.YAML.parse(candidate) as Workflow;
  mutator(workflow.jobs.images!.steps);
  return JSON.stringify(workflow);
}

function action(steps: Step[], name: string): Step {
  return steps.find((step) => step.uses?.startsWith(name + '@'))!;
}

describe('Community container build inputs', () => {
  test('the actual image-build inputs meet the pin contract', () => {
    expect(communityContainerBuildInputFailures(candidate)).toEqual([]);
  });

  test('rejects the former implicit emulator, builder and BuildKit defaults', () => {
    const floating = changed((steps) => {
      delete action(steps, 'docker/setup-qemu-action').with;
      delete action(steps, 'docker/setup-buildx-action').with;
    });
    expect(communityContainerBuildInputFailures(floating)).toHaveLength(5);
  });

  test('rejects mutable tags, floating versions and an unreviewed emulator scope', () => {
    for (const [name, key, value] of [
      ['docker/setup-qemu-action', 'image', 'docker.io/tonistiigi/binfmt:latest'],
      ['docker/setup-qemu-action', 'platforms', 'all'],
      ['docker/setup-buildx-action', 'cache-binary', true],
      ['docker/setup-buildx-action', 'driver', 'remote'],
      ['docker/setup-buildx-action', 'driver-opts', 'image=moby/buildkit:latest'],
    ] as const) {
      const failures = communityContainerBuildInputFailures(
        changed((steps) => {
          const step = action(steps, name!);
          step.with = { ...step.with, [key!]: value };
        }),
      );
      expect(failures).toHaveLength(1);
      expect(failures[0]).toStartWith(name + ' must pin ' + key);
    }
  });

  test('rejects action downloads and skipped or displaced acquisition guards', () => {
    expect(
      communityContainerBuildInputFailures(
        changed((steps) => {
          action(steps, 'docker/setup-buildx-action').with!.version = 'v0.37.2';
        }),
      ),
    ).toContain('docker/setup-buildx-action must retain the checksum-verified existing plugin');
    for (const mutation of [
      (steps: Step[]) => {
        steps.find((step) => step.id === 'verified-buildx')!.if = false;
      },
      (steps: Step[]) => {
        steps.unshift(
          steps.splice(
            steps.findIndex((step) => step.id === 'verified-buildx'),
            1,
          )[0]!,
        );
      },
    ]) {
      expect(communityContainerBuildInputFailures(changed(mutation)).length).toBeGreaterThan(0);
    }
    const workflow = Bun.YAML.parse(candidate) as Workflow;
    delete workflow.jobs.images!.env;
    expect(communityContainerBuildInputFailures(JSON.stringify(workflow))).toContain(
      'images job must use the isolated Community Docker configuration',
    );
  });

  test('a run-script containing the pins cannot replace the actual action', () => {
    const missing = changed((steps) => {
      steps.splice(steps.indexOf(action(steps, 'docker/setup-qemu-action')), 1);
      steps.push({ run: candidate });
    });
    expect(communityContainerBuildInputFailures(missing)).toContain(
      'images job must invoke exactly one docker/setup-qemu-action step',
    );
  });

  test('rejects duplicate or conditional setup steps', () => {
    expect(
      communityContainerBuildInputFailures(
        changed((steps) => steps.push(action(steps, 'docker/setup-qemu-action'))),
      ),
    ).toContain('images job must invoke exactly one docker/setup-qemu-action step');
    expect(
      communityContainerBuildInputFailures(
        changed((steps) => {
          action(steps, 'docker/setup-buildx-action').if = false;
        }),
      ),
    ).toContain('images job must not conditionally skip docker/setup-buildx-action');
  });

  test('rejects invalid YAML or moving the pinned steps outside the images job', () => {
    expect(communityContainerBuildInputFailures('invalid: yaml: value:')).toEqual([
      'candidate workflow must be valid YAML',
    ]);
    const workflow = Bun.YAML.parse(candidate) as Workflow;
    workflow.jobs.decoy = workflow.jobs.images!;
    delete workflow.jobs.images;
    expect(communityContainerBuildInputFailures(JSON.stringify(workflow))).toEqual([
      'candidate workflow must retain the images job and its build steps',
    ]);
  });
});
