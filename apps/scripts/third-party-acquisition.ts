#!/usr/bin/env bun

import { createHash } from 'node:crypto';
import { constants as fsConstants } from 'node:fs';
import {
  copyFile,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile,
} from 'node:fs/promises';
import { basename, dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { tmpdir } from 'node:os';

const SHA256 = /^[0-9a-f]{64}$/;

type VpxFile = { path: string; sha256: string };
type Archive = { id: string; url: string; sha256: string };
type ArchiveMember = {
  archiveId: string;
  path: string;
  sha256: string;
  output?: string;
};
type WixPackage = { id: string; sha256: string };

export type ThirdPartyAcquisitionPolicy = {
  schemaVersion: 1;
  vpxEncode: {
    version: string;
    upstreamRevision: string;
    archiveUrl: string;
    archiveSha256: string;
    archivePrefix: string;
    patchPath: string;
    patchSha256: string;
    patchedFiles: VpxFile[];
  };
  sevenZip: {
    version: string;
    archives: Archive[];
    members: ArchiveMember[];
    retainedNotices: VpxFile[];
  };
  wix: {
    version: string;
    upstreamRevision: string;
    source: string;
    licenseFile: string;
    licenseSha256: string;
    retainedNotice: VpxFile;
    packages: WixPackage[];
  };
};

export type ExtractionKind = '7zip' | 'tar';

function digest(bytes: Uint8Array | string): string {
  return createHash('sha256').update(bytes).digest('hex');
}

function portable(path: string): string {
  return path.split(sep).join('/');
}

function safeRelativePath(path: string, label: string): void {
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

function safePinnedUrl(value: string, label: string): URL {
  const url = new URL(value);
  if (url.protocol !== 'https:') throw new Error(`${label} must use HTTPS`);
  if (/(?:^|[-_.])latest(?:[-_.]|$)/i.test(url.pathname)) {
    throw new Error(`${label} must not use a floating latest URL`);
  }
  return url;
}

function requireDigest(value: string, label: string): void {
  if (!SHA256.test(value)) throw new Error(`${label} requires an exact lowercase SHA-256`);
}

function duplicates(values: readonly string[]): string | undefined {
  const seen = new Set<string>();
  for (const value of values) {
    if (seen.has(value)) return value;
    seen.add(value);
  }
  return undefined;
}

export function validateThirdPartyAcquisitionPolicy(value: unknown): ThirdPartyAcquisitionPolicy {
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('third-party acquisition policy must be a JSON object');
  }
  const policy = value as Partial<ThirdPartyAcquisitionPolicy>;
  if (policy.schemaVersion !== 1 || !policy.vpxEncode || !policy.sevenZip || !policy.wix) {
    throw new Error('unsupported or incomplete third-party acquisition policy');
  }

  const vpx = policy.vpxEncode;
  safePinnedUrl(vpx.archiveUrl, 'vpx-encode archive URL');
  requireDigest(vpx.archiveSha256, 'vpx-encode archive');
  requireDigest(vpx.patchSha256, 'vpx-encode patch');
  safeRelativePath(vpx.patchPath, 'vpx-encode patch');
  if (!/^[0-9a-f]{40}$/.test(vpx.upstreamRevision)) {
    throw new Error('vpx-encode requires an exact upstream Git revision');
  }
  if (!Array.isArray(vpx.patchedFiles) || vpx.patchedFiles.length === 0) {
    throw new Error('vpx-encode patchedFiles must not be empty');
  }
  for (const file of vpx.patchedFiles) {
    safeRelativePath(file.path, 'vpx-encode patched file');
    requireDigest(file.sha256, `vpx-encode patched file ${file.path}`);
  }
  const repeatedVpxFile = duplicates(vpx.patchedFiles.map((file) => file.path));
  if (repeatedVpxFile) throw new Error(`duplicate vpx-encode patched file: ${repeatedVpxFile}`);

  if (!Array.isArray(policy.sevenZip.archives) || !Array.isArray(policy.sevenZip.members)) {
    throw new Error('7-Zip archives and members must be arrays');
  }
  for (const archive of policy.sevenZip.archives) {
    safePinnedUrl(archive.url, `7-Zip ${archive.id} URL`);
    requireDigest(archive.sha256, `7-Zip ${archive.id} archive`);
  }
  const archiveIds = policy.sevenZip.archives.map((archive) => archive.id);
  const repeatedArchive = duplicates(archiveIds);
  if (repeatedArchive) throw new Error(`duplicate 7-Zip archive ID: ${repeatedArchive}`);
  for (const member of policy.sevenZip.members) {
    if (!archiveIds.includes(member.archiveId)) {
      throw new Error(`7-Zip member refers to unknown archive: ${member.archiveId}`);
    }
    safeRelativePath(member.path, '7-Zip archive member');
    requireDigest(member.sha256, `7-Zip member ${member.path}`);
    if (member.output) safeRelativePath(member.output, '7-Zip member output');
  }
  if (!Array.isArray(policy.sevenZip.retainedNotices)) {
    throw new Error('7-Zip retained notices must be an array');
  }
  for (const notice of policy.sevenZip.retainedNotices) {
    safeRelativePath(notice.path, '7-Zip retained notice');
    requireDigest(notice.sha256, `7-Zip retained notice ${notice.path}`);
  }

  safePinnedUrl(policy.wix.source, 'WiX NuGet source');
  if (!/^[0-9a-f]{40}$/.test(policy.wix.upstreamRevision)) {
    throw new Error('WiX requires an exact upstream Git revision');
  }
  requireDigest(policy.wix.licenseSha256, 'WiX package licence');
  safeRelativePath(policy.wix.licenseFile, 'WiX package licence member');
  safeRelativePath(policy.wix.retainedNotice.path, 'WiX retained notice');
  requireDigest(policy.wix.retainedNotice.sha256, 'WiX retained notice');
  if (!Array.isArray(policy.wix.packages) || policy.wix.packages.length === 0) {
    throw new Error('WiX packages must not be empty');
  }
  for (const entry of policy.wix.packages) {
    if (!/^[A-Za-z0-9.-]+$/.test(entry.id)) throw new Error(`invalid WiX package ID: ${entry.id}`);
    requireDigest(entry.sha256, `WiX package ${entry.id}`);
  }
  const repeatedPackage = duplicates(policy.wix.packages.map((entry) => entry.id.toLowerCase()));
  if (repeatedPackage) throw new Error(`duplicate WiX package ID: ${repeatedPackage}`);
  return policy as ThirdPartyAcquisitionPolicy;
}

export async function loadThirdPartyAcquisitionPolicy(
  policyPath: string,
): Promise<ThirdPartyAcquisitionPolicy> {
  return validateThirdPartyAcquisitionPolicy(JSON.parse(await readFile(policyPath, 'utf8')));
}

async function pathExists(path: string): Promise<boolean> {
  try {
    await lstat(path);
    return true;
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false;
    throw error;
  }
}

async function assertRegularFileSha(path: string, expected: string, label: string): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink())
    throw new Error(`${label} is not a regular file`);
  const actual = digest(await readFile(path));
  if (actual !== expected)
    throw new Error(`${label} SHA-256 mismatch: expected ${expected}; got ${actual}`);
}

