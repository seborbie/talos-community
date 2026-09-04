export const FEATURE_UPGRADE_PREFLIGHT_DISK_FREE_BYTES = 40 * 1024 * 1024 * 1024;
export const WINDOWS_11_FEATURE_UPGRADE_TARGET_VERSION = '25H2';
export const WINDOWS_SERVER_FEATURE_UPGRADE_TARGET_VERSION = '2025';
const WINDOWS_11_MEMORY_REQUIRED_BYTES = 4 * 1024 * 1024 * 1024;
const WINDOWS_11_SYSTEM_DISK_REQUIRED_BYTES = 64 * 1024 * 1024 * 1024;

export type FeatureUpgradePreflightTargetProfile = 'windows10_to_11' | 'windows11_feature' | 'server_to_2025';
export type FeatureUpgradePreflightCheckStatus = 'passed' | 'failed' | 'warning' | 'skipped' | 'not_applicable' | 'pending';
export type FeatureUpgradePreflightEvaluationMode = 'preview' | 'final';

export type FeatureUpgradePreflightCheckDefinition = {
  id: string;
  label: string;
  severity: 'required' | 'warning';
  appliesTo: FeatureUpgradePreflightTargetProfile[];
  description: string;
  requiresFreshSnapshot?: boolean;
};

export type FeatureUpgradePreflightCheckResult = {
  id: string;
  label: string;
  severity: 'required' | 'warning';
  status: FeatureUpgradePreflightCheckStatus;
  message: string;
  description: string;
  source: string | null;
  sourceLabel: string;
  sourceUpdatedAt: string | null;
  requiresFreshSnapshot: boolean;
  details?: Record<string, unknown> | null;
};

export type FeatureUpgradePreflightTarget = {
  profile: FeatureUpgradePreflightTargetProfile;
  targetProduct: string;
  targetVersion: string;
  targetBuildLabel: string;
  checks: FeatureUpgradePreflightCheckDefinition[];
};

export type FeatureUpgradePreflightTelemetryState = {
  collectedAt?: Date | string | null;
  osName?: string | null;
  osVersion?: string | null;
  cpuModel?: string | null;
  cpuPhysicalCores?: number | null;
  cpuLogicalCores?: number | null;
  cpuBaseMhz?: number | null;
  memoryTotalBytes?: bigint | number | null;
  pendingUpdatesCount?: number | null;
  rebootRequired?: boolean | null;
  inventoryData?: unknown;
};

export type FeatureUpgradePreflightFact = {
  factKey: string;
  factValue: unknown;
  source: string | null;
  sourceTs: Date | string | null;
  updatedAt?: Date | string | null;
};

export type FeatureUpgradePreflightDeviceEvidence = {
  os: string;
  osVersion?: string | null;
  state?: FeatureUpgradePreflightTelemetryState | null;
  facts?: Map<string, FeatureUpgradePreflightFact> | Record<string, FeatureUpgradePreflightFact> | null;
};

type CheckContext = {
  check: FeatureUpgradePreflightCheckDefinition;
  device: FeatureUpgradePreflightDeviceEvidence;
  target: FeatureUpgradePreflightTarget;
  mode: FeatureUpgradePreflightEvaluationMode;
  facts: Map<string, FeatureUpgradePreflightFact>;
  inventory: Record<string, unknown>;
};

type EvidenceValue<T> = {
  value: T | null;
  source: string | null;
  sourceLabel: string;
  sourceUpdatedAt: string | null;
  details?: Record<string, unknown>;
};

export const FEATURE_UPGRADE_PREFLIGHT_FACT_KEYS = [
  'os.name',
  'os.version',
  'os.architecture',
  'os.edition',
  'os.locale',
  'os.language',
  'security.reboot_required',
  'updates.pending_count',
  'security.bitlocker_enabled',
  'security.bitlocker_protection_status',
  'security.secure_boot_enabled',
  'security.tpm_present',
  'security.tpm_enabled',
  'security.tpm_version',
  'hardware.memory.total_bytes'
];

