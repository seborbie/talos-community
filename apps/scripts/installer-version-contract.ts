import { resolve } from 'node:path';

type InstallerProject = {
  projectPath: string;
  sourcePath: string;
};

type InstallerProduct = {
  cargoManifestPath: string;
  projects: readonly InstallerProject[];
};

export const installerProducts = {
  agent: {
    cargoManifestPath: 'talos_worker/Cargo.toml',
    projects: [
      {
        projectPath: 'installer/msi/Talos.Agent.x64.wixproj',
        sourcePath: 'installer/msi/Agent.x64.wxs',
      },
      {
        projectPath: 'installer/msi/Talos.Agent.x86.wixproj',
        sourcePath: 'installer/msi/Agent.x86.wxs',
      },
      {
        projectPath: 'installer/bundle/Talos.Agent.Bundle.wixproj',
        sourcePath: 'installer/bundle/Bundle.wxs',
      },
    ],
  },
  viewer: {
    cargoManifestPath: 'talos_viewer/src-tauri/Cargo.toml',
    projects: [
      {
        projectPath: 'installer/msi/Talos.Viewer.x64.wixproj',
        sourcePath: 'installer/msi/Viewer.x64.wxs',
      },
      {
        projectPath: 'installer/bundle/Talos.Viewer.Bundle.wixproj',
        sourcePath: 'installer/bundle/Viewer.Bundle.wxs',
      },
    ],
  },
} as const satisfies Record<string, InstallerProduct>;

export function parseCargoPackageVersion(contents: string, manifestPath: string): string {
  const packageSection = contents.match(/(?:^|\n)\s*\[package\]\s*\n([\s\S]*?)(?=\n\s*\[|$)/);
  if (!packageSection?.[1]) {
    throw new Error(`${manifestPath} does not contain a [package] section`);
  }

  const versions = [...packageSection[1].matchAll(/^\s*version\s*=\s*"([^"]+)"\s*(?:#.*)?$/gm)];
  if (versions.length !== 1 || !versions[0]?.[1]) {
    throw new Error(`${manifestPath} must contain exactly one literal package version`);
  }
  return versions[0][1];
}

export function windowsInstallerVersionFailures(version: string, product: string): string[] {
  const match = /^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.exec(version);
  if (!match) {
    return [
      `${product} Cargo version ${JSON.stringify(version)} must use the Windows Installer major.minor.build form`,
    ];
  }

  const components = match.slice(1).map(Number);
  const [major, minor, build] = components;
  if (
    major === undefined ||
    minor === undefined ||
    build === undefined ||
    !components.every(Number.isSafeInteger) ||
    major > 255 ||
    minor > 255 ||
    build > 65_535
  ) {
    return [
      `${product} Cargo version ${JSON.stringify(version)} exceeds Windows Installer limits ` +
        '(major/minor <= 255, build <= 65535)',
    ];
  }

  return [];
}

export function wixSourceVersionFailures(contents: string, sourcePath: string): string[] {
  const releaseElement = /<(Package|Bundle)\b[\s\S]*?>/.exec(contents);
  const releaseVersion = releaseElement?.[0].match(/\bVersion\s*=\s*"([^"]+)"/)?.[1];
  const versions = [...contents.matchAll(/\bVersion\s*=\s*"([^"]+)"/g)].map((match) => match[1]);
  if (!releaseElement?.[1] || releaseVersion !== '$(var.ProductVersion)' || versions.length !== 1) {
    return [
      `${sourcePath} must set its Package/Bundle Version to "$(var.ProductVersion)" and contain no hand-maintained product version`,
    ];
  }
  return [];
}

export function wixProjectSourceFailures(
  contents: string,
  projectPath: string,
  expectedSourceLeaf: string,
): string[] {
  const compileItems = [...contents.matchAll(/<Compile\s+Include="([^"]+)"\s*\/>/g)].map(
    (match) => match[1],
  );
  if (compileItems.length !== 1 || compileItems[0] !== expectedSourceLeaf) {
    return [
      `${projectPath} must compile only ${expectedSourceLeaf}; numbered or alternate authoring copies are not release inputs`,
    ];
  }
  return [];
}

