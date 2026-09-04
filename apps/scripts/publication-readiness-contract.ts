import { resolve } from 'node:path';

const PROJECT_LICENSE = 'AGPL-3.0-only';
const CANONICAL_LICENSE_URL = 'https://www.gnu.org/licenses/agpl-3.0.html';

export type PublicationInputs = {
  publicationIdentity?: unknown;
  licensePresent: boolean;
  licenseText: string;
  packageLicense: string | undefined;
  agentBundle: string;
  viewerBundle: string;
  viewerLicense: string;
  agentX86Installer: string;
  agentX64Installer: string;
  viewerInstaller: string;
  readme: string;
  contributing: string;
  securityPolicy: string;
  conductPolicy: string;
  thirdPartyNotices: string;
  provenanceInventory: string;
  contactPage: string;
  termsPage: string;
  privacyPage: string;
  readinessGuide: string;
  governanceFilesPresent: boolean;
  generatedCursorStatePresent: boolean;
};

function hasInstallerLicencePayload(installer: string): boolean {
  return (
    /Source="[^"\r\n]*LICENSE"/i.test(installer) &&
    /Source="[^"\r\n]*THIRD_PARTY_NOTICES\.md"/i.test(installer)
  );
}

export function publicationReadinessFailures(input: PublicationInputs): string[] {
  const failures: string[] = [];
  let identity: { copyrightHolder: string; sourceUrl: string } | undefined;
  if (input.publicationIdentity !== undefined) {
    const value = input.publicationIdentity;
    if (
      !value ||
      typeof value !== 'object' ||
      !('schemaVersion' in value) ||
      value.schemaVersion !== 1 ||
      !('copyrightHolder' in value) ||
      typeof value.copyrightHolder !== 'string' ||
      !value.copyrightHolder.trim() ||
      value.copyrightHolder.length > 160 ||
      !('sourceUrl' in value) ||
      typeof value.sourceUrl !== 'string' ||
      !/^https:\/\/github\.com\/[A-Za-z0-9-]+\/[A-Za-z0-9_.-]+$/.test(value.sourceUrl)
    ) {
      failures.push('publication identity must record a holder and canonical GitHub source URL');
    } else {
      identity = { copyrightHolder: value.copyrightHolder, sourceUrl: value.sourceUrl };
    }
  }
  const bundles = `${input.agentBundle}\n${input.viewerBundle}`;

  if (bundles.includes('example.com/license')) {
    failures.push('installer bundles must not publish a placeholder license URL');
  }
  if (/organization(?:'|’)?s agreement with Talos/i.test(input.viewerLicense)) {
    failures.push('the Viewer installer must not invent a proprietary Talos agreement');
  }

  if (input.licensePresent) {
    if (input.packageLicense !== PROJECT_LICENSE) {
      failures.push(`a licensed repository package must declare ${PROJECT_LICENSE}`);
    }
    if (
      !/GNU AFFERO GENERAL PUBLIC LICENSE/.test(input.licenseText) ||
      !/Version 3, 19 November 2007/.test(input.licenseText) ||
      !/13\. Remote Network Interaction/.test(input.licenseText)
    ) {
      failures.push('root LICENSE must contain the GNU AGPL version 3 text');
    }
    for (const [label, bundle] of [
      ['agent bundle', input.agentBundle],
      ['viewer bundle', input.viewerBundle],
    ] as const) {
      if (!bundle.includes(`LicenseUrl="${CANONICAL_LICENSE_URL}"`)) {
        failures.push(`${label} must link to the canonical GNU AGPL version 3 text`);
      }
    }
    if (
      !input.viewerLicense.includes(PROJECT_LICENSE) ||
      !input.viewerLicense.includes(CANONICAL_LICENSE_URL)
    ) {
      failures.push('the Viewer installer must show the AGPL-3.0-only notice and canonical URL');
    }
    if (/not ready for public distribution/i.test(input.viewerLicense)) {
      failures.push('replace the Viewer pre-release notice after selecting the project license');
    }
    for (const [label, installer] of [
      ['Agent x86 MSI', input.agentX86Installer],
      ['Agent x64 MSI', input.agentX64Installer],
      ['Viewer MSI', input.viewerInstaller],
    ] as const) {
      if (!hasInstallerLicencePayload(installer)) {
        failures.push(`${label} must install the project licence and third-party notices`);
      }
    }
    if (
      !input.readme.includes(PROJECT_LICENSE) ||
      !input.readme.includes('THIRD_PARTY_NOTICES.md')
    ) {
      failures.push('README must identify AGPL-3.0-only and the third-party notices');
    }
    if (
      !input.contributing.includes(PROJECT_LICENSE) ||
      !/inbound-equals-outbound/i.test(input.contributing)
    ) {
      failures.push('CONTRIBUTING must document inbound-equals-outbound AGPL terms');
    }
    if (!/licensed, but not yet cleared for publication/i.test(input.readinessGuide)) {
      failures.push('the readiness guide must distinguish licensing from publication clearance');
    }
    for (const blocker of [
      ...(!identity ? ['exact legal copyright-holder name', 'corresponding-source URL'] : []),
      'non-maintainer account',
    ]) {
      if (!input.readinessGuide.includes(blocker)) {
        failures.push(`the readiness guide must retain the unresolved ${blocker} blocker`);
      }
    }
    if (
      !/root[\s\S]{0,200}AGPL/i.test(input.thirdPartyNotices) ||
      !/does not replace or\s+alter a third party's licence/i.test(input.thirdPartyNotices)
    ) {
      failures.push('third-party notices must preserve the third-party licence boundary');
    }
    if (
      !input.provenanceInventory.includes(PROJECT_LICENSE) ||
      (!identity &&
        !/exact legal copyright-holder name has not been supplied/i.test(input.provenanceInventory))
    ) {
      failures.push('the provenance inventory must record the selected licence and owner blocker');
    }
    if (identity) {
      if (
        !input.readme.includes(identity.copyrightHolder) ||
        !input.provenanceInventory.includes(identity.copyrightHolder)
      ) {
        failures.push('README and provenance inventory must name the confirmed copyright holder');
      }
      if (
        !input.readinessGuide.includes(identity.sourceUrl) ||
        !input.provenanceInventory.includes(identity.sourceUrl)
      ) {
        failures.push('readiness and provenance documents must record the confirmed source URL');
      }
      if (
        /exact legal copyright-holder name has not been supplied/i.test(input.provenanceInventory)
      ) {
        failures.push('remove the resolved copyright-holder blocker after identity confirmation');
      }
    }
  } else {
    if (input.packageLicense !== 'UNLICENSED') {
      failures.push('without a root license the package must remain explicitly UNLICENSED');
    }
    for (const [label, bundle] of [
      ['agent bundle', input.agentBundle],
      ['viewer bundle', input.viewerBundle],
    ] as const) {
      if (!/LicenseUrl=""/.test(bundle)) {
        failures.push(`${label} must hide its license link until a real license is selected`);
      }
    }
    if (!/not ready for public distribution/i.test(input.viewerLicense)) {
      failures.push('the Viewer installer must carry the truthful pre-release notice');
    }
    if (!/Status:\s*\*\*not yet cleared for publication\*\*/i.test(input.readinessGuide)) {
      failures.push('the readiness guide must explicitly say publication is not yet cleared');
    }
    if (!/Select the source distribution license/i.test(input.readinessGuide)) {
      failures.push('the readiness guide must retain the owner license decision');
    }
  }

  if (
    /github\.com\/Sebtek-Ltd\/AssistAI/i.test(`${input.securityPolicy}\n${input.conductPolicy}`)
  ) {
    failures.push('governance files must not link to the unconfirmed private AssistAI repository');
  }
  if (
    !input.securityPolicy.includes('../../security/advisories/new') ||
    !input.conductPolicy.includes('../../security/advisories/new')
  ) {
    failures.push(
      'security and conduct policies must use the repository-relative private report form',
    );
  }
  if (/<form\b/i.test(input.contactPage) || /console\.log\s*\(/.test(input.contactPage)) {
    failures.push('the Community contact page must not fake submission or log contact data');
  }
  if (/gmail\.com/i.test(`${input.contactPage}\n${input.termsPage}\n${input.privacyPage}`)) {
    failures.push('operator-facing Community pages must not publish a personal Gmail address');
  }
  if (
    /Last updated[^\n]*(new Date|toLocaleDateString)/i.test(
      `${input.termsPage}\n${input.privacyPage}`,
    )
  ) {
    failures.push(
      "legal/operator pages must not claim the viewer's current date as their review date",
    );
  }
  if (!input.governanceFilesPresent) {
    failures.push(
      'licence, notices, provenance, contributor, conduct, security, support, and readiness files are required',
    );
  }
  if (input.generatedCursorStatePresent) {
    failures.push('generated Cursor state/plans must not be included in the publication tree');
  }

  return failures;
}

async function readText(path: string): Promise<string> {
  return Bun.file(path).text();
}

async function anyExists(paths: string[]): Promise<boolean> {
  for (const path of paths) {
    if (await Bun.file(path).exists()) return true;
  }
  return false;
}

export async function checkPublicationReadinessContract(
  repoRoot = resolve(import.meta.dir, '../..'),
): Promise<{ failures: string[]; mode: 'licensed' | 'pre-release' }> {
  const licensePaths = [
    resolve(repoRoot, 'LICENSE'),
    resolve(repoRoot, 'LICENSE.md'),
    resolve(repoRoot, 'COPYING'),
  ];
  const licensePresent = await anyExists(licensePaths);
  let selectedLicensePath: string | undefined;
  for (const path of licensePaths) {
    if (await Bun.file(path).exists()) {
      selectedLicensePath = path;
      break;
    }
  }
  const manifest = JSON.parse(await readText(resolve(repoRoot, 'apps/package.json'))) as {
    license?: string;
  };
  const governanceFilesPresent = await Promise.all(
    [
      'LICENSE',
      'THIRD_PARTY_NOTICES.md',
      'CONTRIBUTING.md',
      'CODE_OF_CONDUCT.md',
      'SECURITY.md',
      'SUPPORT.md',
      'docs/licensing-and-provenance.md',
      'docs/open-source-readiness.md',
    ].map((path) => Bun.file(resolve(repoRoot, path)).exists()),
  ).then((values) => values.every(Boolean));
  const generatedCursorStatePresent = await anyExists([
    resolve(repoRoot, '.cursor/hooks/state/continual-learning-index.json'),
    resolve(repoRoot, '.cursor/hooks/state/continual-learning.json'),
    resolve(repoRoot, '.cursor/plans/rmm_relay_rust_app.plan.md'),
  ]);

  const [
    licenseText,
    agentBundle,
    viewerBundle,
    viewerLicense,
    agentX86Installer,
    agentX64Installer,
    viewerInstaller,
    readme,
    contributing,
    securityPolicy,
    conductPolicy,
    thirdPartyNotices,
    provenanceInventory,
    contactPage,
    termsPage,
    privacyPage,
    readinessGuide,
    publicationIdentityText,
  ] = await Promise.all([
    selectedLicensePath ? readText(selectedLicensePath) : Promise.resolve(''),
    readText(resolve(repoRoot, 'apps/installer/bundle/Bundle.wxs')),
    readText(resolve(repoRoot, 'apps/installer/bundle/Viewer.Bundle.wxs')),
    readText(resolve(repoRoot, 'apps/installer/msi/viewer-license.rtf')),
    readText(resolve(repoRoot, 'apps/installer/msi/Agent.x86.wxs')),
    readText(resolve(repoRoot, 'apps/installer/msi/Agent.x64.wxs')),
    readText(resolve(repoRoot, 'apps/installer/msi/Viewer.x64.wxs')),
    readText(resolve(repoRoot, 'README.md')),
    readText(resolve(repoRoot, 'CONTRIBUTING.md')),
    readText(resolve(repoRoot, 'SECURITY.md')),
    readText(resolve(repoRoot, 'CODE_OF_CONDUCT.md')),
    readText(resolve(repoRoot, 'THIRD_PARTY_NOTICES.md')),
    readText(resolve(repoRoot, 'docs/licensing-and-provenance.md')),
    readText(resolve(repoRoot, 'apps/frontend/src/routes/contact/+page.svelte')),
    readText(resolve(repoRoot, 'apps/frontend/src/routes/terms/+page.svelte')),
    readText(resolve(repoRoot, 'apps/frontend/src/routes/privacy/+page.svelte')),
    readText(resolve(repoRoot, 'docs/open-source-readiness.md')),
    readText(resolve(repoRoot, '.config/publication-identity.json')),
  ]);

  return {
    mode: licensePresent ? 'licensed' : 'pre-release',
    failures: publicationReadinessFailures({
      publicationIdentity: publicationIdentityText
        ? (JSON.parse(publicationIdentityText) as unknown)
        : undefined,
      licensePresent,
      licenseText,
      packageLicense: manifest.license,
      agentBundle,
      viewerBundle,
      viewerLicense,
      agentX86Installer,
      agentX64Installer,
      viewerInstaller,
      readme,
      contributing,
      securityPolicy,
      conductPolicy,
      thirdPartyNotices,
      provenanceInventory,
      contactPage,
      termsPage,
      privacyPage,
      readinessGuide,
      governanceFilesPresent,
      generatedCursorStatePresent,
    }),
  };
}

if (import.meta.main) {
  const result = await checkPublicationReadinessContract();
  if (result.failures.length > 0) {
    console.error('Publication readiness contract failed:\n');
    for (const failure of result.failures) console.error(`- ${failure}`);
    process.exit(1);
  }
  console.log(`Publication readiness contract passed (${result.mode} mode).`);
}