export const FEATURE_UPGRADE_PREFLIGHT_CHECKS: FeatureUpgradePreflightCheckDefinition[] = [
  {
    id: 'os_supported',
    label: 'Supported source and target upgrade path',
    severity: 'required',
    appliesTo: ['windows10_to_11', 'windows11_feature', 'server_to_2025'],
    description: 'Validates the current Windows family and release against the automatic target path Talos will plan for this device.'
  },
  {
    id: 'architecture',
    label: '64-bit architecture compatibility',
    severity: 'required',
    appliesTo: ['windows10_to_11', 'windows11_feature', 'server_to_2025'],
    description: 'Checks the last snapshot operating-system architecture because Windows feature upgrades in this flow require x64 media.'
  },
  {
    id: 'edition_language',
    label: 'Edition and language compatibility',
    severity: 'required',
    appliesTo: ['windows10_to_11', 'windows11_feature', 'server_to_2025'],
    description: 'Confirms the snapshot captured edition and locale so the correct Talos-provided ISO can be matched later.'
  },
  {
    id: 'disk_space',
    label: 'System drive has at least 40 GB free',
    severity: 'required',
    appliesTo: ['windows10_to_11', 'windows11_feature', 'server_to_2025'],
    description: 'Refreshes a full snapshot during preflight and checks the system volume has at least 40 GB free before staging media.',
    requiresFreshSnapshot: true
  },
  {
    id: 'pending_reboot',
    label: 'No pending reboot',
    severity: 'required',
    appliesTo: ['windows10_to_11', 'windows11_feature', 'server_to_2025'],
    description: 'Uses the current patch and snapshot reboot state already tracked by RMM; no custom preflight registry scan is run.'
  },
  {
    id: 'tpm_2_0',
    label: 'TPM 2.0 present, enabled, and ready',
    severity: 'required',
    appliesTo: ['windows10_to_11'],
    description: 'Uses snapshot hardware/security facts to check the Windows 11 TPM 2.0 baseline for Windows 10 to Windows 11 upgrades.'
  },
  {
    id: 'secure_boot',
    label: 'Secure Boot enabled',
    severity: 'required',
    appliesTo: ['windows10_to_11'],
    description: 'Uses the current snapshot Secure Boot fact for Windows 10 to Windows 11 readiness.'
  },
  {
    id: 'cpu_basic',
    label: 'Basic Windows 11 CPU baseline',
    severity: 'required',
    appliesTo: ['windows10_to_11'],
    description: 'Checks cached CPU architecture, core count, and approximate clock speed. The full Microsoft CPU allowlist remains deferred to setup/appraiser.'
  },
  {
    id: 'memory',
    label: 'At least 4 GB RAM',
    severity: 'required',
    appliesTo: ['windows10_to_11'],
    description: 'Uses hardware memory from the latest snapshot/current state and requires at least 4 GB RAM for Windows 11.'
  },
  {
    id: 'system_disk_size',
    label: 'System disk is at least 64 GB',
    severity: 'required',
    appliesTo: ['windows10_to_11'],
    description: 'Uses disk inventory from the latest snapshot to confirm the system disk is at least 64 GB.'
  },
  {
    id: 'bitlocker',
    label: 'BitLocker protection state captured',
    severity: 'warning',
    appliesTo: ['windows10_to_11', 'windows11_feature', 'server_to_2025'],
    description: 'Refreshes BitLocker state during preflight. Protected volumes are warnings so technicians can suspend protection before upgrade.',
    requiresFreshSnapshot: true
  },
  {
    id: 'domain_controller',
    label: 'Domain controller / AD prep warning',
    severity: 'warning',
    appliesTo: ['server_to_2025'],
    description: 'Uses snapshot server-role evidence to warn when the server is a domain controller and needs forest/domain prep planning.'
  }
];

export function featureUpgradeChecksForProfile(profile: FeatureUpgradePreflightTargetProfile) {
  return FEATURE_UPGRADE_PREFLIGHT_CHECKS.filter((check) => check.appliesTo.includes(profile));
}