export function buildTargetsFailures(contents: string): string[] {
  const failures: string[] = [];
  const productVersionDefinitions =
    contents.match(/ProductVersion=\$\(ProductVersion\)/g)?.length ?? 0;
  if (productVersionDefinitions !== 2) {
    failures.push(
      'installer/Directory.Build.targets must define ProductVersion once in each mutually exclusive branch',
    );
  }
  if (
    !contents.includes("Condition=\"'$(_ExistingInstallerDefineConstants)' == ''\"") ||
    !contents.includes("Condition=\"'$(_ExistingInstallerDefineConstants)' != ''\"")
  ) {
    failures.push(
      'installer/Directory.Build.targets must branch on a snapshot so ProductVersion is not appended twice',
    );
  }
  if (
    !contents.includes('Name="ValidateInstallerProductVersion"') ||
    !contents.includes('BeforeTargets="CoreCompile"') ||
    !contents.includes("Condition=\"'$(ProductVersion)' == ''\"")
  ) {
    failures.push('installer/Directory.Build.targets must reject builds that omit ProductVersion');
  }
  return failures;
}

export function buildScriptVersionFailures(contents: string): string[] {
  const failures: string[] = [];
  const requiredSnippets = [
    '$agentInstallerCargoTomlPath = Join-Path $appsRoot "talos_worker\\Cargo.toml"',
    '$viewerInstallerCargoTomlPath = Join-Path $appsRoot "talos_viewer\\src-tauri\\Cargo.toml"',
    '"-p:ProductVersion=$($wixBuild.ProductVersion)"',
    '"-p:ProductVersion=$agentInstallerVersion"',
  ];
  for (const snippet of requiredSnippets) {
    if (!contents.includes(snippet)) {
      failures.push(
        `scripts/build-installers.ps1 is missing required installer-version wiring: ${snippet}`,
      );
    }
  }

  const agentAssignments =
    contents.match(/^\s*ProductVersion\s*=\s*\$agentInstallerVersion\s*$/gm)?.length ?? 0;
  const viewerAssignments =
    contents.match(/^\s*ProductVersion\s*=\s*\$viewerInstallerVersion\s*$/gm)?.length ?? 0;
  if (agentAssignments !== 2) {
    failures.push(
      'scripts/build-installers.ps1 must pass the Agent Cargo version to both Agent MSI builds',
    );
  }
  if (viewerAssignments !== 1) {
    failures.push(
      'scripts/build-installers.ps1 must pass the Viewer Cargo version to the Viewer MSI build',
    );
  }

  return failures;
}

export type InstallerVersionContractResult = {
  failures: string[];
  versions: Record<keyof typeof installerProducts, string>;
};

export async function checkInstallerVersionContract(
  appsRoot = resolve(import.meta.dir, '..'),
  repoRoot = resolve(appsRoot, '..'),
): Promise<InstallerVersionContractResult> {
  const failures: string[] = [];
  const versions = {} as Record<keyof typeof installerProducts, string>;

  for (const [productName, product] of Object.entries(installerProducts) as [
    keyof typeof installerProducts,
    InstallerProduct,
  ][]) {
    const manifestContents = await Bun.file(resolve(appsRoot, product.cargoManifestPath)).text();
    try {
      const version = parseCargoPackageVersion(manifestContents, product.cargoManifestPath);
      versions[productName] = version;
      failures.push(...windowsInstallerVersionFailures(version, productName));
    } catch (error) {
      failures.push(error instanceof Error ? error.message : String(error));
      versions[productName] = 'invalid';
    }

    for (const project of product.projects) {
      const sourceContents = await Bun.file(resolve(appsRoot, project.sourcePath)).text();
      failures.push(...wixSourceVersionFailures(sourceContents, project.sourcePath));

      const projectContents = await Bun.file(resolve(appsRoot, project.projectPath)).text();
      const expectedSourceLeaf = project.sourcePath.split('/').at(-1);
      if (!expectedSourceLeaf) {
        failures.push(`Unable to resolve WiX source leaf for ${project.sourcePath}`);
      } else {
        failures.push(
          ...wixProjectSourceFailures(projectContents, project.projectPath, expectedSourceLeaf),
        );
      }
    }
  }

  const targetsContents = await Bun.file(
    resolve(appsRoot, 'installer/Directory.Build.targets'),
  ).text();
  failures.push(...buildTargetsFailures(targetsContents));

  const buildScriptContents = await Bun.file(
    resolve(repoRoot, 'scripts/build-installers.ps1'),
  ).text();
  failures.push(...buildScriptVersionFailures(buildScriptContents));

  return { failures, versions };
}

if (import.meta.main) {
  const result = await checkInstallerVersionContract();
  if (result.failures.length > 0) {
    console.error('Installer version contract check failed:\n');
    for (const failure of result.failures) console.error(`- ${failure}`);
    process.exit(1);
  }

  console.log(
    `Installer version contract passed (Agent ${result.versions.agent} from talos_worker; ` +
      `Viewer ${result.versions.viewer} from talos_viewer).`,
  );
}
