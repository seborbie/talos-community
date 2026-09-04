#!/usr/bin/env bun

import { createHash } from 'node:crypto';
import { chmod, lstat, mkdir, readFile, realpath, writeFile } from 'node:fs/promises';
import { basename, dirname, extname, isAbsolute, relative, resolve, sep } from 'node:path';

export const PUBLIC_EXPORT_MANIFEST = '.talos-export-manifest.json';

export type PublicExportGate = {
  id: string;
  reason: string;
};

export type PublicExportBlocker = PublicExportGate & {
  patterns: string[];
};

export type PermittedBinaryFile = {
  path: string;
  sha256: string;
  provenance: string;
};

export type PublicExportPolicy = {
  schemaVersion: 1;
  name: string;
  maximumFileBytes: number;
  externalGates: PublicExportGate[];
  requiredFiles: string[];
  includePatterns: string[];
  excludePatterns: string[];
  blockedOmissions: PublicExportBlocker[];
  forbiddenBinaryExtensions: string[];
  permittedBinaryFiles: PermittedBinaryFile[];
};

export type ExportedSourceFile = {
  path: string;
  mode: '100644' | '100755';
  size: number;
  sha256: string;
};

export type PublicExportManifest = {
  schemaVersion: 1;
  policy: {
    name: string;
    path: string;
    sha256: string;
  };
  source: {
    headCommit: string;
    headTree: string;
    clean: boolean;
    gitStatusSha256: string;
    candidateSetSha256: string;
  };
  snapshot: {
    publicationReady: boolean;
    fileCount: number;
    totalBytes: number;
    contentTreeSha256: string;
  };
  readinessFailures: PublicExportGate[];
  omittedBlockers: Array<PublicExportGate & { paths: string[] }>;
  files: ExportedSourceFile[];
};

export type PublicExportPlan = {
  manifest: PublicExportManifest;
  sourceFiles: Array<ExportedSourceFile & { absolutePath: string; bytes: Uint8Array }>;
};

export type PublicExportOptions = {
  repoRoot: string;
  policyPath: string;
  outputDirectory?: string;
  allowIncomplete: boolean;
  write: boolean;
};

type GitSourceState = {
  headCommit: string;
  headTree: string;
  clean: boolean;
  statusBytes: Uint8Array;
  candidatePaths: string[];
  indexModes: Map<string, string>;
};

const SHA256_PATTERN = /^[0-9a-f]{64}$/;
const GIT_OBJECT_PATTERN = /^[0-9a-f]{40}$/;
const SAFE_GATE_ID_PATTERN = /^[A-Z][A-Z0-9-]{2,31}$/;

function sha256(bytes: Uint8Array | string): string {
  return createHash('sha256').update(bytes).digest('hex');
}

function portablePath(path: string): string {
  return path.split(sep).join('/');
}

function pathIsInside(parent: string, child: string): boolean {
  const rel = relative(parent, child);
  return rel !== '' && !rel.startsWith(`..${sep}`) && rel !== '..' && !isAbsolute(rel);
}

function assertSafeRepoPath(path: string, label: string): void {
  if (
    path.length === 0 ||
    path.includes('\\') ||
    path.startsWith('/') ||
    path.endsWith('/') ||
    path.split('/').some((component) => component === '' || component === '.' || component === '..')
  ) {
    throw new Error(`${label} must be a normalized repository-relative path: ${path}`);
  }
}

function assertSafeGlob(pattern: string, label: string): void {
  if (
    pattern.length === 0 ||
    pattern === '*' ||
    pattern === '**' ||
    pattern.includes('\\') ||
    pattern.startsWith('/') ||
    pattern.split('/').some((component) => component === '..')
  ) {
    throw new Error(`${label} is too broad or is not repository-relative: ${pattern}`);
  }
}

function matchesAny(path: string, patterns: readonly string[]): boolean {
  return patterns.some((pattern) => new Bun.Glob(pattern).match(path));
}

function duplicate(values: readonly string[]): string | undefined {
  const seen = new Set<string>();
  for (const value of values) {
    if (seen.has(value)) return value;
    seen.add(value);
  }
  return undefined;
}