export function inferFeatureUpgradePreflightTarget(os: string): FeatureUpgradePreflightTarget | null {
  const lower = os.toLowerCase();
  if (!/\bwindows\b/.test(lower)) return null;
  if (lower.includes('server')) {
    return {
      profile: 'server_to_2025',
      targetProduct: 'Windows Server',
      targetVersion: WINDOWS_SERVER_FEATURE_UPGRADE_TARGET_VERSION,
      targetBuildLabel: 'Windows Server 2025',
      checks: featureUpgradeChecksForProfile('server_to_2025')
    };
  }
  if (lower.includes('windows 10')) {
    return {
      profile: 'windows10_to_11',
      targetProduct: 'Windows 11',
      targetVersion: WINDOWS_11_FEATURE_UPGRADE_TARGET_VERSION,
      targetBuildLabel: `Windows 11 ${WINDOWS_11_FEATURE_UPGRADE_TARGET_VERSION}`,
      checks: featureUpgradeChecksForProfile('windows10_to_11')
    };
  }
  if (lower.includes('windows 11')) {
    return {
      profile: 'windows11_feature',
      targetProduct: 'Windows 11',
      targetVersion: WINDOWS_11_FEATURE_UPGRADE_TARGET_VERSION,
      targetBuildLabel: `Windows 11 ${WINDOWS_11_FEATURE_UPGRADE_TARGET_VERSION}`,
      checks: featureUpgradeChecksForProfile('windows11_feature')
    };
  }
  return null;
}

export function readFeatureUpgradeAgentIds(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return [...new Set(value.map((item) => (typeof item === 'string' ? item.trim() : '')).filter(Boolean))];
}

export function evaluateFeatureUpgradePreflightChecks(input: {
  device: FeatureUpgradePreflightDeviceEvidence;
  target: FeatureUpgradePreflightTarget;
  mode: FeatureUpgradePreflightEvaluationMode;
}): FeatureUpgradePreflightCheckResult[] {
  const facts = normalizeFactMap(input.device.facts);
  const inventory = asRecord(input.device.state?.inventoryData);
  return input.target.checks.map((check) => {
    const context: CheckContext = {
      check,
      device: input.device,
      target: input.target,
      mode: input.mode,
      facts,
      inventory
    };
    if (check.requiresFreshSnapshot && input.mode === 'preview') {
      return pendingFreshSnapshot(context);
    }
    switch (check.id) {
      case 'os_supported':
        return evaluateOsSupported(context);
      case 'architecture':
        return evaluateArchitecture(context);
      case 'edition_language':
        return evaluateEditionLanguage(context);
      case 'disk_space':
        return evaluateDiskSpace(context);
      case 'pending_reboot':
        return evaluatePendingReboot(context);
      case 'tpm_2_0':
        return evaluateTpm(context);
      case 'secure_boot':
        return evaluateSecureBoot(context);
      case 'cpu_basic':
        return evaluateCpuBasic(context);
      case 'memory':
        return evaluateMemory(context);
      case 'system_disk_size':
        return evaluateSystemDiskSize(context);
      case 'bitlocker':
        return evaluateBitLocker(context);
      case 'domain_controller':
        return evaluateDomainController(context);
      default:
        return result(context, 'skipped', 'Unknown preflight check', snapshotEvidence(input.device.state), null);
    }
  });
}

export function aggregateFeatureUpgradePreflightStatus(checks: FeatureUpgradePreflightCheckResult[]) {
  if (checks.some((check) => check.severity === 'required' && check.status === 'failed')) return 'failed';
  if (checks.some((check) => check.status === 'warning')) return 'warning';
  if (checks.some((check) => check.status === 'pending')) return 'running';
  return 'passed';
}

export function summarizeFeatureUpgradePreflightChecks(checks: unknown, status: 'failed' | 'warning') {
  const items = Array.isArray(checks) ? checks : [];
  return items
    .filter((item) => {
      const record = asRecord(item);
      return record.status === status;
    })
    .map((item) => {
      const record = asRecord(item);
      return {
        id: readString(record.id),
        label: readString(record.label),
        message: readString(record.message)
      };
    });
}

