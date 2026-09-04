<script lang="ts">
  import { AlertTriangle, CheckCircle2, CircleDashed, Info, XCircle } from 'lucide-svelte';
  import type { FeatureUpgradePreflightCheckDefinition, FeatureUpgradePreflightCheckResult } from '$lib/types';

  export let title = 'Preflight checklist';
  export let checks: Array<FeatureUpgradePreflightCheckDefinition | FeatureUpgradePreflightCheckResult> = [];

  const fallbackChecks: FeatureUpgradePreflightCheckDefinition[] = [
    {
      id: 'edition_language',
      label: 'Edition and language compatibility',
      severity: 'required',
      description: 'Checks cached snapshot facts for edition and language evidence.'
    },
    {
      id: 'disk_space',
      label: 'System drive has at least 40 GB free',
      severity: 'required',
      description: 'Refreshes snapshot storage and reboot evidence before allowing ISO staging.',
      requiresFreshSnapshot: true
    },
    {
      id: 'pending_reboot',
      label: 'No pending reboot',
      severity: 'required',
      description: 'Uses current patch and snapshot reboot state already tracked by RMM.'
    },
    {
      id: 'bitlocker',
      label: 'BitLocker protection state captured',
      severity: 'warning',
      description: 'Refreshes BitLocker state and warns when protection needs review.',
      requiresFreshSnapshot: true
    }
  ];

  let expandedCheckIds = new Set<string>();
  $: items = checks.length > 0 ? checks : fallbackChecks;

  function statusOf(item: FeatureUpgradePreflightCheckDefinition | FeatureUpgradePreflightCheckResult) {
    return 'status' in item ? item.status : item.requiresFreshSnapshot ? 'pending' : 'not_applicable';
  }

  function messageOf(item: FeatureUpgradePreflightCheckDefinition | FeatureUpgradePreflightCheckResult) {
    if ('message' in item) return item.message;
    if (item.requiresFreshSnapshot) return 'Refreshes during preflight';
    return item.severity === 'warning' ? 'Warning check' : 'Required check';
  }

  function sourceLine(item: FeatureUpgradePreflightCheckDefinition | FeatureUpgradePreflightCheckResult) {
    if (!('sourceLabel' in item)) return item.requiresFreshSnapshot ? 'Evidence: fresh snapshot required' : 'Evidence: cached RMM telemetry';
    const timestamp = item.sourceUpdatedAt ? ` · ${formatDate(item.sourceUpdatedAt)}` : '';
    return `${item.sourceLabel ?? 'Evidence'}${timestamp}`;
  }

  function explanationOf(item: FeatureUpgradePreflightCheckDefinition | FeatureUpgradePreflightCheckResult) {
    const severity = item.severity === 'warning' ? 'Warning only' : 'Required';
    const freshness = item.requiresFreshSnapshot ? 'Refreshed during preflight.' : 'Uses cached RMM device or snapshot evidence.';
    return `${item.description ?? 'Preflight readiness check.'} ${severity}. ${freshness}`;
  }

  function formatDate(value: string) {
    const parsed = Date.parse(value);
    if (Number.isNaN(parsed)) return value;
    return new Intl.DateTimeFormat(undefined, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit'
    }).format(parsed);
  }

  function toggleInfo(id: string) {
    const next = new Set(expandedCheckIds);
    next.has(id) ? next.delete(id) : next.add(id);
    expandedCheckIds = next;
  }
</script>

<section class="preflight-checklist">
  <h3>{title}</h3>
  <ul>
    {#each items as item}
      {@const status = statusOf(item)}
      {@const explanation = explanationOf(item)}
      <li class:passed={status === 'passed'} class:failed={status === 'failed'} class:warning={status === 'warning'} class:pending={status === 'pending'}>
        <span class="status-icon" aria-hidden="true">
          {#if status === 'passed'}
            <CheckCircle2 size={15} />
          {:else if status === 'failed'}
            <XCircle size={15} />
          {:else if status === 'warning'}
            <AlertTriangle size={15} />
          {:else}
            <CircleDashed size={15} />
          {/if}
        </span>
        <span class="check-copy">
          <span class="label-row">
            <strong>{item.label}</strong>
            <span class="info-wrap">
              <button
                class="info-button"
                type="button"
                aria-label={`About ${item.label}`}
                aria-expanded={expandedCheckIds.has(item.id)}
                on:click={() => toggleInfo(item.id)}
              >
                <Info size={14} />
              </button>
              <span class="info-tooltip" role="tooltip">{explanation}</span>
            </span>
          </span>
          <small>{messageOf(item)}</small>
          <small class="source-line">{sourceLine(item)}</small>
          {#if expandedCheckIds.has(item.id)}
            <span class="inline-explanation">{explanation}</span>
          {/if}
        </span>
      </li>
    {/each}
  </ul>
</section>

<style>
  .preflight-checklist {
    overflow: visible;
    border: 1px solid rgba(105, 135, 180, 0.24);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.03);
    padding: 1rem;
  }

  .preflight-checklist h3 {
    margin: 0 0 0.75rem;
    font-size: 1.05rem;
  }

  ul {
    display: grid;
    gap: 0.75rem;
    margin: 0;
    padding: 0;
    list-style: none;
    color: rgb(180 202 235);
    font-size: 0.84rem;
  }

  li {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 0.55rem;
    align-items: flex-start;
  }

  .status-icon {
    display: inline-flex;
    margin-top: 0.1rem;
    color: rgb(145 164 198);
  }

  .check-copy {
    display: grid;
    gap: 0.18rem;
    min-width: 0;
  }

  .label-row {
    display: flex;
    min-width: 0;
    align-items: center;
    gap: 0.35rem;
  }

  strong {
    min-width: 0;
    color: rgb(212 229 255);
    font-weight: 800;
    line-height: 1.25;
  }

  small {
    color: rgb(145 164 198);
    line-height: 1.35;
  }

  .source-line {
    color: rgb(116 136 172);
  }

  .info-wrap {
    position: relative;
    display: inline-flex;
    flex: 0 0 auto;
  }

  .info-button {
    display: inline-flex;
    min-width: 1.45rem;
    min-height: 1.45rem;
    align-items: center;
    justify-content: center;
    border: 1px solid rgba(118, 190, 255, 0.32);
    border-radius: 999px;
    background: rgba(38, 92, 165, 0.18);
    color: rgb(145 203 255);
    padding: 0;
  }

  .info-tooltip {
    position: absolute;
    z-index: 20;
    top: calc(100% + 0.35rem);
    right: 0;
    display: none;
    width: min(20rem, 70vw);
    border: 1px solid rgba(118, 190, 255, 0.36);
    border-radius: 7px;
    background: rgb(9 18 42);
    box-shadow: 0 14px 34px rgba(0, 0, 0, 0.35);
    color: rgb(213 228 255);
    padding: 0.7rem;
    line-height: 1.4;
  }

  .info-wrap:hover .info-tooltip,
  .info-wrap:focus-within .info-tooltip {
    display: block;
  }

  .inline-explanation {
    margin-top: 0.3rem;
    border-left: 2px solid rgba(118, 190, 255, 0.45);
    color: rgb(187 207 238);
    padding-left: 0.6rem;
    line-height: 1.4;
  }

  li.passed .status-icon {
    color: rgb(98 230 170);
  }

  li.failed .status-icon {
    color: rgb(255 118 118);
  }

  li.warning .status-icon {
    color: rgb(255 205 92);
  }

  li.pending .status-icon {
    color: rgb(125 200 255);
  }
</style>