export function validatePublicExportPolicy(value: unknown): PublicExportPolicy {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('public export policy must be a JSON object');
  }
  const policy = value as Partial<PublicExportPolicy>;
  if (policy.schemaVersion !== 1) throw new Error('unsupported public export policy schema');
  if (typeof policy.name !== 'string' || policy.name.trim().length === 0) {
    throw new Error('public export policy requires a name');
  }
  if (
    !Number.isSafeInteger(policy.maximumFileBytes) ||
    (policy.maximumFileBytes ?? 0) < 1024 ||
    (policy.maximumFileBytes ?? 0) > 16 * 1024 * 1024
  ) {
    throw new Error('maximumFileBytes must be an integer between 1 KiB and 16 MiB');
  }

  for (const [label, values] of [
    ['requiredFiles', policy.requiredFiles],
    ['includePatterns', policy.includePatterns],
    ['excludePatterns', policy.excludePatterns],
    ['forbiddenBinaryExtensions', policy.forbiddenBinaryExtensions],
  ] as const) {
    if (!Array.isArray(values) || values.some((entry) => typeof entry !== 'string')) {
      throw new Error(`${label} must be a string array`);
    }
    const repeated = duplicate(values);
    if (repeated) throw new Error(`${label} contains duplicate entry: ${repeated}`);
  }

  for (const path of policy.requiredFiles ?? []) assertSafeRepoPath(path, 'required file');
  for (const pattern of policy.includePatterns ?? []) assertSafeGlob(pattern, 'include pattern');
  for (const pattern of policy.excludePatterns ?? []) assertSafeGlob(pattern, 'exclude pattern');

  if (!Array.isArray(policy.externalGates) || !Array.isArray(policy.blockedOmissions)) {
    throw new Error('externalGates and blockedOmissions must be arrays');
  }
  const gates = [...policy.externalGates, ...policy.blockedOmissions];
  for (const gate of gates) {
    if (
      !gate ||
      typeof gate !== 'object' ||
      typeof gate.id !== 'string' ||
      !SAFE_GATE_ID_PATTERN.test(gate.id) ||
      typeof gate.reason !== 'string' ||
      gate.reason.trim().length === 0
    ) {
      throw new Error('every publication gate requires a stable ID and reason');
    }
  }
  const repeatedGate = duplicate(gates.map((gate) => gate.id));
  if (repeatedGate) throw new Error(`duplicate publication gate ID: ${repeatedGate}`);
  for (const blocker of policy.blockedOmissions) {
    if (!Array.isArray(blocker.patterns) || blocker.patterns.length === 0) {
      throw new Error(`${blocker.id} requires at least one blocked path pattern`);
    }
    for (const pattern of blocker.patterns) assertSafeGlob(pattern, `${blocker.id} pattern`);
  }

  for (const extension of policy.forbiddenBinaryExtensions ?? []) {
    if (extension !== extension.toLowerCase() || !/^\.[a-z0-9]+$/.test(extension)) {
      throw new Error(`invalid forbidden binary extension: ${extension}`);
    }
  }
  if (!Array.isArray(policy.permittedBinaryFiles)) {
    throw new Error('permittedBinaryFiles must be an array');
  }
  const permittedPaths: string[] = [];
  for (const binary of policy.permittedBinaryFiles) {
    if (!binary || typeof binary !== 'object') {
      throw new Error('each permitted binary must be an object');
    }
    assertSafeRepoPath(binary.path, 'permitted binary path');
    permittedPaths.push(binary.path);
    if (!SHA256_PATTERN.test(binary.sha256)) {
      throw new Error(`${binary.path} requires an exact lowercase SHA-256`);
    }
    if (typeof binary.provenance !== 'string' || binary.provenance.trim().length === 0) {
      throw new Error(`${binary.path} requires provenance`);
    }
    if (!policy.forbiddenBinaryExtensions?.includes(extname(binary.path).toLowerCase())) {
      throw new Error(`${binary.path} is not covered by the binary-extension policy`);
    }
    if (!matchesAny(binary.path, policy.includePatterns ?? [])) {
      throw new Error(`permitted binary is not allowlisted: ${binary.path}`);
    }
    if (matchesAny(binary.path, policy.excludePatterns ?? [])) {
      throw new Error(`permitted binary is excluded: ${binary.path}`);
    }
    for (const blocker of policy.blockedOmissions) {
      if (matchesAny(binary.path, blocker.patterns)) {
        throw new Error(`permitted binary is blocked by ${blocker.id}: ${binary.path}`);
      }
    }
  }
  const repeatedBinary = duplicate(permittedPaths);
  if (repeatedBinary) throw new Error(`duplicate permitted binary: ${repeatedBinary}`);

  for (const required of policy.requiredFiles ?? []) {
    if (!matchesAny(required, policy.includePatterns ?? [])) {
      throw new Error(`required file is not allowlisted: ${required}`);
    }
    if (matchesAny(required, policy.excludePatterns ?? [])) {
      throw new Error(`required file is excluded: ${required}`);
    }
    for (const blocker of policy.blockedOmissions) {
      if (matchesAny(required, blocker.patterns)) {
        throw new Error(`required file is blocked by ${blocker.id}: ${required}`);
      }
    }
  }

  return policy as PublicExportPolicy;
}