function evaluateOsSupported(context: CheckContext) {
  const osText = readString(context.device.state?.osName) ?? context.device.os;
  const lower = osText.toLowerCase();
  const source = deviceRecordEvidence();
  if (context.target.profile === 'server_to_2025') {
    const year = serverYear(osText) ?? serverYear(context.device.os);
    const joined = `${osText} ${context.device.os}`.toLowerCase();
    const supported = matchesServerUpgradePath(year, joined);
    return passFail(
      context,
      supported,
      `${osText} has a supported in-place path to Windows Server 2025`,
      `${osText} does not have a supported direct path to Windows Server 2025`,
      source,
      { sourceOs: context.device.os, detectedOs: osText, target: context.target.targetBuildLabel }
    );
  }
  if (context.target.profile === 'windows10_to_11') {
    return passFail(
      context,
      lower.includes('windows 10') || context.device.os.toLowerCase().includes('windows 10'),
      'Windows 10 source detected for Windows 11 25H2',
      'Expected a Windows 10 source device for this Windows 11 upgrade path',
      source,
      { sourceOs: context.device.os, detectedOs: osText, target: context.target.targetBuildLabel }
    );
  }
  return passFail(
    context,
    lower.includes('windows 11') || context.device.os.toLowerCase().includes('windows 11'),
    'Windows 11 source detected for feature upgrade',
    'Expected a Windows 11 source device for this feature upgrade path',
    source,
    { sourceOs: context.device.os, detectedOs: osText, target: context.target.targetBuildLabel }
  );
}

function evaluateArchitecture(context: CheckContext) {
  const architecture = firstString(
    valueAtPath(context.inventory, ['operating_system', 'system', 'architecture']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'os', 'architecture']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'osArchitecture']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'architecture']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'os', 'architecture']),
    factValue(context, 'os.architecture'),
    valueAtPath(context.inventory, ['system', 'architecture']),
    valueAtPath(context.inventory, ['system', 'os', 'architecture']),
    valueAtPath(context.inventory, ['hardware', 'cpu', 'architecture'])
  );
  return passFailUnknown(
    context,
    architecture,
    (value) => isX64Text(value),
    '64-bit Windows architecture detected',
    'Feature upgrades require 64-bit Windows architecture',
    snapshotEvidence(context.device.state),
    { architecture }
  );
}

function evaluateEditionLanguage(context: CheckContext) {
  const edition = firstString(
    valueAtPath(context.inventory, ['operating_system', 'system', 'edition']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'os', 'edition']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'edition']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'os', 'edition']),
    factValue(context, 'os.edition'),
    valueAtPath(context.inventory, ['system', 'edition'])
  );
  const locale = firstString(
    valueAtPath(context.inventory, ['operating_system', 'system', 'locale']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'language']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'os', 'locale']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'os', 'language']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'locale']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'language']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'os', 'locale']),
    valueAtPath(context.inventory, ['operatingSystem', 'system', 'os', 'language']),
    factValue(context, 'os.locale'),
    factValue(context, 'os.language'),
    valueAtPath(context.inventory, ['system', 'locale'])
  );
  return passFailUnknown(
    context,
    edition || locale ? { edition, locale } : null,
    () => Boolean(edition && locale),
    'Edition and locale are available for target media matching',
    'Edition or locale was not captured; target media cannot be matched confidently',
    snapshotEvidence(context.device.state),
    { edition, locale }
  );
}

function evaluateDiskSpace(context: CheckContext) {
  const volume = systemVolume(context.inventory);
  const freeBytes = readNumber(volume?.free_bytes ?? volume?.freeBytes);
  return passFailUnknown(
    context,
    freeBytes,
    (value) => value >= FEATURE_UPGRADE_PREFLIGHT_DISK_FREE_BYTES,
    'System drive has at least 40 GB free',
    'System drive has less than 40 GB free',
    freshSnapshotEvidence(context.device.state),
    {
      driveLetter: readString(volume?.drive_letter ?? volume?.driveLetter),
      freeBytes,
      requiredBytes: FEATURE_UPGRADE_PREFLIGHT_DISK_FREE_BYTES
    }
  );
}

