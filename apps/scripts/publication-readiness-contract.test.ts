import { describe, expect, test } from 'bun:test';
import {
  checkPublicationReadinessContract,
  publicationReadinessFailures,
  type PublicationInputs,
} from './publication-readiness-contract';

const installerWithNotices =
  '<File Source="../../../LICENSE" /><File Source="../../../THIRD_PARTY_NOTICES.md" />';

const safePreRelease: PublicationInputs = {
  licensePresent: false,
  licenseText: '',
  packageLicense: 'UNLICENSED',
  agentBundle: '<Wix LicenseUrl="" />',
  viewerBundle: '<Wix LicenseUrl="" />',
  viewerLicense: 'This tree is not ready for public distribution.',
  agentX86Installer: '',
  agentX64Installer: '',
  viewerInstaller: '',
  readme: 'Private development tree',
  contributing: 'Contribution policy pending',
  securityPolicy: 'Use ../../security/advisories/new for a private report.',
  conductPolicy: 'Use ../../security/advisories/new for a private conduct report.',
  thirdPartyNotices: '',
  provenanceInventory: '',
  contactPage: 'No support destination is configured',
  termsPage: 'Operator terms',
  privacyPage: 'Operator privacy notice',
  readinessGuide:
    'Status: **not yet cleared for publication**\n- [ ] Select the source distribution license',
  governanceFilesPresent: true,
  generatedCursorStatePresent: false,
};

const safeLicensed: PublicationInputs = {
  ...safePreRelease,
  licensePresent: true,
  licenseText:
    'GNU AFFERO GENERAL PUBLIC LICENSE\nVersion 3, 19 November 2007\n13. Remote Network Interaction',
  packageLicense: 'AGPL-3.0-only',
  agentBundle: '<Wix LicenseUrl="https://www.gnu.org/licenses/agpl-3.0.html" />',
  viewerBundle: '<Wix LicenseUrl="https://www.gnu.org/licenses/agpl-3.0.html" />',
  viewerLicense: 'AGPL-3.0-only https://www.gnu.org/licenses/agpl-3.0.html',
  agentX86Installer: installerWithNotices,
  agentX64Installer: installerWithNotices,
  viewerInstaller: installerWithNotices,
  readme: 'AGPL-3.0-only THIRD_PARTY_NOTICES.md',
  contributing: 'AGPL-3.0-only inbound-equals-outbound contribution terms.',
  thirdPartyNotices: "The root AGPL does not replace or alter a third party's licence.",
  provenanceInventory:
    'AGPL-3.0-only. The exact legal copyright-holder name has not been supplied.',
  readinessGuide: `Status: **licensed, but not yet cleared for publication**
exact legal copyright-holder name
corresponding-source URL
non-maintainer account`,
};

describe('publication readiness contract', () => {
  test('rejects a fake license and contact/legal claims before licensing', () => {
    const failures = publicationReadinessFailures({
      ...safePreRelease,
      agentBundle: '<Wix LicenseUrl="https://example.com/license" />',
      viewerLicense: "Use is subject to your organization's agreement with Talos.",
      contactPage: '<form>submit</form><script>console.log(message)</script>',
    });
    expect(failures).toContain('installer bundles must not publish a placeholder license URL');
    expect(failures).toContain(
      'the Viewer installer must not invent a proprietary Talos agreement',
    );
    expect(failures).toContain(
      'the Community contact page must not fake submission or log contact data',
    );
  });

  test('requires every publication surface to move together after licensing', () => {
    const failures = publicationReadinessFailures({
      ...safePreRelease,
      licensePresent: true,
    });
    expect(failures).toContain('a licensed repository package must declare AGPL-3.0-only');
    expect(failures).toContain('agent bundle must link to the canonical GNU AGPL version 3 text');
    expect(failures).toContain(
      'the Viewer installer must show the AGPL-3.0-only notice and canonical URL',
    );
    expect(failures).toContain(
      'Agent x64 MSI must install the project licence and third-party notices',
    );
  });

  test('rejects an unconfirmed repository URL and missing owner/contact blockers', () => {
    const failures = publicationReadinessFailures({
      ...safeLicensed,
      securityPolicy: 'Use https://github.com/Sebtek-Ltd/AssistAI/security/advisories/new',
      readinessGuide: 'Status: **licensed, but not yet cleared for publication**',
    });
    expect(failures).toContain(
      'governance files must not link to the unconfirmed private AssistAI repository',
    );
    expect(failures).toContain(
      'the readiness guide must retain the unresolved exact legal copyright-holder name blocker',
    );
    expect(failures).toContain(
      'security and conduct policies must use the repository-relative private report form',
    );
  });

  test('accepts the coherent licensed-state fixture', () => {
    expect(publicationReadinessFailures(safeLicensed)).toEqual([]);
  });

  test('confirmed identity replaces resolved blockers without dropping notice verification', () => {
    const confirmed: PublicationInputs = {
      ...safeLicensed,
      publicationIdentity: {
        schemaVersion: 1,
        copyrightHolder: 'Example Owner',
        sourceUrl: 'https://github.com/example/talos',
      },
      readme: `${safeLicensed.readme} Copyright Example Owner`,
      provenanceInventory:
        'AGPL-3.0-only. Copyright Example Owner. https://github.com/example/talos',
      readinessGuide:
        'Status: **licensed, but not yet cleared for publication**\nhttps://github.com/example/talos\nnon-maintainer account',
    };
    expect(publicationReadinessFailures(confirmed)).toEqual([]);
    expect(publicationReadinessFailures({ ...confirmed, readme: safeLicensed.readme })).toContain(
      'README and provenance inventory must name the confirmed copyright holder',
    );
    expect(
      publicationReadinessFailures({
        ...confirmed,
        provenanceInventory: safeLicensed.provenanceInventory,
      }),
    ).toContain('remove the resolved copyright-holder blocker after identity confirmation');
    expect(
      publicationReadinessFailures({ ...confirmed, publicationIdentity: { schemaVersion: 1 } }),
    ).toContain('publication identity must record a holder and canonical GitHub source URL');
  });

  test('the current tree is internally honest about publication state', async () => {
    const result = await checkPublicationReadinessContract();
    expect(result.mode).toBe('licensed');
    expect(result.failures).toEqual([]);
  });
});