async function run(command: string, args: string[], cwd?: string): Promise<void> {
  const child = Bun.spawn([command, ...args], { cwd, stdout: 'pipe', stderr: 'pipe' });
  const [stdout, stderr, exitCode] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (exitCode !== 0) {
    throw new Error(
      `${basename(command)} failed (${exitCode}): ${(stderr || stdout).trim() || 'no output'}`,
    );
  }
}

async function downloadPinned(url: string, expected: string, destination: string): Promise<void> {
  const response = await fetch(url, { redirect: 'follow' });
  if (!response.ok) throw new Error(`download failed (${response.status}) for ${url}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  const actual = digest(bytes);
  if (actual !== expected) {
    throw new Error(`download SHA-256 mismatch for ${url}: expected ${expected}; got ${actual}`);
  }
  await writeFile(destination, bytes, { flag: 'wx', mode: 0o600 });
}

async function filesBelow(root: string, prefix = ''): Promise<string[]> {
  const found: string[] = [];
  for (const entry of await readdir(resolve(root, prefix), { withFileTypes: true })) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isSymbolicLink()) throw new Error(`third-party source contains a symlink: ${path}`);
    if (entry.isDirectory()) found.push(...(await filesBelow(root, path)));
    else if (entry.isFile()) found.push(path);
    else throw new Error(`third-party source contains a special file: ${path}`);
  }
  return found.sort();
}

export async function verifyPatchedVpxTree(
  root: string,
  policy: ThirdPartyAcquisitionPolicy,
): Promise<void> {
  const rootMetadata = await lstat(root);
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) {
    throw new Error('vpx-encode root is not a regular directory');
  }
  const expected = policy.vpxEncode.patchedFiles.map((file) => file.path).sort();
  const actual = (await filesBelow(root)).filter((path) => path !== '.DS_Store');
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(`vpx-encode tree differs from reviewed file set: ${actual.join(', ')}`);
  }
  await Promise.all(
    policy.vpxEncode.patchedFiles.map((file) =>
      assertRegularFileSha(
        resolve(root, ...file.path.split('/')),
        file.sha256,
        `vpx-encode ${file.path}`,
      ),
    ),
  );
}

export async function acquireVpxEncode(options: {
  repoRoot: string;
  policy: ThirdPartyAcquisitionPolicy;
  output?: string;
}): Promise<string> {
  const repoRoot = await realpath(resolve(options.repoRoot));
  const output = resolve(options.output ?? resolve(repoRoot, 'apps/vpx-encode'));
  const patchPath = resolve(repoRoot, ...options.policy.vpxEncode.patchPath.split('/'));
  await assertRegularFileSha(patchPath, options.policy.vpxEncode.patchSha256, 'vpx-encode patch');
  if (await pathExists(output)) {
    await verifyPatchedVpxTree(output, options.policy);
    return output;
  }

  const temporary = await mkdtemp(join(tmpdir(), 'talos-vpx-acquisition-'));
  const archive = resolve(temporary, 'vpx-encode.crate');
  const extracted = resolve(temporary, 'extracted');
  const staged = resolve(temporary, 'staged');
  let createdOutput = false;
  try {
    await mkdir(extracted);
    await mkdir(resolve(staged, 'src'), { recursive: true });
    await downloadPinned(
      options.policy.vpxEncode.archiveUrl,
      options.policy.vpxEncode.archiveSha256,
      archive,
    );
    const prefix = options.policy.vpxEncode.archivePrefix;
    await run('tar', [
      '-xzf',
      archive,
      '-C',
      extracted,
      `${prefix}/Cargo.toml.orig`,
      `${prefix}/README.md`,
      `${prefix}/src/lib.rs`,
    ]);
    await copyFile(resolve(extracted, prefix, 'Cargo.toml.orig'), resolve(staged, 'Cargo.toml'));
    await copyFile(resolve(extracted, prefix, 'README.md'), resolve(staged, 'README.md'));
    await copyFile(resolve(extracted, prefix, 'src/lib.rs'), resolve(staged, 'src/lib.rs'));
    await run('git', ['apply', '--whitespace=nowarn', patchPath], staged);
    await verifyPatchedVpxTree(staged, options.policy);
    await mkdir(dirname(output), { recursive: true });
    await mkdir(output);
    createdOutput = true;
    await mkdir(resolve(output, 'src'));
    await copyFile(resolve(staged, 'Cargo.toml'), resolve(output, 'Cargo.toml'));
    await copyFile(resolve(staged, 'README.md'), resolve(output, 'README.md'));
    await copyFile(resolve(staged, 'src/lib.rs'), resolve(output, 'src/lib.rs'));
    await verifyPatchedVpxTree(output, options.policy);
    return output;
  } catch (error) {
    if (createdOutput) await rm(output, { recursive: true, force: true });
    throw error;
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

function packageFileName(packageId: string, version: string): string {
  return `${packageId.toLowerCase()}.${version}.nupkg`;
}

async function extractMembers(
  extractor: string,
  kind: ExtractionKind,
  archive: string,
  output: string,
  members: string[],
): Promise<void> {
  await mkdir(output, { recursive: true });
  if (kind === 'tar') {
    await run(extractor, ['-xf', archive, '-C', output, ...members]);
  } else {
    await run(extractor, ['x', '-y', '-bd', `-o${output}`, archive, ...members]);
  }
}

function xmlEscape(value: string): string {
  return value.replaceAll('&', '&amp;').replaceAll('"', '&quot;').replaceAll('<', '&lt;');
}

export async function verifyInstallerNotices(
  repoRoot: string,
  policy: ThirdPartyAcquisitionPolicy,
): Promise<void> {
  const root = await realpath(resolve(repoRoot));
  for (const notice of [...policy.sevenZip.retainedNotices, policy.wix.retainedNotice]) {
    const noticePath = resolve(root, ...notice.path.split('/'));
    if (!(await pathExists(noticePath))) {
      throw new Error(`retained third-party notice is missing: ${notice.path}`);
    }
    await assertRegularFileSha(noticePath, notice.sha256, `retained notice ${notice.path}`);
  }
}

async function writeGeneratedFile(path: string, contents: string): Promise<void> {
  if (await pathExists(path)) {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new Error(`generated path is not a regular file: ${path}`);
    }
    if ((await readFile(path, 'utf8')) !== contents) {
      throw new Error(`refusing to replace an unexpected generated file: ${path}`);
    }
    return;
  }
  await writeFile(path, contents, { encoding: 'utf8', flag: 'wx', mode: 0o600 });
}

export async function acquireInstallerTools(options: {
  repoRoot: string;
  policy: ThirdPartyAcquisitionPolicy;
  extractor: string;
  extractionKind: ExtractionKind;
}): Promise<{
  sevenZipExecutable: string;
  sfxStub: string;
  nugetConfig: string;
  manifest: string;
}> {
  const repoRoot = await realpath(resolve(options.repoRoot));
  const sevenZip = options.policy.sevenZip;
  const wix = options.policy.wix;
  const toolRoot = resolve(repoRoot, `apps/installer/tmp/third-party/7zip-${sevenZip.version}`);
  const feedRoot = resolve(repoRoot, `apps/installer/tmp/third-party/wix-${wix.version}-feed`);
  const nugetConfig = resolve(repoRoot, 'apps/installer/tmp/third-party/NuGet.Config');
  const manifest = resolve(toolRoot, 'acquisition-manifest.json');
  const executableMember = sevenZip.members.find((member) => member.path === '7za.exe');
  const stubMember = sevenZip.members.find((member) => member.path === 'bin/7zSD.sfx');
  if (!executableMember?.output || !stubMember?.output) {
    throw new Error('7-Zip policy requires output paths for 7za.exe and bin/7zSD.sfx');
  }
  const sevenZipExecutable = resolve(repoRoot, ...executableMember.output.split('/'));
  const sfxStub = resolve(repoRoot, ...stubMember.output.split('/'));

  await verifyInstallerNotices(repoRoot, options.policy);

  const temporary = await mkdtemp(join(tmpdir(), 'talos-installer-acquisition-'));
  try {
    for (const archive of sevenZip.archives) {
      const path = resolve(temporary, basename(new URL(archive.url).pathname));
      await downloadPinned(archive.url, archive.sha256, path);
      const members = sevenZip.members.filter((member) => member.archiveId === archive.id);
      const extractedRoot = resolve(temporary, `extracted-${archive.id}`);
      await extractMembers(
        options.extractor,
        options.extractionKind,
        path,
        extractedRoot,
        members.map((member) => member.path),
      );
      for (const member of members) {
        await assertRegularFileSha(
          resolve(extractedRoot, ...member.path.split('/')),
          member.sha256,
          `7-Zip ${member.path}`,
        );
      }
      for (const member of members.filter((entry) => entry.output)) {
        const destination = resolve(repoRoot, ...member.output!.split('/'));
        if (await pathExists(destination)) {
          await assertRegularFileSha(destination, member.sha256, `existing ${member.output}`);
        } else {
          await mkdir(dirname(destination), { recursive: true });
          await copyFile(
            resolve(extractedRoot, ...member.path.split('/')),
            destination,
            fsConstants.COPYFILE_EXCL,
          );
        }
      }
    }

    await mkdir(feedRoot, { recursive: true });
    for (const entry of wix.packages) {
      const filename = packageFileName(entry.id, wix.version);
      const destination = resolve(feedRoot, filename);
      if (await pathExists(destination)) {
        await assertRegularFileSha(destination, entry.sha256, `existing WiX package ${entry.id}`);
      } else {
        const url = `${wix.source}/${entry.id.toLowerCase()}/${wix.version}/${filename}`;
        await downloadPinned(url, entry.sha256, destination);
      }
      const extractedPackage = resolve(temporary, `wix-${entry.id.toLowerCase()}`);
      await extractMembers(
        options.extractor,
        options.extractionKind,
        destination,
        extractedPackage,
        [wix.licenseFile],
      );
      await assertRegularFileSha(
        resolve(extractedPackage, ...wix.licenseFile.split('/')),
        wix.licenseSha256,
        `WiX ${entry.id} embedded ${wix.licenseFile}`,
      );
    }

    const config = [
      '<?xml version="1.0" encoding="utf-8"?>',
      '<configuration>',
      '  <packageSources>',
      '    <clear />',
      `    <add key="TalosPinnedWiX" value="${xmlEscape(feedRoot)}" />`,
      '  </packageSources>',
      '</configuration>',
      '',
    ].join('\n');
    await mkdir(dirname(nugetConfig), { recursive: true });
    await writeGeneratedFile(nugetConfig, config);

    const acquisitionManifest = {
      schemaVersion: 1,
      sevenZip: {
        version: sevenZip.version,
        archives: sevenZip.archives,
        members: sevenZip.members,
        retainedNotices: sevenZip.retainedNotices,
      },
      wix: {
        version: wix.version,
        upstreamRevision: wix.upstreamRevision,
        packages: wix.packages,
        licenseFile: wix.licenseFile,
        licenseSha256: wix.licenseSha256,
        retainedNotice: wix.retainedNotice,
      },
    };
    await mkdir(toolRoot, { recursive: true });
    await writeGeneratedFile(manifest, `${JSON.stringify(acquisitionManifest, null, 2)}\n`);
    await assertRegularFileSha(sevenZipExecutable, executableMember.sha256, 'pinned 7za.exe');
    await assertRegularFileSha(sfxStub, stubMember.sha256, 'pinned 7zSD.sfx');
    return { sevenZipExecutable, sfxStub, nugetConfig, manifest };
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

function option(name: string): string | undefined {
  const index = Bun.argv.indexOf(name);
  const value = index >= 0 ? Bun.argv[index + 1] : undefined;
  return value && !value.startsWith('--') ? value : undefined;
}

async function main(): Promise<void> {
  const command = Bun.argv[2];
  const repoRoot = resolve(option('--repo-root') ?? resolve(import.meta.dir, '../..'));
  const policyPath = resolve(
    option('--policy') ?? resolve(repoRoot, '.config/third-party-acquisition.json'),
  );
  const policy = await loadThirdPartyAcquisitionPolicy(policyPath);
  if (command === 'vpx') {
    const output = await acquireVpxEncode({ repoRoot, policy, output: option('--output') });
    process.stdout.write(
      `${JSON.stringify({ vpxEncode: portable(relative(repoRoot, output)) })}\n`,
    );
    return;
  }
  if (command === 'installer') {
    const extractor = option('--extractor');
    if (!extractor) throw new Error('installer acquisition requires --extractor <7z-or-tar>');
    const extractionKind = option('--extractor-kind') ?? '7zip';
    if (extractionKind !== '7zip' && extractionKind !== 'tar') {
      throw new Error('--extractor-kind must be 7zip or tar');
    }
    const result = await acquireInstallerTools({
      repoRoot,
      policy,
      extractor,
      extractionKind,
    });
    process.stdout.write(
      `${JSON.stringify({
        sevenZipExecutable: portable(relative(repoRoot, result.sevenZipExecutable)),
        sfxStub: portable(relative(repoRoot, result.sfxStub)),
        nugetConfig: portable(relative(repoRoot, result.nugetConfig)),
        manifest: portable(relative(repoRoot, result.manifest)),
      })}\n`,
    );
    return;
  }
  throw new Error('usage: third-party-acquisition.ts <vpx|installer> [options]');
}

if (import.meta.main) await main();