function evaluatePendingReboot(context: CheckContext) {
  const fact = context.facts.get('security.reboot_required');
  const factValue = fact ? readBoolean(fact.factValue) : null;
  const stateValue = context.device.state?.rebootRequired ?? null;
  const pendingReboot = stateValue ?? factValue;
  const evidence = stateValue !== null && stateValue !== undefined
    ? patchStateEvidence(context.device.state)
    : factEvidence(fact, 'Current patch/snapshot state');
  return passFailUnknown(
    context,
    pendingReboot,
    (value) => value === false,
    'No pending reboot detected in current RMM state',
    'A pending reboot must be cleared before feature upgrade',
    evidence,
    {
      rebootRequired: pendingReboot,
      pendingUpdatesCount: context.device.state?.pendingUpdatesCount ?? null
    }
  );
}

function evaluateTpm(context: CheckContext) {
  const present = readBooleanFromFactOrInventory(context, 'security.tpm_present', ['hardware', 'tpm', 'present']);
  const enabled = readBooleanFromFactOrInventory(context, 'security.tpm_enabled', ['hardware', 'tpm', 'enabled']);
  const ready = readBoolean(valueAtPath(context.inventory, ['hardware', 'tpm', 'ready']));
  const version = readString(factValue(context, 'security.tpm_version')) ?? firstString(valueAtPath(context.inventory, ['hardware', 'tpm', 'version']));
  const hasEvidence = present.value !== null || enabled.value !== null || ready !== null || version !== null;
  const evidence = present.evidence.source ? present.evidence : snapshotEvidence(context.device.state);
  const spec20 = version?.split(',').some((part) => part.trim().startsWith('2.0')) ?? false;
  return passFailUnknown(
    context,
    hasEvidence ? { present: present.value, enabled: enabled.value, ready, version } : null,
    () => present.value === true && enabled.value === true && ready === true && spec20,
    'TPM 2.0 is present, enabled, and ready',
    'TPM 2.0 is missing, disabled, not ready, or not reported',
    evidence,
    { present: present.value, enabled: enabled.value, ready, version }
  );
}

function evaluateSecureBoot(context: CheckContext) {
  const secureBoot = readBooleanFromFactOrInventory(context, 'security.secure_boot_enabled', ['hardware', 'secure_boot']);
  return passFailUnknown(
    context,
    secureBoot.value,
    (value) => value === true,
    'Secure Boot is enabled',
    'Secure Boot is not enabled or could not be verified',
    secureBoot.evidence,
    { secureBootEnabled: secureBoot.value }
  );
}

function evaluateCpuBasic(context: CheckContext) {
  const cores = context.device.state?.cpuPhysicalCores ?? readNumber(valueAtPath(context.inventory, ['hardware', 'cpu', 'cores']));
  const clockMhz = context.device.state?.cpuBaseMhz ?? readNumber(valueAtPath(context.inventory, ['hardware', 'cpu', 'frequency_mhz']));
  const architecture = firstString(
    valueAtPath(context.inventory, ['hardware', 'cpu', 'architecture']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'architecture']),
    valueAtPath(context.inventory, ['operating_system', 'system', 'os', 'architecture']),
    factValue(context, 'os.architecture')
  );
  const hasEvidence = cores !== null || clockMhz !== null || architecture !== null;
  return passFailUnknown(
    context,
    hasEvidence ? { cores, clockMhz, architecture } : null,
    () => (cores ?? 0) >= 2 && (clockMhz ?? 0) >= 1000 && (architecture ? isX64Text(architecture) : true),
    'CPU meets the basic Windows 11 local baseline',
    'CPU does not meet the basic Windows 11 local baseline',
    snapshotEvidence(context.device.state),
    { cpuModel: context.device.state?.cpuModel ?? null, cores, clockMhz, architecture }
  );
}