export type PublicPathClassification =
  | { kind: 'include' }
  | { kind: 'exclude' }
  | { kind: 'not-allowlisted' }
  | { kind: 'blocker'; blocker: PublicExportBlocker };

export function classifyPublicPath(
  path: string,
  policy: PublicExportPolicy,
): PublicPathClassification {
  assertSafeRepoPath(path, 'candidate path');
  for (const blocker of policy.blockedOmissions) {
    if (matchesAny(path, blocker.patterns)) return { kind: 'blocker', blocker };
  }
  if (matchesAny(path, policy.excludePatterns)) return { kind: 'exclude' };
  if (!matchesAny(path, policy.includePatterns)) return { kind: 'not-allowlisted' };
  return { kind: 'include' };
}

async function runGit(repoRoot: string, args: string[]): Promise<Uint8Array> {
  const process = Bun.spawn(['git', '-C', repoRoot, ...args], {
    stdout: 'pipe',
    stderr: 'pipe',
  });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(process.stdout).arrayBuffer(),
    new Response(process.stderr).text(),
    process.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(`git ${args[0] ?? ''} failed: ${stderr.trim() || `exit ${exitCode}`}`);
  }
  return new Uint8Array(stdout);
}

function decode(bytes: Uint8Array): string {
  return new TextDecoder().decode(bytes);
}

function nulRecords(bytes: Uint8Array): string[] {
  return decode(bytes)
    .split('\0')
    .filter((entry) => entry.length > 0);
}

async function readGitSourceState(repoRoot: string): Promise<GitSourceState> {
  const [headBytes, treeBytes, statusBytes, candidateBytes, indexBytes] = await Promise.all([
    runGit(repoRoot, ['rev-parse', 'HEAD']),
    runGit(repoRoot, ['rev-parse', 'HEAD^{tree}']),
    runGit(repoRoot, ['status', '--porcelain=v1', '-z', '--untracked-files=all']),
    runGit(repoRoot, ['ls-files', '--cached', '--others', '--exclude-standard', '-z']),
    runGit(repoRoot, ['ls-files', '-s', '-z']),
  ]);
  const headCommit = decode(headBytes).trim();
  const headTree = decode(treeBytes).trim();
  if (!GIT_OBJECT_PATTERN.test(headCommit) || !GIT_OBJECT_PATTERN.test(headTree)) {
    throw new Error('public export requires a repository with a valid Git HEAD and tree');
  }

  const indexModes = new Map<string, string>();
  for (const record of nulRecords(indexBytes)) {
    const match = /^(\d{6}) [0-9a-f]{40} \d\t(.+)$/.exec(record);
    if (!match) throw new Error(`could not parse Git index entry: ${record}`);
    const [, mode, path] = match;
    indexModes.set(portablePath(path), mode);
  }

  const candidatePaths = [...new Set(nulRecords(candidateBytes).map(portablePath))].sort();
  return {
    headCommit,
    headTree,
    clean: statusBytes.length === 0,
    statusBytes,
    candidatePaths,
    indexModes,
  };
}

function sourceMode(
  path: string,
  indexMode: string | undefined,
  fileMode: number,
): '100644' | '100755' {
  if (indexMode === '120000') throw new Error(`public export refuses symlinks: ${path}`);
  if (indexMode === '160000') throw new Error(`public export refuses Git submodules: ${path}`);
  if (indexMode && indexMode !== '100644' && indexMode !== '100755') {
    throw new Error(`unsupported Git mode ${indexMode} for ${path}`);
  }
  return indexMode === '100755' || (!indexMode && (fileMode & 0o111) !== 0) ? '100755' : '100644';
}

function contentTreeSha256(files: readonly ExportedSourceFile[]): string {
  const hash = createHash('sha256');
  for (const file of files) {
    hash.update(file.mode);
    hash.update(' ');
    hash.update(file.path);
    hash.update('\0');
    hash.update(file.sha256);
    hash.update('\n');
  }
  return hash.digest('hex');
}