function evaluateMemory(context: CheckContext) {
  const fact = context.facts.get('hardware.memory.total_bytes');
  const memoryBytes = normalizeBigNumberish(context.device.state?.memoryTotalBytes) ?? readNumber(fact?.factValue) ?? readNumber(valueAtPath(context.inventory, ['hardware', 'memory', 'total_bytes']));
  const evidence = context.device.state?.memoryTotalBytes !== null && context.device.state?.memoryTotalBytes !== undefined
    ? snapshotEvidence(context.device.state)
    : factEvidence(fact, 'Current hardware fact');
  return passFailUnknown(
    context,
    memoryBytes,
    (value) => value >= WINDOWS_11_MEMORY_REQUIRED_BYTES,
    'At least 4 GB RAM detected',
    'Windows 11 requires at least 4 GB RAM',
    evidence,
    { memoryTotalBytes: memoryBytes, requiredBytes: WINDOWS_11_MEMORY_REQUIRED_BYTES }
  );
}

function evaluateSystemDiskSize(context: CheckContext) {
  const volume = systemVolume(context.inventory);
  const disk = systemDisk(context.inventory, volume);
  const totalBytes = readNumber(volume?.total_bytes ?? volume?.totalBytes) ?? readNumber(disk?.size_bytes ?? disk?.sizeBytes);
  return passFailUnknown(
    context,
    totalBytes,
    (value) => value >= WINDOWS_11_SYSTEM_DISK_REQUIRED_BYTES,
    'System disk is at least 64 GB',
    'Windows 11 requires at least 64 GB system storage',
    snapshotEvidence(context.device.state),
    { totalBytes, requiredBytes: WINDOWS_11_SYSTEM_DISK_REQUIRED_BYTES }
  );
}

function evaluateBitLocker(context: CheckContext) {
  const bitlocker = asRecord(valueAtPath(context.inventory, ['security', 'bitlocker']));
  const enabled = readBoolean(bitlocker.enabled) ?? readBoolean(factValue(context, 'security.bitlocker_enabled'));
  const systemDrive = readString(systemVolume(context.inventory)?.drive_letter ?? systemVolume(context.inventory)?.driveLetter) ?? 'C:';
  const volumes = Array.isArray(bitlocker.volumes) ? bitlocker.volumes.map(asRecord) : [];
  const volume = volumes.find((item) => sameDrive(readString(item.drive_letter ?? item.driveLetter), systemDrive)) ?? volumes[0] ?? null;
  const protectionStatus = firstString(
    volume?.protection_status,
    volume?.protectionStatus,
    factValue(context, 'security.bitlocker_protection_status')
  );
  const isProtected = enabled === true || protectionStatus?.toLowerCase() === 'protected';
  if (enabled === null && !protectionStatus) {
    return result(
      context,
      context.mode === 'preview' ? 'pending' : 'warning',
      context.mode === 'preview'
        ? 'BitLocker status will be refreshed during preflight'
        : 'BitLocker state was not captured; review manually before staging',
      freshSnapshotEvidence(context.device.state),
      { enabled, protectionStatus, systemDrive }
    );
  }
  return result(
    context,
    isProtected ? 'warning' : 'passed',
    isProtected
      ? 'BitLocker protection should be reviewed or suspended before upgrade'
      : 'BitLocker does not appear to require suspension',
    freshSnapshotEvidence(context.device.state),
    { enabled, protectionStatus, systemDrive }
  );
}

function evaluateDomainController(context: CheckContext) {
  const adDs = asRecord(valueAtPath(context.inventory, ['operating_system', 'ad_ds']));
  const isDc = readBoolean(adDs.is_domain_controller ?? adDs.isDomainController);
  if (isDc === null) {
    return result(
      context,
      'skipped',
      'Domain controller role evidence is not present in the latest snapshot',
      snapshotEvidence(context.device.state),
      null
    );
  }
  return result(
    context,
    isDc ? 'warning' : 'passed',
    isDc
      ? 'Domain controller detected; forest/domain prep handling is required during install planning'
      : 'Domain controller role not detected',
    snapshotEvidence(context.device.state),
    { isDomainController: isDc, domainName: readString(adDs.domain_name ?? adDs.domainName) }
  );
}

function pendingFreshSnapshot(context: CheckContext) {
  return result(
    context,
    'pending',
    'Will be refreshed by the full snapshot collected when preflight is confirmed',
    freshSnapshotEvidence(context.device.state),
    null
  );
}

function passFail<T>(
  context: CheckContext,
  passed: boolean,
  passedMessage: string,
  failedMessage: string,
  evidence: EvidenceValue<unknown>,
  details: Record<string, unknown> | null
) {
  return result(context, passed ? 'passed' : 'failed', passed ? passedMessage : failedMessage, evidence, details);
}

function passFailUnknown<T>(
  context: CheckContext,
  value: T | null | undefined,
  predicate: (value: T) => boolean,
  passedMessage: string,
  failedMessage: string,
  evidence: EvidenceValue<unknown>,
  details: Record<string, unknown> | null
) {
  if (value === null || value === undefined) {
    return result(
      context,
      context.mode === 'preview' ? 'pending' : context.check.severity === 'warning' ? 'warning' : 'failed',
      context.mode === 'preview'
        ? 'Waiting for snapshot evidence from the existing RMM telemetry path'
        : context.check.severity === 'warning'
          ? 'Evidence was not captured; review manually'
          : 'Required evidence was not captured by the latest snapshot',
      evidence,
      details
    );
  }
  return passFail(context, predicate(value), passedMessage, failedMessage, evidence, details);
}

function result(
  context: CheckContext,
  status: FeatureUpgradePreflightCheckStatus,
  message: string,
  evidence: EvidenceValue<unknown>,
  details: Record<string, unknown> | null
): FeatureUpgradePreflightCheckResult {
  return {
    id: context.check.id,
    label: context.check.label,
    severity: context.check.severity,
    status,
    message,
    description: context.check.description,
    source: evidence.source,
    sourceLabel: evidence.sourceLabel,
    sourceUpdatedAt: evidence.sourceUpdatedAt,
    requiresFreshSnapshot: context.check.requiresFreshSnapshot === true,
    details
  };
}

function normalizeFactMap(facts: FeatureUpgradePreflightDeviceEvidence['facts']) {
  if (facts instanceof Map) return facts;
  const map = new Map<string, FeatureUpgradePreflightFact>();
  if (facts && typeof facts === 'object') {
    for (const [key, value] of Object.entries(facts)) {
      map.set(key, value);
    }
  }
  return map;
}

function factValue(context: CheckContext, factKey: string) {
  return context.facts.get(factKey)?.factValue;
}

function factEvidence(fact: FeatureUpgradePreflightFact | undefined, fallbackLabel: string): EvidenceValue<unknown> {
  return {
    value: fact?.factValue ?? null,
    source: fact?.source ?? null,
    sourceLabel: fact ? fallbackLabel : 'No cached evidence',
    sourceUpdatedAt: isoDate(fact?.sourceTs ?? fact?.updatedAt ?? null)
  };
}

function snapshotEvidence(state?: FeatureUpgradePreflightTelemetryState | null): EvidenceValue<unknown> {
  return {
    value: null,
    source: 'snapshot',
    sourceLabel: state?.collectedAt ? 'Latest snapshot' : 'No snapshot evidence',
    sourceUpdatedAt: isoDate(state?.collectedAt ?? null)
  };
}

function freshSnapshotEvidence(state?: FeatureUpgradePreflightTelemetryState | null): EvidenceValue<unknown> {
  return {
    value: null,
    source: 'snapshot',
    sourceLabel: 'Fresh preflight snapshot',
    sourceUpdatedAt: isoDate(state?.collectedAt ?? null)
  };
}

function patchStateEvidence(state?: FeatureUpgradePreflightTelemetryState | null): EvidenceValue<unknown> {
  return {
    value: null,
    source: 'patch_state',
    sourceLabel: 'Current patch/snapshot state',
    sourceUpdatedAt: isoDate(state?.collectedAt ?? null)
  };
}

function deviceRecordEvidence(): EvidenceValue<unknown> {
  return {
    value: null,
    source: 'device_record',
    sourceLabel: 'Device record',
    sourceUpdatedAt: null
  };
}