async function loadPolicy(
  policyPath: string,
): Promise<{ policy: PublicExportPolicy; bytes: Uint8Array }> {
  const bytes = await readFile(policyPath);
  let value: unknown;
  try {
    value = JSON.parse(decode(bytes));
  } catch (error) {
    throw new Error(`public export policy is not valid JSON: ${String(error)}`);
  }
  return { policy: validatePublicExportPolicy(value), bytes };
}

export async function planPublicSourceExport(
  options: PublicExportOptions,
): Promise<PublicExportPlan> {
  const repoRoot = await realpath(resolve(options.repoRoot));
  const policyPath = await realpath(resolve(options.policyPath));
  if (!pathIsInside(repoRoot, policyPath)) {
    throw new Error('public export policy must live inside the source repository');
  }
  if (options.outputDirectory) {
    const output = resolve(options.outputDirectory);
    const canonicalParent = await realpath(dirname(output));
    const canonicalOutput = resolve(canonicalParent, basename(output));
    if (canonicalOutput === repoRoot || pathIsInside(repoRoot, canonicalOutput)) {
      throw new Error('public export destination must be outside the private source repository');
    }
  }

  const [{ policy, bytes: policyBytes }, git] = await Promise.all([
    loadPolicy(policyPath),
    readGitSourceState(repoRoot),
  ]);
  const policyRepoPath = portablePath(relative(repoRoot, policyPath));
  const blockerPaths = new Map<string, string[]>();
  const sourceFiles: PublicExportPlan['sourceFiles'] = [];
  const permittedBinaries = new Map(
    policy.permittedBinaryFiles.map((entry) => [entry.path, entry]),
  );

  for (const path of git.candidatePaths) {
    const classification = classifyPublicPath(path, policy);
    if (classification.kind === 'blocker') {
      const paths = blockerPaths.get(classification.blocker.id) ?? [];
      paths.push(path);
      blockerPaths.set(classification.blocker.id, paths);
      continue;
    }
    if (classification.kind !== 'include') continue;

    const absolutePath = resolve(repoRoot, ...path.split('/'));
    if (!pathIsInside(repoRoot, absolutePath))
      throw new Error(`candidate escaped repository: ${path}`);
    let metadata;
    try {
      metadata = await lstat(absolutePath);
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') continue;
      throw error;
    }
    if (metadata.isSymbolicLink()) throw new Error(`public export refuses symlinks: ${path}`);
    if (!metadata.isFile()) throw new Error(`public export accepts only regular files: ${path}`);
    if (metadata.size > policy.maximumFileBytes) {
      throw new Error(`${path} exceeds the reviewed ${policy.maximumFileBytes}-byte source limit`);
    }

    const bytes = await readFile(absolutePath);
    const digest = sha256(bytes);
    const permittedBinary = permittedBinaries.get(path);
    if (permittedBinary) {
      if (digest !== permittedBinary.sha256) {
        throw new Error(`${path} differs from its reviewed binary digest`);
      }
    } else {
      const extension = extname(path).toLowerCase();
      if (policy.forbiddenBinaryExtensions.includes(extension)) {
        throw new Error(`unreviewed binary/media file is allowlisted: ${path}`);
      }
      if (bytes.includes(0)) throw new Error(`unreviewed binary content is allowlisted: ${path}`);
      try {
        new TextDecoder('utf-8', { fatal: true }).decode(bytes);
      } catch {
        throw new Error(`allowlisted source is not valid UTF-8: ${path}`);
      }
    }

    sourceFiles.push({
      path,
      absolutePath,
      bytes,
      mode: sourceMode(path, git.indexModes.get(path), metadata.mode),
      size: bytes.byteLength,
      sha256: digest,
    });
  }
  sourceFiles.sort((left, right) => left.path.localeCompare(right.path));

  const paths = new Set(sourceFiles.map((file) => file.path));
  for (const required of policy.requiredFiles) {
    if (!paths.has(required))
      throw new Error(`required public source file is missing: ${required}`);
  }
  for (const permittedBinary of policy.permittedBinaryFiles) {
    if (!paths.has(permittedBinary.path)) {
      throw new Error(`reviewed binary/media file is missing: ${permittedBinary.path}`);
    }
  }

  const omittedBlockers = policy.blockedOmissions
    .map((blocker) => ({
      id: blocker.id,
      reason: blocker.reason,
      paths: [...(blockerPaths.get(blocker.id) ?? [])].sort(),
    }))
    .filter((blocker) => blocker.paths.length > 0);
  const readinessFailures: PublicExportGate[] = [...policy.externalGates];
  if (!git.clean) {
    readinessFailures.push({
      id: 'SOURCE-DIRTY',
      reason: 'The source checkout is not a reviewed clean integration commit.',
    });
  }
  readinessFailures.push(...omittedBlockers.map(({ id, reason }) => ({ id, reason })));

  const files = sourceFiles.map(({ absolutePath: _absolutePath, bytes: _bytes, ...file }) => file);
  const candidateSetSha256 = sha256(`${git.candidatePaths.join('\n')}\n`);
  const manifest: PublicExportManifest = {
    schemaVersion: 1,
    policy: {
      name: policy.name,
      path: policyRepoPath,
      sha256: sha256(policyBytes),
    },
    source: {
      headCommit: git.headCommit,
      headTree: git.headTree,
      clean: git.clean,
      gitStatusSha256: sha256(git.statusBytes),
      candidateSetSha256,
    },
    snapshot: {
      publicationReady: readinessFailures.length === 0,
      fileCount: files.length,
      totalBytes: files.reduce((total, file) => total + file.size, 0),
      contentTreeSha256: contentTreeSha256(files),
    },
    readinessFailures,
    omittedBlockers,
    files,
  };

  if (!manifest.snapshot.publicationReady && !options.allowIncomplete) {
    throw new Error(
      `public export is not ready: ${readinessFailures.map((failure) => failure.id).join(', ')}`,
    );
  }
  return { manifest, sourceFiles };
}

export async function writePublicSourceExport(
  plan: PublicExportPlan,
  outputDirectory: string,
): Promise<void> {
  const output = resolve(outputDirectory);
  try {
    await lstat(output);
    throw new Error(`public export destination already exists: ${output}`);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
  }
  await mkdir(output, { recursive: false, mode: 0o755 });
  for (const file of plan.sourceFiles) {
    const destination = resolve(output, ...file.path.split('/'));
    if (!pathIsInside(output, destination))
      throw new Error(`export path escaped destination: ${file.path}`);
    await mkdir(dirname(destination), { recursive: true, mode: 0o755 });
    await writeFile(destination, file.bytes, { mode: file.mode === '100755' ? 0o755 : 0o644 });
    await chmod(destination, file.mode === '100755' ? 0o755 : 0o644);
  }
  await writeFile(
    resolve(output, PUBLIC_EXPORT_MANIFEST),
    `${JSON.stringify(plan.manifest, null, 2)}\n`,
    { encoding: 'utf8', mode: 0o644 },
  );
}

export async function exportPublicSource(
  options: PublicExportOptions,
): Promise<PublicExportManifest> {
  const plan = await planPublicSourceExport(options);
  if (options.write) {
    if (!options.outputDirectory) throw new Error('--output is required unless --check is used');
    await writePublicSourceExport(plan, options.outputDirectory);
  }
  return plan.manifest;
}

function option(name: string): string | undefined {
  const index = Bun.argv.indexOf(name);
  const value = index >= 0 ? Bun.argv[index + 1] : undefined;
  if (value?.startsWith('--')) return undefined;
  return value;
}

async function main(): Promise<void> {
  const repoRoot = resolve(option('--repo-root') ?? resolve(import.meta.dir, '../..'));
  const policyPath = resolve(
    option('--policy') ?? resolve(repoRoot, '.config/public-export-policy.json'),
  );
  const checkOnly = Bun.argv.includes('--check');
  const outputDirectory = option('--output');
  if (!checkOnly && !outputDirectory)
    throw new Error('--output is required unless --check is used');
  const manifest = await exportPublicSource({
    repoRoot,
    policyPath,
    outputDirectory,
    allowIncomplete: Bun.argv.includes('--allow-incomplete'),
    write: !checkOnly,
  });
  const summary = {
    publicationReady: manifest.snapshot.publicationReady,
    fileCount: manifest.snapshot.fileCount,
    totalBytes: manifest.snapshot.totalBytes,
    contentTreeSha256: manifest.snapshot.contentTreeSha256,
    readinessFailureIds: manifest.readinessFailures.map((failure) => failure.id),
  };
  process.stdout.write(`${JSON.stringify(summary, null, 2)}\n`);
}

if (import.meta.main) await main();