function readBooleanFromFactOrInventory(context: CheckContext, factKey: string, inventoryPath: string[]) {
  const fact = context.facts.get(factKey);
  const factResult = readBoolean(fact?.factValue);
  if (factResult !== null) {
    return { value: factResult, evidence: factEvidence(fact, 'Current hardware fact') };
  }
  return { value: readBoolean(valueAtPath(context.inventory, inventoryPath)), evidence: snapshotEvidence(context.device.state) };
}

function systemVolume(inventory: Record<string, unknown>) {
  const disks = readArray(valueAtPath(inventory, ['hardware', 'disks']));
  for (const disk of disks) {
    for (const volume of readArray(asRecord(disk).volumes)) {
      const record = asRecord(volume);
      const driveLetter = readString(record.drive_letter ?? record.driveLetter);
      if (stringEquals(driveLetter, 'C:')) return record;
    }
  }
  for (const disk of disks) {
    const first = readArray(asRecord(disk).volumes)[0];
    if (first) return asRecord(first);
  }
  return null;
}

function systemDisk(inventory: Record<string, unknown>, volume: Record<string, unknown> | null) {
  const disks = readArray(valueAtPath(inventory, ['hardware', 'disks'])).map(asRecord);
  if (!volume) return disks[0] ?? null;
  const driveLetter = readString(volume.drive_letter ?? volume.driveLetter);
  return disks.find((disk) => readArray(disk.volumes).some((item) => stringEquals(readString(asRecord(item).drive_letter ?? asRecord(item).driveLetter), driveLetter))) ?? disks[0] ?? null;
}

function valueAtPath(value: unknown, path: string[]): unknown {
  let current: unknown = value;
  for (const key of path) {
    const record = asRecord(current);
    if (!(key in record)) return undefined;
    current = record[key];
  }
  return current;
}

function asRecord(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

function readArray(value: unknown): unknown[] {
  return Array.isArray(value) ? value : [];
}

function readString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function firstString(...values: unknown[]) {
  for (const value of values) {
    const text = readString(value);
    if (text) return text;
  }
  return null;
}

function readNumber(value: unknown): number | null {
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  if (typeof value === 'bigint') return Number(value);
  if (typeof value === 'string' && value.trim()) {
    const parsed = Number(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function normalizeBigNumberish(value: bigint | number | null | undefined) {
  if (typeof value === 'bigint') return Number(value);
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function readBoolean(value: unknown): boolean | null {
  if (typeof value === 'boolean') return value;
  if (typeof value === 'number') return value !== 0;
  if (typeof value === 'string') {
    const lower = value.trim().toLowerCase();
    if (['true', 'yes', '1', 'on', 'protected'].includes(lower)) return true;
    if (['false', 'no', '0', 'off', 'unprotected'].includes(lower)) return false;
  }
  return null;
}

function isX64Text(value: string) {
  const lower = value.toLowerCase();
  return lower.includes('64') || lower.includes('x64') || lower.includes('amd64');
}

function stringEquals(left: string | null | undefined, right: string | null | undefined) {
  if (!left || !right) return false;
  return left.toLowerCase() === right.toLowerCase();
}

function sameDrive(left: string | null | undefined, right: string | null | undefined) {
  const normalize = (value: string | null | undefined) => {
    if (!value) return null;
    const trimmed = value.trim().toLowerCase();
    const driveMatch = trimmed.match(/^([a-z]):/);
    return driveMatch ? `${driveMatch[1]}:` : trimmed;
  };
  const normalizedLeft = normalize(left);
  const normalizedRight = normalize(right);
  return Boolean(normalizedLeft && normalizedRight && normalizedLeft === normalizedRight);
}

function serverYear(value: string): number | null {
  const match = value.match(/\b(2025|2022|2019|2016|2012|2008)\b/);
  return match ? Number(match[1]) : null;
}

function matchesServerUpgradePath(year: number | null, osText: string) {
  return matchesNumber(year, [2016, 2019, 2022, 2025]) || (year === 2012 && osText.includes('r2'));
}

function matchesNumber(value: number | null, candidates: number[]) {
  return value !== null && candidates.includes(value);
}

function isoDate(value: Date | string | null | undefined) {
  if (!value) return null;
  if (value instanceof Date) return value.toISOString();
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : new Date(parsed).toISOString();
}
