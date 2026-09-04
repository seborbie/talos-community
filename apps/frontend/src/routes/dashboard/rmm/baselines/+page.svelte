<script lang="ts">
  import { onDestroy, onMount } from 'svelte';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';
  import Button from '$lib/ui/Button.svelte';
  import Input from '$lib/ui/Input.svelte';
  import Label from '$lib/ui/Label.svelte';
  import Table from '$lib/ui/Table.svelte';
  import TableBody from '$lib/ui/TableBody.svelte';
  import TableCell from '$lib/ui/TableCell.svelte';
  import TableHead from '$lib/ui/TableHead.svelte';
  import TableHeader from '$lib/ui/TableHeader.svelte';
  import TableRow from '$lib/ui/TableRow.svelte';
  import { RefreshCw, AlertCircle, Plus, Trash2, Edit, Zap, ShieldCheck, Settings, GitBranch } from 'lucide-svelte';
  import { rmmApi } from '$lib/api';
  import type {
    RmmTelemetryBaselineScopeCatalogResponse,
    RmmTelemetryBaselineScopeType,
    RmmTelemetryScopedBaseline,
    RmmTelemetryScopedBaselineSummaryResponse,
    RmmTelemetryScopedBaselineDriftListResponse,
    RmmTelemetryDecision,
    RmmTelemetryIntent,
    RmmTelemetryStabilityOverride,
    RmmTelemetryStabilityOverridePreviewResponse,
    RmmTelemetryRoutingRule,
    RmmTelemetryRoutingMatchOperator,
    RmmTelemetryRoutingAction,
    RmmTelemetryRoutingRuleTestResponse,
    RmmTelemetryRoutingTestCandidate
  } from '$lib/types';

  type ScopeOption = {
    id: string;
    name: string;
    subtitle?: string | null;
  };

  const scopeTypeOptions: Array<{ value: RmmTelemetryBaselineScopeType; label: string }> = [
    { value: 'organization', label: 'Organization' },
    { value: 'customer', label: 'Customer' },
    { value: 'site', label: 'Site' },
    { value: 'device', label: 'Device' }
  ];

  let catalogLoading = true;
  let catalogError: string | null = null;
  let catalog: RmmTelemetryBaselineScopeCatalogResponse | null = null;

  let selectedScopeType: RmmTelemetryBaselineScopeType = 'organization';
  let selectedScopeId = '';
  let factKeyFilter = '';
  let onlyUnstable = false;
  let onlyOverridden = false;
  let onlyIgnored = false;

  let baselinesLoading = false;
  let baselinesError: string | null = null;
  let baselines: RmmTelemetryScopedBaseline[] = [];
  let selectedBaseline: RmmTelemetryScopedBaseline | null = null;

  let summaryLoading = false;
  let summaryError: string | null = null;
  let summary: RmmTelemetryScopedBaselineSummaryResponse['summary'] | null = null;

  let selectedFactKey = '';
  let driftLoading = false;
  let driftError: string | null = null;
  let driftItems: RmmTelemetryScopedBaselineDriftListResponse['items'] = [];

  const toDisplayJson = (value: unknown, max = 120): string => {
    const rendered = JSON.stringify(value ?? null);
    if (rendered.length <= max) return rendered;
    return `${rendered.slice(0, max - 3)}...`;
  };

  const percent = (value: number): string => `${(value * 100).toFixed(1)}%`;

  const dateLabel = (value: string | null): string => {
    if (!value) return '—';
    const parsed = Date.parse(value);
    if (Number.isNaN(parsed)) return '—';
    return new Date(parsed).toLocaleString();
  };

  const round = (value: number): string => Number.isFinite(value) ? value.toFixed(3) : '0.000';

  const promotionStateLabel = (value: string): string => {
    switch (value) {
      case 'stable_baseline':
        return 'Stable baseline';
      case 'candidate':
        return 'Candidate';
      case 'tracked_as_noisy':
        return 'Tracked as noisy';
      case 'suppressed_by_override':
        return 'Ignored by override';
      case 'missing_device_baseline':
        return 'Missing device baseline';
      case 'drifting_from_scope':
        return 'Drifting from scope';
      default:
        return 'Pending';
    }
  };

  const promotionStateClass = (value: string): string => {
    switch (value) {
      case 'stable_baseline':
        return 'bg-green-100 text-green-800';
      case 'candidate':
        return 'bg-blue-100 text-blue-800';
      case 'tracked_as_noisy':
        return 'bg-amber-100 text-amber-800';
      case 'suppressed_by_override':
        return 'bg-slate-200 text-slate-700';
      case 'missing_device_baseline':
      case 'drifting_from_scope':
        return 'bg-rose-100 text-rose-800';
      default:
        return 'bg-gray-100 text-gray-700';
    }
  };

  const stabilityClassClass = (value: string | null | undefined): string => {
    switch (value) {
      case 'stable':
        return 'bg-green-100 text-green-800';
      case 'noisy':
        return 'bg-amber-100 text-amber-800';
      case 'ignored':
        return 'bg-slate-200 text-slate-700';
      default:
        return 'bg-gray-100 text-gray-700';
    }
  };

  const warningLabel = (value: string): string => {
    switch (value) {
      case 'not_baseline_eligible':
        return 'Not baseline eligible';
      case 'ignored_by_override':
        return 'Ignored by override';
      case 'overridden_noisy':
        return 'Noisy by override';
      case 'pending_promotion':
        return 'Pending promotion';
      case 'low_sample_size':
        return 'Low sample size';
      case 'low_support_ratio':
        return 'Low support ratio';
      default:
        return value.replaceAll('_', ' ');
    }
  };

  const warningClass = (value: string): string => {
    switch (value) {
      case 'ignored_by_override':
      case 'not_baseline_eligible':
        return 'bg-slate-200 text-slate-700';
      case 'low_sample_size':
      case 'low_support_ratio':
      case 'pending_promotion':
      case 'overridden_noisy':
        return 'bg-amber-100 text-amber-800';
      default:
        return 'bg-gray-100 text-gray-700';
    }
  };

  const scopeOptionsForType = (scopeType: RmmTelemetryBaselineScopeType): ScopeOption[] => {
    if (!catalog) return [];
    if (scopeType === 'organization') {
      return [{ id: catalog.organization.id, name: catalog.organization.name }];
    }
    if (scopeType === 'customer') {
      return catalog.customers.map((customer) => ({
        id: customer.id,
        name: customer.name,
        subtitle: `${customer.deviceCount} devices`
      }));
    }
    if (scopeType === 'site') {
      return catalog.sites.map((site) => ({
        id: site.id,
        name: site.name,
        subtitle: `${site.customerName} • ${site.deviceCount} devices`
      }));
    }
    return catalog.devices.map((device) => ({
      id: device.agentId,
      name: device.hostname || device.agentId,
      subtitle: [device.customerName, device.siteName].filter(Boolean).join(' • ') || null
    }));
  };

  $: availableScopeOptions = scopeOptionsForType(selectedScopeType);
  $: {
    if (availableScopeOptions.length === 0) {
      selectedScopeId = '';
    } else if (!availableScopeOptions.some((option) => option.id === selectedScopeId)) {
      selectedScopeId = availableScopeOptions[0].id;
    }
  }
  $: visibleBaselines = baselines.filter((baseline) => {
    if (onlyOverridden && !baseline.overrideMatched) return false;
    if (onlyIgnored && baseline.overrideStabilityClass !== 'ignored') return false;
    return true;
  });
  $: visibleDriftItems = driftItems.filter((item) => {
    if (onlyOverridden && !item.overrideMatched) return false;
    if (onlyIgnored && item.overrideStabilityClass !== 'ignored') return false;
    return true;
  });
  $: trustSummary = {
    overridden: baselines.filter((baseline) => baseline.overrideMatched).length,
    ignored: baselines.filter((baseline) => baseline.overrideStabilityClass === 'ignored').length,
    suppressed: baselines.filter((baseline) => !baseline.baselineEligible).length
  };
  $: selectedBaseline = baselines.find((baseline) => baseline.factKey === selectedFactKey) ?? null;

  const loadCatalog = async () => {
    try {
      catalogLoading = true;
      catalogError = null;
      catalog = await rmmApi.getTelemetryBaselineScopes(500);
    } catch (err) {
      catalogError = err instanceof Error ? err.message : 'Failed to load baseline scopes';
      catalog = null;
    } finally {
      catalogLoading = false;
    }
  };

  const loadBaselines = async () => {
    if (!selectedScopeId.trim()) {
      baselines = [];
      summary = null;
      return;
    }

    selectedFactKey = '';
    selectedBaseline = null;
    driftItems = [];
    driftError = null;

    try {
      baselinesLoading = true;
      baselinesError = null;
      const response = await rmmApi.getTelemetryScopedBaselines(selectedScopeType, selectedScopeId, {
        factKey: factKeyFilter.trim() || undefined,
        onlyUnstable,
        limit: 1000
      });
      baselines = response.items;
    } catch (err) {
      baselinesError = err instanceof Error ? err.message : 'Failed to load baselines';
      baselines = [];
    } finally {
      baselinesLoading = false;
    }
  };

  const loadSummary = async () => {
    if (!selectedScopeId.trim()) {
      summary = null;
      return;
    }
    try {
      summaryLoading = true;
      summaryError = null;
      const response = await rmmApi.getTelemetryScopedBaselineSummary(selectedScopeType, selectedScopeId);
      summary = response.summary;
    } catch (err) {
      summaryError = err instanceof Error ? err.message : 'Failed to load summary';
      summary = null;
    } finally {
      summaryLoading = false;
    }
  };

  const loadDrift = async (factKey: string) => {
    if (selectedScopeType === 'device' || !selectedScopeId.trim()) return;
    selectedFactKey = factKey;
    try {
      driftLoading = true;
      driftError = null;
      const response = await rmmApi.getTelemetryScopedBaselineDrift(selectedScopeType, selectedScopeId, {
        factKey,
        limit: 300
      });
      driftItems = response.items;
    } catch (err) {
      driftError = err instanceof Error ? err.message : 'Failed to load drift';
      driftItems = [];
    } finally {
      driftLoading = false;
    }
  };

  const refreshScopeData = async () => {
    await Promise.all([loadBaselines(), loadSummary()]);
  };

  const changeScopeType = (scopeType: RmmTelemetryBaselineScopeType) => {
    selectedScopeType = scopeType;
    selectedFactKey = '';
    selectedBaseline = null;
    driftItems = [];
  };

  const refreshAll = async () => {
    await loadCatalog();
    await refreshScopeData();
  };

  const selectBaseline = async (baseline: RmmTelemetryScopedBaseline) => {
    selectedFactKey = baseline.factKey;
    if (selectedScopeType !== 'device') {
      await loadDrift(baseline.factKey);
    }
  };

  // --- Intents ---
  let activeTab: 'baselines' | 'routing' | 'intents' | 'overrides' = 'baselines';

  let intentsLoading = false;
  let intentsError: string | null = null;
  let intents: RmmTelemetryIntent[] = [];

  let showIntentForm = false;
  let intentFormMode: 'create' | 'edit' = 'create';
  let editingIntentId: string | null = null;
  let intentForm = resetIntentForm();

  function resetIntentForm() {
    return {
      name: '',
      description: '',
      type: 'hardcoded' as string,
      triggerDomain: '',
      triggerKey: '',
      allowListText: '',
      stepsText: '',
      aiPrompt: '',
      requiresApproval: true,
      maxRetries: 1,
      timeoutSeconds: 300,
      enabled: true,
      userPrompt: ''
    };
  }

  const loadIntents = async () => {
    try {
      intentsLoading = true;
      intentsError = null;
      const response = await rmmApi.getIntents();
      intents = response.items;
    } catch (err) {
      intentsError = err instanceof Error ? err.message : 'Failed to load intents';
      intents = [];
    } finally {
      intentsLoading = false;
    }
  };

  const openCreateIntent = () => {
    intentForm = resetIntentForm();
    intentFormMode = 'create';
    editingIntentId = null;
    showIntentForm = true;
  };

  const openCreateIntentFromBaseline = (factKey: string) => {
    intentForm = resetIntentForm();
    intentFormMode = 'create';
    editingIntentId = null;
    intentForm.triggerDomain = 'baseline';
    intentForm.triggerKey = factKey;
    intentForm.name = `Remediate ${factKey}`;
    intentForm.userPrompt = `Create a remediation intent for when the baseline fact "${factKey}" changes unexpectedly. The intent should diagnose the issue and attempt to restore the baseline value.`;
    showIntentForm = true;
    activeTab = 'intents';
  };

  const openEditIntent = (intent: RmmTelemetryIntent) => {
    intentFormMode = 'edit';
    editingIntentId = intent.id;
    intentForm = {
      name: intent.name,
      description: intent.description ?? '',
      type: intent.type,
      triggerDomain: intent.triggerDomain ?? '',
      triggerKey: intent.triggerKey ?? '',
      allowListText: intent.allowList ? intent.allowList.join('\n') : '',
      stepsText: intent.steps ? JSON.stringify(intent.steps, null, 2) : '',
      aiPrompt: intent.aiPrompt ?? '',
      requiresApproval: intent.requiresApproval,
      maxRetries: intent.maxRetries,
      timeoutSeconds: intent.timeoutSeconds,
      enabled: intent.enabled,
      userPrompt: ''
    };
    showIntentForm = true;
  };

  let intentSaving = false;
  let intentSaveError: string | null = null;

  const saveIntent = async () => {
    try {
      intentSaving = true;
      intentSaveError = null;

      const allowList = intentForm.allowListText.trim()
        ? intentForm.allowListText.split('\n').map(s => s.trim()).filter(Boolean)
        : undefined;

      let steps: Array<{ command: string; description?: string; timeout_seconds?: number }> | undefined;
      if (intentForm.stepsText.trim()) {
        try {
          steps = JSON.parse(intentForm.stepsText);
        } catch {
          intentSaveError = 'Steps must be valid JSON array';
          return;
        }
      }

      const payload = {
        name: intentForm.name,
        description: intentForm.description || undefined,
        type: intentForm.type,
        triggerDomain: intentForm.triggerDomain || undefined,
        triggerKey: intentForm.triggerKey || undefined,
        allowList,
        steps,
        aiPrompt: intentForm.aiPrompt || undefined,
        requiresApproval: intentForm.requiresApproval,
        maxRetries: intentForm.maxRetries,
        timeoutSeconds: intentForm.timeoutSeconds,
        enabled: intentForm.enabled
      };

      if (intentFormMode === 'edit' && editingIntentId) {
        await rmmApi.updateIntent(editingIntentId, payload);
      } else {
        await rmmApi.createIntent(payload);
      }
      showIntentForm = false;
      await loadIntents();
    } catch (err) {
      intentSaveError = err instanceof Error ? err.message : 'Failed to save intent';
    } finally {
      intentSaving = false;
    }
  };

  const deleteIntent = async (id: string) => {
    try {
      await rmmApi.deleteIntent(id);
      await loadIntents();
    } catch (err) {
      intentsError = err instanceof Error ? err.message : 'Failed to delete intent';
    }
  };

  const toggleIntentEnabled = async (intent: RmmTelemetryIntent) => {
    try {
      await rmmApi.updateIntent(intent.id, { enabled: !intent.enabled });
      await loadIntents();
    } catch (err) {
      intentsError = err instanceof Error ? err.message : 'Failed to toggle intent';
    }
  };

  // --- Routing Rules ---
  type RoutingFormMode = 'create' | 'edit';

  const routingOperatorOptions: Array<{ value: RmmTelemetryRoutingMatchOperator; label: string }> = [
    { value: 'equals', label: 'Equals' },
    { value: 'not_equals', label: 'Not equals' },
    { value: 'contains', label: 'Contains' },
    { value: 'not_contains', label: 'Not contains' },
    { value: 'starts_with', label: 'Starts with' },
    { value: 'ends_with', label: 'Ends with' },
    { value: 'exists', label: 'Exists' }
  ];

  const routingDomainOptions: Array<{ value: 'baseline' | 'scope_drift' | 'event'; label: string }> = [
    { value: 'baseline', label: 'Baseline shift' },
    { value: 'scope_drift', label: 'Scoped drift' },
    { value: 'event', label: 'Event' }
  ];

  const routingActionOptions: Array<{ value: RmmTelemetryRoutingAction; label: string }> = [
    { value: 'ignore', label: 'Ignore' },
    { value: 'ticket', label: 'Raise ticket' },
    { value: 'recommend', label: 'Recommend intent' },
    { value: 'auto_remediate', label: 'Queue remediation' },
    { value: 'llm_router', label: 'LLM router handoff' }
  ];

  let routingRulesLoading = false;
  let routingRulesError: string | null = null;
  let routingRules: RmmTelemetryRoutingRule[] = [];
  let showRoutingForm = false;
  let routingFormMode: RoutingFormMode = 'create';
  let editingRoutingRuleId: string | null = null;
  let routingForm = resetRoutingForm();
  let routingSaving = false;
  let routingSaveError: string | null = null;
  let selectedRoutingRuleId: string | null = null;
  let routingDecisionAgentId = '';
  let routingDecisionsLoading = false;
  let routingDecisionsError: string | null = null;
  let routingDecisions: RmmTelemetryDecision[] = [];
  let routingTestForm = resetRoutingTestForm();
  let routingTestLoading = false;
  let routingTestError: string | null = null;
  let routingTestResult: RmmTelemetryRoutingRuleTestResponse | null = null;

  function resetRoutingForm() {
    return {
      customerId: '',
      siteId: '',
      agentId: '',
      triggerDomain: 'baseline' as 'baseline' | 'scope_drift' | 'event',
      triggerKey: '',
      matchOperator: 'equals' as RmmTelemetryRoutingMatchOperator,
      matchValue: '',
      previousMatchOperator: '',
      previousMatchValue: '',
      minSupportRatio: '',
      minConfidenceScore: '',
      scopeTypeFilter: '',
      action: 'ticket' as RmmTelemetryRoutingAction,
      intentId: '',
      cooldownSeconds: 3600,
      enabled: false,
      priority: 100
    };
  }

  function resetRoutingTestForm() {
    return {
      domain: 'baseline' as 'baseline' | 'scope_drift' | 'event',
      triggerKey: '',
      currentValueText: '',
      previousValueText: '',
      supportRatio: '',
      confidenceScore: '',
      scopeType: '',
      customerId: '',
      siteId: '',
      agentId: ''
    };
  }

  const routingBlockedReasonLabel = (value: string): string => value.replaceAll('_', ' ');

  const decisionExecutionClass = (value: string): string => {
    switch (value) {
      case 'completed':
        return 'bg-green-100 text-green-800';
      case 'failed':
        return 'bg-rose-100 text-rose-800';
      case 'skipped':
        return 'bg-amber-100 text-amber-800';
      default:
        return 'bg-gray-100 text-gray-700';
    }
  };

  const scopeLabelForIds = (customerId?: string | null, siteId?: string | null, agentId?: string | null): string => {
    if (agentId && catalog) {
      const device = catalog.devices.find((item) => item.agentId === agentId);
      return device?.hostname || agentId;
    }
    if (siteId && catalog) {
      const site = catalog.sites.find((item) => item.id === siteId);
      return site ? `Site: ${site.name}` : `Site: ${siteId}`;
    }
    if (customerId && catalog) {
      const customer = catalog.customers.find((item) => item.id === customerId);
      return customer ? `Customer: ${customer.name}` : `Customer: ${customerId}`;
    }
    return 'Organization';
  };

  const parseValueText = (value: string): unknown => {
    const trimmed = value.trim();
    if (!trimmed) return null;
    try {
      return JSON.parse(trimmed);
    } catch {
      return trimmed;
    }
  };

  const loadRoutingRules = async () => {
    try {
      routingRulesLoading = true;
      routingRulesError = null;
      const response = await rmmApi.getRoutingRules();
      routingRules = response.items;
    } catch (err) {
      routingRulesError = err instanceof Error ? err.message : 'Failed to load routing rules';
      routingRules = [];
    } finally {
      routingRulesLoading = false;
    }
  };

  const loadRoutingDecisions = async (agentId: string, matchedRuleId: string | null) => {
    if (!agentId || !matchedRuleId) {
      routingDecisions = [];
      routingDecisionAgentId = '';
      routingDecisionsError = null;
      return;
    }

    try {
      routingDecisionsLoading = true;
      routingDecisionsError = null;
      routingDecisionAgentId = agentId;
      const response = await rmmApi.getTelemetryDecisions(agentId, 25, matchedRuleId);
      routingDecisions = response.items;
    } catch (err) {
      routingDecisionsError = err instanceof Error ? err.message : 'Failed to load rule decisions';
      routingDecisions = [];
    } finally {
      routingDecisionsLoading = false;
    }
  };

  const buildRoutingRulePayload = () => {
    const minSupportRatio = routingForm.minSupportRatio.trim() ? Number(routingForm.minSupportRatio) : null;
    const minConfidenceScore = routingForm.minConfidenceScore.trim() ? Number(routingForm.minConfidenceScore) : null;
    if ((minSupportRatio !== null && Number.isNaN(minSupportRatio)) || (minConfidenceScore !== null && Number.isNaN(minConfidenceScore))) {
      throw new Error('Support ratio and confidence score must be valid numbers');
    }

    return {
      customerId: routingForm.customerId || null,
      siteId: routingForm.siteId || null,
      agentId: routingForm.agentId || null,
      triggerDomain: routingForm.triggerDomain,
      triggerKey: routingForm.triggerKey.trim(),
      matchOperator: routingForm.matchOperator,
      matchValue: routingForm.matchValue.trim() || null,
      previousMatchOperator: routingForm.previousMatchOperator || null,
      previousMatchValue: routingForm.previousMatchValue.trim() || null,
      minSupportRatio,
      minConfidenceScore,
      scopeTypeFilter: routingForm.scopeTypeFilter || null,
      action: routingForm.action,
      intentId: routingForm.intentId || null,
      cooldownSeconds: Number(routingForm.cooldownSeconds) || 0,
      enabled: routingForm.enabled,
      priority: Number(routingForm.priority) || 100
    };
  };

  const buildRoutingTestCandidate = (): RmmTelemetryRoutingTestCandidate => ({
    domain: routingTestForm.domain,
    triggerKey: routingTestForm.triggerKey.trim(),
    currentValue: parseValueText(routingTestForm.currentValueText),
    currentValueText: routingTestForm.currentValueText.trim(),
    previousValue: routingTestForm.previousValueText.trim() ? parseValueText(routingTestForm.previousValueText) : null,
    previousValueText: routingTestForm.previousValueText.trim() || null,
    supportRatio: routingTestForm.supportRatio.trim() ? Number(routingTestForm.supportRatio) : null,
    confidenceScore: routingTestForm.confidenceScore.trim() ? Number(routingTestForm.confidenceScore) : null,
    scopeType: (routingTestForm.scopeType || null) as RmmTelemetryBaselineScopeType | null,
    customerId: routingTestForm.customerId || null,
    siteId: routingTestForm.siteId || null,
    agentId: routingTestForm.agentId || null
  });

  const syncRoutingTestFromForm = () => {
    routingTestForm = {
      ...routingTestForm,
      domain: routingForm.triggerDomain,
      triggerKey: routingForm.triggerKey,
      scopeType: routingForm.scopeTypeFilter || routingTestForm.scopeType,
      customerId: routingForm.customerId || routingTestForm.customerId,
      siteId: routingForm.siteId || routingTestForm.siteId,
      agentId: routingForm.agentId || routingTestForm.agentId
    };
  };

  const closeRoutingForm = () => {
    showRoutingForm = false;
    routingFormMode = 'create';
    editingRoutingRuleId = null;
    selectedRoutingRuleId = null;
    routingForm = resetRoutingForm();
    routingTestForm = resetRoutingTestForm();
    routingTestResult = null;
    routingTestError = null;
    routingSaveError = null;
    routingDecisions = [];
    routingDecisionAgentId = '';
  };

  const openCreateRoutingRule = () => {
    routingFormMode = 'create';
    editingRoutingRuleId = null;
    selectedRoutingRuleId = null;
    routingForm = resetRoutingForm();
    routingTestForm = resetRoutingTestForm();
    routingTestResult = null;
    routingTestError = null;
    showRoutingForm = true;
    activeTab = 'routing';
  };

  const openEditRoutingRule = async (rule: RmmTelemetryRoutingRule) => {
    routingFormMode = 'edit';
    editingRoutingRuleId = rule.id;
    selectedRoutingRuleId = rule.id;
    routingForm = {
      customerId: rule.customerId ?? '',
      siteId: rule.siteId ?? '',
      agentId: rule.agentId ?? '',
      triggerDomain: (rule.triggerDomain as 'baseline' | 'scope_drift' | 'event') ?? 'baseline',
      triggerKey: rule.triggerKey,
      matchOperator: (rule.matchOperator as RmmTelemetryRoutingMatchOperator) ?? 'equals',
      matchValue: rule.matchValue ?? '',
      previousMatchOperator: rule.previousMatchOperator ?? '',
      previousMatchValue: rule.previousMatchValue ?? '',
      minSupportRatio: rule.minSupportRatio?.toString() ?? '',
      minConfidenceScore: rule.minConfidenceScore?.toString() ?? '',
      scopeTypeFilter: rule.scopeTypeFilter ?? '',
      action: (rule.action as RmmTelemetryRoutingAction) ?? 'ignore',
      intentId: rule.intentId ?? '',
      cooldownSeconds: rule.cooldownSeconds,
      enabled: rule.enabled,
      priority: rule.priority
    };
    routingTestForm = {
      ...resetRoutingTestForm(),
      domain: (rule.triggerDomain as 'baseline' | 'scope_drift' | 'event') ?? 'baseline',
      triggerKey: rule.triggerKey,
      scopeType: rule.scopeTypeFilter ?? '',
      customerId: rule.customerId ?? '',
      siteId: rule.siteId ?? '',
      agentId: rule.agentId ?? ''
    };
    routingTestResult = null;
    routingTestError = null;
    showRoutingForm = true;
    activeTab = 'routing';
    await loadRoutingDecisions(rule.agentId || routingTestForm.agentId || '', rule.id);
  };

  const openCreateRoutingRuleFromBaseline = (baseline: RmmTelemetryScopedBaseline) => {
    openCreateRoutingRule();
    routingForm = {
      ...routingForm,
      customerId: selectedScopeType === 'customer' ? selectedScopeId : '',
      siteId: selectedScopeType === 'site' ? selectedScopeId : '',
      agentId: selectedScopeType === 'device' ? selectedScopeId : '',
      triggerDomain: 'baseline',
      triggerKey: baseline.factKey,
      matchOperator: 'equals',
      matchValue: JSON.stringify(baseline.promotedValue ?? null),
      minSupportRatio: baseline.supportRatio?.toString() ?? '',
      minConfidenceScore: baseline.confidenceScore?.toString() ?? '',
      scopeTypeFilter: 'device',
      action: 'ticket',
      cooldownSeconds: 3600
    };
    routingTestForm = {
      ...routingTestForm,
      domain: 'baseline',
      triggerKey: baseline.factKey,
      currentValueText: JSON.stringify(baseline.promotedValue ?? null),
      supportRatio: baseline.supportRatio?.toString() ?? '',
      confidenceScore: baseline.confidenceScore?.toString() ?? '',
      scopeType: 'device',
      customerId: selectedScopeType === 'customer' ? selectedScopeId : '',
      siteId: selectedScopeType === 'site' ? selectedScopeId : '',
      agentId: selectedScopeType === 'device' ? selectedScopeId : ''
    };
  };

  const openCreateRoutingRuleFromDrift = (drift: RmmTelemetryScopedBaselineDriftListResponse['items'][number]) => {
    openCreateRoutingRule();
    routingForm = {
      ...routingForm,
      customerId: selectedScopeType === 'customer' ? selectedScopeId : '',
      siteId: selectedScopeType === 'site' ? selectedScopeId : '',
      agentId: '',
      triggerDomain: 'scope_drift',
      triggerKey: drift.factKey,
      matchOperator: 'equals',
      matchValue: JSON.stringify(drift.deviceValue ?? null),
      previousMatchOperator: 'equals',
      previousMatchValue: JSON.stringify(drift.scopeValue ?? null),
      minSupportRatio: drift.scopeSupportRatio?.toString() ?? '',
      minConfidenceScore: drift.scopeConfidenceScore?.toString() ?? '',
      scopeTypeFilter: selectedScopeType,
      action: 'recommend',
      cooldownSeconds: 3600
    };
    routingTestForm = {
      ...routingTestForm,
      domain: 'scope_drift',
      triggerKey: drift.factKey,
      currentValueText: JSON.stringify(drift.deviceValue ?? null),
      previousValueText: JSON.stringify(drift.scopeValue ?? null),
      supportRatio: drift.scopeSupportRatio?.toString() ?? '',
      confidenceScore: drift.scopeConfidenceScore?.toString() ?? '',
      scopeType: selectedScopeType,
      customerId: drift.customerId ?? '',
      siteId: drift.siteId ?? '',
      agentId: drift.agentId
    };
  };

  const saveRoutingRule = async () => {
    try {
      routingSaving = true;
      routingSaveError = null;
      const payload = buildRoutingRulePayload();
      if (!payload.triggerKey) {
        routingSaveError = 'Trigger key is required';
        return;
      }

      if (routingFormMode === 'edit' && editingRoutingRuleId) {
        const updated = await rmmApi.updateRoutingRule(editingRoutingRuleId, payload);
        selectedRoutingRuleId = updated.id;
        await loadRoutingDecisions(updated.agentId || routingTestForm.agentId || '', updated.id);
      } else {
        const created = await rmmApi.createRoutingRule(payload);
        selectedRoutingRuleId = created.id;
        editingRoutingRuleId = created.id;
        routingFormMode = 'edit';
        await loadRoutingDecisions(created.agentId || routingTestForm.agentId || '', created.id);
      }
      await loadRoutingRules();
    } catch (err) {
      routingSaveError = err instanceof Error ? err.message : 'Failed to save routing rule';
    } finally {
      routingSaving = false;
    }
  };

  const deleteRoutingRule = async (id: string) => {
    try {
      await rmmApi.deleteRoutingRule(id);
      if (editingRoutingRuleId === id) {
        closeRoutingForm();
      }
      await loadRoutingRules();
    } catch (err) {
      routingRulesError = err instanceof Error ? err.message : 'Failed to delete routing rule';
    }
  };

  const toggleRoutingRuleEnabled = async (rule: RmmTelemetryRoutingRule) => {
    try {
      const updated = rule.enabled
        ? await rmmApi.disableRoutingRule(rule.id)
        : await rmmApi.enableRoutingRule(rule.id);
      if (editingRoutingRuleId === updated.id) {
        routingForm.enabled = updated.enabled;
      }
      await loadRoutingRules();
    } catch (err) {
      routingRulesError = err instanceof Error ? err.message : 'Failed to toggle routing rule';
    }
  };

  const runRoutingRuleTest = async () => {
    try {
      routingTestLoading = true;
      routingTestError = null;
      const candidate = buildRoutingTestCandidate();
      if (!candidate.triggerKey.trim() || !candidate.currentValueText?.trim()) {
        routingTestError = 'Trigger key and current value are required to dry-run a rule';
        return;
      }
      routingTestResult = await rmmApi.testRoutingRule({
        ruleId: editingRoutingRuleId || undefined,
        rule: editingRoutingRuleId ? undefined : buildRoutingRulePayload(),
        candidate
      });
      await loadRoutingDecisions(candidate.agentId || routingDecisionAgentId, editingRoutingRuleId);
    } catch (err) {
      routingTestError = err instanceof Error ? err.message : 'Failed to dry-run routing rule';
      routingTestResult = null;
    } finally {
      routingTestLoading = false;
    }
  };

  // --- Stability Overrides ---
  let overridesLoading = false;
  let overridesError: string | null = null;
  let overrides: RmmTelemetryStabilityOverride[] = [];

  type OverrideStabilityClass = 'stable' | 'noisy' | 'ignored';
  type OverrideFormMode = 'create' | 'edit';

  let showOverrideForm = false;
  let overrideFormMode: OverrideFormMode = 'create';
  let editingOverrideId: string | null = null;
  let overrideForm = resetOverrideForm();
  let overrideSaving = false;
  let overridePreviewLoading = false;
  let overridePreviewError: string | null = null;
  let overridePreview: RmmTelemetryStabilityOverridePreviewResponse | null = null;
  let overridePreviewTimer: ReturnType<typeof setTimeout> | null = null;

  function resetOverrideForm() {
    return {
      factKeyPattern: '',
      stabilityClass: 'ignored' as OverrideStabilityClass,
      reason: ''
    };
  }

  const resetOverridePreview = () => {
    overridePreview = null;
    overridePreviewError = null;
    overridePreviewLoading = false;
  };

  const closeOverrideForm = () => {
    showOverrideForm = false;
    overrideFormMode = 'create';
    editingOverrideId = null;
    overrideForm = resetOverrideForm();
    if (overridePreviewTimer) {
      clearTimeout(overridePreviewTimer);
      overridePreviewTimer = null;
    }
    resetOverridePreview();
  };

  const loadOverrides = async () => {
    try {
      overridesLoading = true;
      overridesError = null;
      const response = await rmmApi.getStabilityOverrides();
      overrides = response.items;
    } catch (err) {
      overridesError = err instanceof Error ? err.message : 'Failed to load overrides';
      overrides = [];
    } finally {
      overridesLoading = false;
    }
  };

  const loadOverridePreview = async (factKeyPattern: string) => {
    if (!factKeyPattern.trim()) {
      resetOverridePreview();
      return;
    }
    try {
      overridePreviewLoading = true;
      overridePreviewError = null;
      overridePreview = await rmmApi.previewStabilityOverride(factKeyPattern.trim(), 8);
    } catch (err) {
      overridePreviewError = err instanceof Error ? err.message : 'Failed to preview override impact';
      overridePreview = null;
    } finally {
      overridePreviewLoading = false;
    }
  };

  const scheduleOverridePreview = () => {
    if (overridePreviewTimer) {
      clearTimeout(overridePreviewTimer);
      overridePreviewTimer = null;
    }
    const pattern = overrideForm.factKeyPattern.trim();
    if (!showOverrideForm || !pattern) {
      resetOverridePreview();
      return;
    }
    overridePreviewTimer = setTimeout(() => {
      void loadOverridePreview(pattern);
    }, 250);
  };

  const saveOverride = async () => {
    if (!overrideForm.factKeyPattern.trim()) return;
    try {
      overrideSaving = true;
      const payload = {
        factKeyPattern: overrideForm.factKeyPattern.trim(),
        stabilityClass: overrideForm.stabilityClass,
        reason: overrideForm.reason.trim() || undefined
      };
      if (overrideFormMode === 'edit' && editingOverrideId) {
        await rmmApi.updateStabilityOverride(editingOverrideId, {
          ...payload,
          reason: overrideForm.reason.trim() || null
        });
      } else {
        await rmmApi.createStabilityOverride(payload);
      }
      closeOverrideForm();
      await Promise.all([loadOverrides(), refreshScopeData()]);
      if (selectedFactKey && selectedScopeType !== 'device') {
        await loadDrift(selectedFactKey);
      }
    } catch (err) {
      overridesError = err instanceof Error ? err.message : 'Failed to save override';
    } finally {
      overrideSaving = false;
    }
  };

  const deleteOverride = async (id: string) => {
    try {
      await rmmApi.deleteStabilityOverride(id);
      await Promise.all([loadOverrides(), refreshScopeData()]);
      if (selectedFactKey && selectedScopeType !== 'device') {
        await loadDrift(selectedFactKey);
      }
    } catch (err) {
      overridesError = err instanceof Error ? err.message : 'Failed to delete override';
    }
  };

  const createOverrideFromFact = (factKey: string) => {
    overrideFormMode = 'create';
    editingOverrideId = null;
    overrideForm = { factKeyPattern: factKey, stabilityClass: 'ignored', reason: '' };
    showOverrideForm = true;
    activeTab = 'overrides';
    scheduleOverridePreview();
  };

  const openEditOverride = (override: RmmTelemetryStabilityOverride) => {
    overrideFormMode = 'edit';
    editingOverrideId = override.id;
    overrideForm = {
      factKeyPattern: override.factKeyPattern,
      stabilityClass: override.stabilityClass,
      reason: override.reason ?? ''
    };
    showOverrideForm = true;
    activeTab = 'overrides';
    scheduleOverridePreview();
  };

  const openOverrideFromBaseline = (baseline: RmmTelemetryScopedBaseline) => {
    if (baseline.overrideId) {
      const existing = overrides.find((override) => override.id === baseline.overrideId);
      if (existing) {
        openEditOverride(existing);
        return;
      }
    }
    createOverrideFromFact(baseline.factKey);
  };

  onDestroy(() => {
    if (overridePreviewTimer) {
      clearTimeout(overridePreviewTimer);
    }
  });

  onMount(async () => {
    await loadCatalog();
    await Promise.all([refreshScopeData(), loadIntents(), loadOverrides(), loadRoutingRules()]);
  });
</script>

<div class="space-y-6">
  <div class="flex items-start justify-between">
    <div>
      <h1 class="text-3xl font-bold aero-gradient-text">Baseline Observatory</h1>
      <p class="text-sm aero-muted mt-1">
        Observe promoted baselines, route them into actions, manage intents, and configure stability overrides.
      </p>
    </div>
  </div>

  <div class="flex gap-1 border-b">
    <button
      class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
      class:border-blue-600={activeTab === 'baselines'}
      class:text-blue-600={activeTab === 'baselines'}
      class:border-transparent={activeTab !== 'baselines'}
      class:text-gray-500={activeTab !== 'baselines'}
      on:click={() => activeTab = 'baselines'}
    >
      <ShieldCheck class="h-4 w-4 inline mr-1" />Baselines
    </button>
    <button
      class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
      class:border-blue-600={activeTab === 'routing'}
      class:text-blue-600={activeTab === 'routing'}
      class:border-transparent={activeTab !== 'routing'}
      class:text-gray-500={activeTab !== 'routing'}
      on:click={() => { activeTab = 'routing'; if (routingRules.length === 0 && !routingRulesLoading) loadRoutingRules(); }}
    >
      <GitBranch class="h-4 w-4 inline mr-1" />Routing ({routingRules.length})
    </button>
    <button
      class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
      class:border-blue-600={activeTab === 'intents'}
      class:text-blue-600={activeTab === 'intents'}
      class:border-transparent={activeTab !== 'intents'}
      class:text-gray-500={activeTab !== 'intents'}
      on:click={() => { activeTab = 'intents'; if (intents.length === 0 && !intentsLoading) loadIntents(); }}
    >
      <Zap class="h-4 w-4 inline mr-1" />Intents ({intents.length})
    </button>
    <button
      class="px-4 py-2 text-sm font-medium transition-colors border-b-2 -mb-px"
      class:border-blue-600={activeTab === 'overrides'}
      class:text-blue-600={activeTab === 'overrides'}
      class:border-transparent={activeTab !== 'overrides'}
      class:text-gray-500={activeTab !== 'overrides'}
      on:click={() => { activeTab = 'overrides'; if (overrides.length === 0 && !overridesLoading) loadOverrides(); }}
    >
      <Settings class="h-4 w-4 inline mr-1" />Stability Overrides ({overrides.length})
    </button>
  </div>

  {#if activeTab === 'baselines'}
  <Card>
    <CardHeader>
      <CardTitle>Scope Filters</CardTitle>
      <CardDescription>Choose a scope and load its current baseline facts.</CardDescription>
    </CardHeader>
    <CardContent>
      {#if catalogError}
        <div class="mb-4 aero-alert-error">
          <div class="flex items-center gap-2">
            <AlertCircle class="h-4 w-4" />
            <span>{catalogError}</span>
          </div>
        </div>
      {/if}

      <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-5 gap-3">
        <div>
          <Label for="baseline-scope-type" className="mb-1 block">Scope Type</Label>
          <select
            id="baseline-scope-type"
            class="glass-input h-10 w-full"
            bind:value={selectedScopeType}
            on:change={(event) => changeScopeType((event.currentTarget as HTMLSelectElement).value as RmmTelemetryBaselineScopeType)}
            disabled={catalogLoading}
          >
            {#each scopeTypeOptions as option}
              <option value={option.value}>{option.label}</option>
            {/each}
          </select>
        </div>

        <div class="md:col-span-2">
          <Label for="baseline-scope-id" className="mb-1 block">Scope</Label>
          <select
            id="baseline-scope-id"
            class="glass-input h-10 w-full"
            bind:value={selectedScopeId}
            disabled={catalogLoading || availableScopeOptions.length === 0}
          >
            {#each availableScopeOptions as option}
              <option value={option.id}>
                {option.name}{option.subtitle ? ` (${option.subtitle})` : ''}
              </option>
            {/each}
          </select>
        </div>

        <div>
          <Label for="baseline-fact-filter" className="mb-1 block">Fact Filter</Label>
          <Input id="baseline-fact-filter" type="text" bind:value={factKeyFilter} placeholder="service.sql..." />
        </div>

        <div class="flex items-end">
          <div class="flex flex-col gap-2 text-sm">
            <label class="aero-label flex items-center gap-2 cursor-pointer">
              <input type="checkbox" bind:checked={onlyUnstable} class="aero-checkbox" />
              Only unstable
            </label>
            <label class="aero-label flex items-center gap-2 cursor-pointer">
              <input type="checkbox" bind:checked={onlyOverridden} class="aero-checkbox" />
              Only overridden
            </label>
            <label class="aero-label flex items-center gap-2 cursor-pointer">
              <input type="checkbox" bind:checked={onlyIgnored} class="aero-checkbox" />
              Only ignored
            </label>
          </div>
        </div>
      </div>

      <div class="mt-4 flex flex-wrap gap-2">
        <Button on:click={refreshScopeData} disabled={catalogLoading || !selectedScopeId || baselinesLoading || summaryLoading}>
          {#if baselinesLoading || summaryLoading}
            <RefreshCw class="h-4 w-4 animate-spin" />
          {:else}
            <RefreshCw class="h-4 w-4" />
          {/if}
          Apply
        </Button>
        <Button variant="outline" on:click={refreshAll} disabled={catalogLoading}>
          <RefreshCw class={`h-4 w-4 ${catalogLoading ? 'animate-spin' : ''}`} />
          Reload Scopes
        </Button>
      </div>
    </CardContent>
  </Card>

  <div class="grid grid-cols-1 md:grid-cols-3 xl:grid-cols-6 gap-4">
    <Card>
      <CardHeader>
        <CardDescription>Total Facts</CardDescription>
        <CardTitle>{summary?.totalFacts ?? '—'}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Stable Facts</CardDescription>
        <CardTitle>{summary?.stableFacts ?? '—'}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Unstable Facts</CardDescription>
        <CardTitle>{summary?.unstableFacts ?? '—'}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Avg Confidence</CardDescription>
        <CardTitle>{summary ? round(summary.avgConfidenceScore) : '—'}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Overridden</CardDescription>
        <CardTitle>{trustSummary.overridden}</CardTitle>
      </CardHeader>
    </Card>
    <Card>
      <CardHeader>
        <CardDescription>Ignored</CardDescription>
        <CardTitle>{trustSummary.ignored}</CardTitle>
      </CardHeader>
    </Card>
  </div>

  {#if summaryError}
    <div class="aero-alert-error">{summaryError}</div>
  {/if}

  <Card>
    <CardHeader>
      <CardTitle>Scoped Baselines</CardTitle>
      <CardDescription>
        {selectedScopeType} scope • {visibleBaselines.length} loaded facts
        {#if summary?.latestUpdatedAt}
          • Updated {dateLabel(summary.latestUpdatedAt)}
        {/if}
      </CardDescription>
    </CardHeader>
    <CardContent>
      {#if baselinesError}
        <div class="aero-alert-error">{baselinesError}</div>
      {:else if baselinesLoading}
        <div class="flex h-24 items-center justify-center">
          <RefreshCw class="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      {:else if visibleBaselines.length === 0}
        <div class="text-sm aero-empty-state">No baseline records for this scope.</div>
      {:else}
        <div class="aero-table-wrap overflow-x-auto">
          <Table className="text-sm">
            <TableHeader>
              <TableRow>
                <TableHead className="w-[240px]">Fact</TableHead>
                <TableHead>Promoted Value</TableHead>
                <TableHead className="w-[110px]">Support</TableHead>
                <TableHead className="w-[120px]">Count</TableHead>
                <TableHead className="w-[110px]">Confidence</TableHead>
                <TableHead className="w-[90px]">Stable</TableHead>
                <TableHead className="w-[180px]">Updated</TableHead>
                <TableHead className="w-[180px]">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {#each visibleBaselines as baseline (baseline.factKey)}
                <TableRow
                  className={selectedBaseline?.factKey === baseline.factKey ? 'bg-blue-50' : ''}
                  on:click={() => void selectBaseline(baseline)}
                >
                  <TableCell className="align-top">
                    <div class="font-mono text-xs">{baseline.factKey}</div>
                    <div class="mt-1 flex flex-wrap gap-1">
                      <span class={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${promotionStateClass(baseline.promotionState)}`}>
                        {promotionStateLabel(baseline.promotionState)}
                      </span>
                      {#if baseline.overrideMatched}
                        <span class={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${stabilityClassClass(baseline.overrideStabilityClass)}`}>
                          Override: {baseline.overrideStabilityClass}
                        </span>
                      {/if}
                      {#if baseline.trustWarnings.length > 0}
                        {#each baseline.trustWarnings.slice(0, 2) as warning}
                          <span class={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${warningClass(warning)}`}>
                            {warningLabel(warning)}
                          </span>
                        {/each}
                      {/if}
                    </div>
                  </TableCell>
                  <TableCell className="font-mono text-xs text-muted-foreground align-top" title={JSON.stringify(baseline.promotedValue)}>
                    {toDisplayJson(baseline.promotedValue)}
                  </TableCell>
                  <TableCell className="align-top">{percent(baseline.supportRatio ?? 0)}</TableCell>
                  <TableCell className="align-top">{baseline.supportCount}/{baseline.totalCount}</TableCell>
                  <TableCell className="align-top">{round(baseline.confidenceScore ?? 0)}</TableCell>
                  <TableCell className="align-top">
                    <span class={baseline.isStable ? 'aero-badge-online' : 'aero-badge-amber'}>
                      {baseline.isStable ? 'Yes' : 'No'}
                    </span>
                  </TableCell>
                  <TableCell className="align-top text-xs text-muted-foreground">{dateLabel(baseline.updatedAt)}</TableCell>
                  <TableCell className="align-top">
                    <div class="flex gap-1">
                      <button
                        class="text-xs text-blue-600 hover:text-blue-800 hover:underline"
                        on:click|stopPropagation={() => openCreateIntentFromBaseline(baseline.factKey)}
                        title="Create intent for this fact"
                      >
                        <Zap class="h-3 w-3 inline" /> Intent
                      </button>
                      <button
                        class="text-xs text-emerald-600 hover:text-emerald-800 hover:underline"
                        on:click|stopPropagation={() => openCreateRoutingRuleFromBaseline(baseline)}
                        title="Create routing rule from this fact"
                      >
                        <GitBranch class="h-3 w-3 inline" /> Route
                      </button>
                      <button
                        class="text-xs text-gray-500 hover:text-gray-700 hover:underline"
                        on:click|stopPropagation={() => openOverrideFromBaseline(baseline)}
                        title={baseline.overrideMatched ? 'Edit matched override' : 'Override stability class'}
                      >
                        <Settings class="h-3 w-3 inline" /> {baseline.overrideMatched ? 'Edit Override' : 'Override'}
                      </button>
                    </div>
                  </TableCell>
                </TableRow>
              {/each}
            </TableBody>
          </Table>
        </div>
      {/if}
    </CardContent>
  </Card>

  {#if selectedBaseline}
    <Card>
      <CardHeader>
        <CardTitle>Fact Diagnostics</CardTitle>
        <CardDescription>
          Explainability for <span class="font-mono">{selectedBaseline.factKey}</span>
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div class="grid grid-cols-1 md:grid-cols-2 xl:grid-cols-4 gap-4 text-sm">
          <div class="baseline-panel p-4">
            <div class="text-xs uppercase tracking-wide text-muted-foreground">Promotion State</div>
            <div class="mt-2">
              <span class={`inline-flex rounded-full px-2 py-1 text-xs font-medium ${promotionStateClass(selectedBaseline.promotionState)}`}>
                {promotionStateLabel(selectedBaseline.promotionState)}
              </span>
            </div>
            <div class="mt-3 text-xs text-muted-foreground">
              Baseline eligible: {selectedBaseline.baselineEligible ? 'Yes' : 'No'}
            </div>
            <div class="mt-1 text-xs text-muted-foreground">
              Effective stability: {selectedBaseline.effectiveStabilityClass ?? 'Not available'}
            </div>
          </div>
          <div class="baseline-panel p-4">
            <div class="text-xs uppercase tracking-wide text-muted-foreground">Override</div>
            {#if selectedBaseline.overrideMatched}
              <div class="mt-2 font-mono text-xs">{selectedBaseline.overridePattern}</div>
              <div class="mt-2">
                <span class={`inline-flex rounded-full px-2 py-1 text-xs font-medium ${stabilityClassClass(selectedBaseline.overrideStabilityClass)}`}>
                  {selectedBaseline.overrideStabilityClass}
                </span>
              </div>
              <div class="mt-3 text-xs text-muted-foreground">{selectedBaseline.overrideReason ?? 'No reason recorded.'}</div>
            {:else}
              <div class="mt-2 text-sm text-muted-foreground">No override matched this fact.</div>
            {/if}
          </div>
          <div class="baseline-panel p-4">
            <div class="text-xs uppercase tracking-wide text-muted-foreground">Confidence</div>
            <div class="mt-2 text-lg font-semibold">{round(selectedBaseline.confidenceScore ?? 0)}</div>
            <div class="mt-3 text-xs text-muted-foreground">Support ratio: {percent(selectedBaseline.supportRatio ?? 0)}</div>
            <div class="mt-1 text-xs text-muted-foreground">Support count: {selectedBaseline.supportCount}/{selectedBaseline.totalCount}</div>
            <div class="mt-1 text-xs text-muted-foreground">Sample size: {selectedBaseline.sampleSize}</div>
          </div>
          <div class="baseline-panel p-4">
            <div class="text-xs uppercase tracking-wide text-muted-foreground">Trust Warnings</div>
            {#if selectedBaseline.trustWarnings.length === 0}
              <div class="mt-2 text-sm text-muted-foreground">No trust warnings for this fact.</div>
            {:else}
              <div class="mt-2 flex flex-wrap gap-2">
                {#each selectedBaseline.trustWarnings as warning}
                  <span class={`inline-flex rounded-full px-2 py-1 text-xs font-medium ${warningClass(warning)}`}>
                    {warningLabel(warning)}
                  </span>
                {/each}
              </div>
            {/if}
            <div class="mt-3 text-xs text-muted-foreground">Last changed: {dateLabel(selectedBaseline.lastChangedAt)}</div>
            <div class="mt-1 text-xs text-muted-foreground">Last updated: {dateLabel(selectedBaseline.updatedAt)}</div>
          </div>
        </div>
      </CardContent>
    </Card>
  {/if}

  {#if selectedScopeType !== 'device'}
    <Card>
      <CardHeader>
        <CardTitle>Drift View</CardTitle>
        <CardDescription>
          {#if selectedFactKey}
            Device drift for <span class="font-mono">{selectedFactKey}</span>
            {#if selectedBaseline?.overrideMatched}
              • overridden by <span class="font-mono">{selectedBaseline.overridePattern}</span>
            {/if}
          {:else}
            Select a fact row above to inspect drifting devices.
          {/if}
        </CardDescription>
      </CardHeader>
      <CardContent>
        {#if driftError}
          <div class="aero-alert-error">{driftError}</div>
        {:else if driftLoading}
          <div class="flex h-20 items-center justify-center">
            <RefreshCw class="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        {:else if selectedFactKey && visibleDriftItems.length === 0}
          <div class="text-sm aero-empty-state">No drift detected for this fact.</div>
        {:else if selectedFactKey && visibleDriftItems.length > 0}
          <div class="aero-table-wrap overflow-x-auto">
            <Table className="text-sm">
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[210px]">Device</TableHead>
                  <TableHead className="w-[180px]">Customer/Site</TableHead>
                  <TableHead>Scope Value</TableHead>
                  <TableHead>Device Value</TableHead>
                  <TableHead className="w-[180px]">Device Updated</TableHead>
                  <TableHead className="w-[110px]">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {#each visibleDriftItems as drift (drift.agentId + drift.factKey)}
                  <TableRow>
                    <TableCell className="align-top">
                      <div class="font-medium">{drift.hostname}</div>
                      <div class="font-mono text-xs text-muted-foreground">{drift.agentId}</div>
                      <div class="mt-1 flex flex-wrap gap-1">
                        <span class={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${promotionStateClass(drift.promotionState)}`}>
                          {promotionStateLabel(drift.promotionState)}
                        </span>
                        {#if drift.overrideMatched}
                          <span class={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${stabilityClassClass(drift.overrideStabilityClass)}`}>
                            Override: {drift.overrideStabilityClass}
                          </span>
                        {/if}
                      </div>
                    </TableCell>
                    <TableCell className="align-top text-xs text-muted-foreground">
                      {drift.customerName ?? '—'}
                      {#if drift.siteName}
                        <br />{drift.siteName}
                      {/if}
                    </TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground align-top" title={JSON.stringify(drift.scopeValue)}>
                      {toDisplayJson(drift.scopeValue, 80)}
                    </TableCell>
                    <TableCell className="font-mono text-xs text-muted-foreground align-top" title={JSON.stringify(drift.deviceValue)}>
                      {toDisplayJson(drift.deviceValue, 80)}
                    </TableCell>
                    <TableCell className="align-top text-xs text-muted-foreground">
                      {dateLabel(drift.deviceUpdatedAt)}
                      <div class="mt-1">Scope sample: {drift.scopeSampleSize}</div>
                      <div>Scope confidence: {round(drift.scopeConfidenceScore ?? 0)}</div>
                    </TableCell>
                    <TableCell className="align-top">
                      <button
                        class="text-xs text-emerald-600 hover:text-emerald-800 hover:underline"
                        on:click={() => openCreateRoutingRuleFromDrift(drift)}
                        title="Create routing rule from this drift"
                      >
                        <GitBranch class="h-3 w-3 inline" /> Route
                      </button>
                    </TableCell>
                  </TableRow>
                {/each}
              </TableBody>
            </Table>
          </div>
        {/if}
      </CardContent>
    </Card>
  {/if}
  {/if}

  {#if activeTab === 'routing'}
  <div class="space-y-6">
    <Card>
      <CardHeader>
        <div class="flex items-center justify-between gap-4">
          <div>
            <CardTitle>Routing Rules</CardTitle>
            <CardDescription>
              Turn baseline shifts, scoped drift, and events into deterministic outcomes.
            </CardDescription>
          </div>
          <Button on:click={openCreateRoutingRule}>
            <Plus class="h-4 w-4 mr-1" /> New Rule
          </Button>
        </div>
      </CardHeader>
      <CardContent>
        {#if routingRulesError}
          <div class="aero-alert-error mb-4">{routingRulesError}</div>
        {/if}
        {#if routingRulesLoading}
          <div class="flex h-24 items-center justify-center">
            <RefreshCw class="h-6 w-6 animate-spin text-muted-foreground" />
          </div>
        {:else if routingRules.length === 0}
          <div class="text-sm aero-empty-state">
            No routing rules configured yet. Create one from a baseline fact, a drift row, or start from scratch.
          </div>
        {:else}
          <div class="aero-table-wrap overflow-x-auto">
            <Table className="text-sm">
              <TableHeader>
                <TableRow>
                  <TableHead className="w-[230px]">Trigger</TableHead>
                  <TableHead className="w-[170px]">Scope</TableHead>
                  <TableHead className="w-[160px]">Action</TableHead>
                  <TableHead className="w-[120px]">Cooldown</TableHead>
                  <TableHead className="w-[160px]">Readiness</TableHead>
                  <TableHead className="w-[170px]">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {#each routingRules as rule (rule.id)}
                  <TableRow>
                    <TableCell className="align-top">
                      <div class="font-medium">{rule.triggerDomain}</div>
                      <div class="mt-1 font-mono text-xs text-muted-foreground">{rule.triggerKey}</div>
                      <div class="mt-2 flex flex-wrap gap-1">
                        <span class={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${rule.enabled ? 'bg-green-100 text-green-800' : 'bg-gray-100 text-gray-700'}`}>
                          {rule.enabled ? 'Enabled' : 'Disabled'}
                        </span>
                        <span class="inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium bg-blue-100 text-blue-800">
                          {rule.matchOperator}{#if rule.matchValue}: {rule.matchValue}{/if}
                        </span>
                        {#if rule.scopeTypeFilter}
                          <span class="inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium bg-indigo-100 text-indigo-800">
                            scope: {rule.scopeTypeFilter}
                          </span>
                        {/if}
                      </div>
                    </TableCell>
                    <TableCell className="align-top text-xs text-muted-foreground">
                      <div>{scopeLabelForIds(rule.customerId, rule.siteId, rule.agentId)}</div>
                      <div class="mt-1">Specificity: {rule.specificity}</div>
                      <div class="mt-1">Priority: {rule.priority}</div>
                    </TableCell>
                    <TableCell className="align-top">
                      <div class="font-medium">{rule.action}</div>
                      {#if rule.intentId}
                        <div class="mt-1 font-mono text-xs text-muted-foreground">{rule.intentId}</div>
                      {/if}
                    </TableCell>
                    <TableCell className="align-top text-xs text-muted-foreground">
                      {rule.cooldownSeconds}s
                      {#if rule.minSupportRatio !== null}
                        <div class="mt-1">support ≥ {rule.minSupportRatio}</div>
                      {/if}
                      {#if rule.minConfidenceScore !== null}
                        <div>confidence ≥ {rule.minConfidenceScore}</div>
                      {/if}
                    </TableCell>
                    <TableCell className="align-top">
                      <div class="flex flex-wrap gap-1">
                        {#if rule.blockedReasons.length === 0}
                          <span class="inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium bg-green-100 text-green-800">
                            Ready
                          </span>
                        {:else}
                          {#each rule.blockedReasons as reason}
                            <span class="inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium bg-amber-100 text-amber-800">
                              {routingBlockedReasonLabel(reason)}
                            </span>
                          {/each}
                        {/if}
                      </div>
                    </TableCell>
                    <TableCell className="align-top">
                      <div class="flex flex-wrap gap-2">
                        <button class="text-xs text-blue-600 hover:text-blue-800 hover:underline" on:click={() => openEditRoutingRule(rule)}>
                          <Edit class="h-3 w-3 inline" /> Edit
                        </button>
                        <button class="text-xs text-emerald-600 hover:text-emerald-800 hover:underline" on:click={() => toggleRoutingRuleEnabled(rule)}>
                          {rule.enabled ? 'Disable' : 'Enable'}
                        </button>
                        <button class="text-xs text-rose-600 hover:text-rose-800 hover:underline" on:click={() => deleteRoutingRule(rule.id)}>
                          <Trash2 class="h-3 w-3 inline" /> Delete
                        </button>
                      </div>
                    </TableCell>
                  </TableRow>
                {/each}
              </TableBody>
            </Table>
          </div>
        {/if}
      </CardContent>
    </Card>

    {#if showRoutingForm}
      <Card>
        <CardHeader>
          <CardTitle>{routingFormMode === 'edit' ? 'Edit Routing Rule' : 'Create Routing Rule'}</CardTitle>
          <CardDescription>
            Author a deterministic rule, then dry-run it against a candidate before enabling it.
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-6">
          {#if routingSaveError}
            <div class="aero-alert-error">{routingSaveError}</div>
          {/if}

          <div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
            <div>
              <Label className="block mb-1">Trigger Domain</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.triggerDomain} on:change={syncRoutingTestFromForm}>
                {#each routingDomainOptions as option}
                  <option value={option.value}>{option.label}</option>
                {/each}
              </select>
            </div>
            <div class="xl:col-span-2">
              <Label className="block mb-1">Trigger Key Pattern</Label>
              <Input bind:value={routingForm.triggerKey} placeholder="app.cisco_anyconnect.installed or app.*" on:input={syncRoutingTestFromForm} />
            </div>
            <div>
              <Label className="block mb-1">Action</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.action}>
                {#each routingActionOptions as option}
                  <option value={option.value}>{option.label}</option>
                {/each}
              </select>
            </div>

            <div>
              <Label className="block mb-1">Current Match</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.matchOperator}>
                {#each routingOperatorOptions as option}
                  <option value={option.value}>{option.label}</option>
                {/each}
              </select>
            </div>
            <div class="xl:col-span-3">
              <Label className="block mb-1">Current Match Value</Label>
              <Input bind:value={routingForm.matchValue} placeholder='e.g. "false" or vpn' />
            </div>

            <div>
              <Label className="block mb-1">Previous Match</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.previousMatchOperator}>
                <option value="">None</option>
                {#each routingOperatorOptions as option}
                  <option value={option.value}>{option.label}</option>
                {/each}
              </select>
            </div>
            <div class="xl:col-span-3">
              <Label className="block mb-1">Previous Match Value</Label>
              <Input bind:value={routingForm.previousMatchValue} placeholder='e.g. "true"' disabled={!routingForm.previousMatchOperator} />
            </div>

            <div>
              <Label className="block mb-1">Min Support Ratio</Label>
              <Input bind:value={routingForm.minSupportRatio} placeholder="0.8" />
            </div>
            <div>
              <Label className="block mb-1">Min Confidence Score</Label>
              <Input bind:value={routingForm.minConfidenceScore} placeholder="0.8" />
            </div>
            <div>
              <Label className="block mb-1">Scope Type Filter</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.scopeTypeFilter} on:change={syncRoutingTestFromForm}>
                <option value="">Any</option>
                {#each scopeTypeOptions as option}
                  <option value={option.value}>{option.label}</option>
                {/each}
              </select>
            </div>
            <div>
              <Label className="block mb-1">Intent</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.intentId} disabled={!(routingForm.action === 'recommend' || routingForm.action === 'auto_remediate')}>
                <option value="">None</option>
                {#each intents as intent}
                  <option value={intent.id}>{intent.name}{intent.enabled ? '' : ' (disabled)'}</option>
                {/each}
              </select>
            </div>

            <div>
              <Label className="block mb-1">Customer Selector</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.customerId} on:change={syncRoutingTestFromForm}>
                <option value="">Organization-wide</option>
                {#each catalog?.customers ?? [] as customer}
                  <option value={customer.id}>{customer.name}</option>
                {/each}
              </select>
            </div>
            <div>
              <Label className="block mb-1">Site Selector</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.siteId} on:change={syncRoutingTestFromForm}>
                <option value="">Any site</option>
                {#each catalog?.sites ?? [] as site}
                  <option value={site.id}>{site.name}</option>
                {/each}
              </select>
            </div>
            <div>
              <Label className="block mb-1">Device Selector</Label>
              <select class="glass-input h-10 w-full" bind:value={routingForm.agentId} on:change={syncRoutingTestFromForm}>
                <option value="">Any device</option>
                {#each catalog?.devices ?? [] as device}
                  <option value={device.agentId}>{device.hostname || device.agentId}</option>
                {/each}
              </select>
            </div>
            <div>
              <Label className="block mb-1">Cooldown Seconds</Label>
              <input class="glass-input h-10 w-full px-3" type="number" bind:value={routingForm.cooldownSeconds} min="0" />
            </div>
            <div>
              <Label className="block mb-1">Priority</Label>
              <input class="glass-input h-10 w-full px-3" type="number" bind:value={routingForm.priority} min="0" />
            </div>
            <div class="flex items-center gap-2 pt-7">
              <input type="checkbox" class="aero-checkbox" bind:checked={routingForm.enabled} />
              <span class="text-sm">Enabled</span>
            </div>
          </div>

          <div class="baseline-panel p-4 space-y-4">
            <div class="flex items-center justify-between gap-4">
              <div>
                <div class="text-sm font-medium">Dry-run Candidate</div>
                <div class="mt-1 text-xs text-muted-foreground">
                  Test this rule against a baseline, drift, or event candidate before you enable it.
                </div>
              </div>
              <Button variant="outline" on:click={runRoutingRuleTest} disabled={routingTestLoading}>
                {#if routingTestLoading}
                  <RefreshCw class="h-4 w-4 mr-1 animate-spin" /> Testing...
                {:else}
                  Dry Run
                {/if}
              </Button>
            </div>

            {#if routingTestError}
              <div class="aero-alert-error">{routingTestError}</div>
            {/if}

            <div class="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-4">
              <div>
                <Label className="block mb-1">Candidate Domain</Label>
                <select class="glass-input h-10 w-full" bind:value={routingTestForm.domain}>
                  {#each routingDomainOptions as option}
                    <option value={option.value}>{option.label}</option>
                  {/each}
                </select>
              </div>
              <div class="xl:col-span-3">
                <Label className="block mb-1">Candidate Trigger Key</Label>
                <Input bind:value={routingTestForm.triggerKey} />
              </div>
              <div class="xl:col-span-2">
                <Label className="block mb-1">Current Value</Label>
                <Input bind:value={routingTestForm.currentValueText} placeholder='e.g. "false"' />
              </div>
              <div class="xl:col-span-2">
                <Label className="block mb-1">Previous Value</Label>
                <Input bind:value={routingTestForm.previousValueText} placeholder='e.g. "true"' />
              </div>
              <div>
                <Label className="block mb-1">Support Ratio</Label>
                <Input bind:value={routingTestForm.supportRatio} placeholder="0.9" />
              </div>
              <div>
                <Label className="block mb-1">Confidence Score</Label>
                <Input bind:value={routingTestForm.confidenceScore} placeholder="0.95" />
              </div>
              <div>
                <Label className="block mb-1">Scope Type</Label>
                <select class="glass-input h-10 w-full" bind:value={routingTestForm.scopeType}>
                  <option value="">Any</option>
                  {#each scopeTypeOptions as option}
                    <option value={option.value}>{option.label}</option>
                  {/each}
                </select>
              </div>
              <div>
                <Label className="block mb-1">Agent</Label>
                <select class="glass-input h-10 w-full" bind:value={routingTestForm.agentId}>
                  <option value="">Any device</option>
                  {#each catalog?.devices ?? [] as device}
                    <option value={device.agentId}>{device.hostname || device.agentId}</option>
                  {/each}
                </select>
              </div>
            </div>

            {#if routingTestResult}
              <div class="baseline-panel baseline-panel-subtle p-4">
                <div class="flex flex-wrap items-center gap-2">
                  <span class={`inline-flex rounded-full px-2 py-1 text-xs font-medium ${routingTestResult.wouldMatch && !routingTestResult.cooldownBlocked ? 'bg-green-100 text-green-800' : 'bg-gray-100 text-gray-700'}`}>
                    {routingTestResult.wouldMatch ? 'Would match' : 'No match'}
                  </span>
                  {#if routingTestResult.cooldownBlocked}
                    <span class="inline-flex rounded-full px-2 py-1 text-xs font-medium bg-amber-100 text-amber-800">Blocked by cooldown</span>
                  {/if}
                  <span class="inline-flex rounded-full px-2 py-1 text-xs font-medium bg-blue-100 text-blue-800">
                    Action: {routingTestResult.action}
                  </span>
                </div>
                <div class="mt-3 text-xs text-muted-foreground">
                  Dedupe key: <span class="font-mono">{routingTestResult.dedupeKey ?? '—'}</span>
                </div>
                {#if routingTestResult.blockedReasons.length > 0}
                  <div class="mt-3 flex flex-wrap gap-2">
                    {#each routingTestResult.blockedReasons as reason}
                      <span class="inline-flex rounded-full px-2 py-1 text-xs font-medium bg-amber-100 text-amber-800">
                        {routingBlockedReasonLabel(reason)}
                      </span>
                    {/each}
                  </div>
                {/if}
                {#if routingTestResult.explanation.length > 0}
                  <div class="mt-3 text-xs text-muted-foreground">
                    {routingTestResult.explanation.join(' • ')}
                  </div>
                {/if}
              </div>
            {/if}
          </div>

          <div class="flex flex-wrap gap-2">
            <Button on:click={saveRoutingRule} disabled={routingSaving}>
              {routingSaving ? 'Saving...' : routingFormMode === 'edit' ? 'Update Rule' : 'Create Rule'}
            </Button>
            <Button variant="outline" on:click={closeRoutingForm}>Cancel</Button>
          </div>
        </CardContent>
      </Card>
    {/if}

    <Card>
      <CardHeader>
        <CardTitle>Recent Decisions</CardTitle>
        <CardDescription>
          {#if selectedRoutingRuleId && routingDecisionAgentId}
            Recent decisions for rule <span class="font-mono">{selectedRoutingRuleId}</span> on device <span class="font-mono">{routingDecisionAgentId}</span>.
          {:else}
            Select or dry-run a rule with a device context to inspect recent executions.
          {/if}
        </CardDescription>
      </CardHeader>
      <CardContent>
        {#if routingDecisionsError}
          <div class="aero-alert-error">{routingDecisionsError}</div>
        {:else if routingDecisionsLoading}
          <div class="flex h-20 items-center justify-center">
            <RefreshCw class="h-5 w-5 animate-spin text-muted-foreground" />
          </div>
        {:else if routingDecisions.length === 0}
          <div class="text-sm aero-empty-state">No decisions loaded for the current rule/device context.</div>
        {:else}
          <div class="aero-table-wrap overflow-x-auto">
            <Table className="text-sm">
              <TableHeader>
                <TableRow>
                  <TableHead>When</TableHead>
                  <TableHead>Trigger</TableHead>
                  <TableHead>Action</TableHead>
                  <TableHead>Status</TableHead>
                  <TableHead>Outcome</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {#each routingDecisions as decision (decision.id)}
                  <TableRow>
                    <TableCell className="align-top text-xs text-muted-foreground">{dateLabel(decision.decidedAt)}</TableCell>
                    <TableCell className="align-top">
                      <div class="font-medium">{decision.domain}</div>
                      <div class="mt-1 font-mono text-xs text-muted-foreground">{decision.triggerKey}</div>
                    </TableCell>
                    <TableCell className="align-top">
                      <div>{decision.action}</div>
                      {#if decision.intentId}
                        <div class="mt-1 font-mono text-xs text-muted-foreground">{decision.intentId}</div>
                      {/if}
                    </TableCell>
                    <TableCell className="align-top">
                      <span class={`inline-flex rounded-full px-2 py-0.5 text-[11px] font-medium ${decisionExecutionClass(decision.executionStatus)}`}>
                        {decision.executionStatus}
                      </span>
                    </TableCell>
                    <TableCell className="align-top text-xs text-muted-foreground">
                      {decision.outcomeMessage ?? decision.reason ?? '—'}
                      {#if decision.externalRef}
                        <div class="mt-1 font-mono">{decision.externalRef}</div>
                      {/if}
                    </TableCell>
                  </TableRow>
                {/each}
              </TableBody>
            </Table>
          </div>
        {/if}
      </CardContent>
    </Card>
  </div>
  {/if}

  {#if activeTab === 'intents'}
  <Card>
    <CardHeader>
      <div class="flex items-center justify-between">
        <div>
          <CardTitle>Remediation Intents</CardTitle>
          <CardDescription>Define what actions to take when baselines shift or events fire. Create intents manually or describe what you want and let AI generate the steps.</CardDescription>
        </div>
        <Button on:click={openCreateIntent}>
          <Plus class="h-4 w-4 mr-1" /> New Intent
        </Button>
      </div>
    </CardHeader>
    <CardContent>
      {#if intentsError}
        <div class="mb-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          <AlertCircle class="h-4 w-4 inline mr-1" />{intentsError}
        </div>
      {/if}
      {#if intentsLoading}
        <div class="flex h-24 items-center justify-center">
          <RefreshCw class="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      {:else if intents.length === 0}
        <div class="text-center py-10 text-sm text-muted-foreground">
          <Zap class="h-8 w-8 mx-auto mb-2 text-gray-300" />
          <p>No intents yet. Create one to define automated remediation actions.</p>
          <p class="mt-1 text-xs">You can also create intents from the Baselines tab by clicking "Intent" on any fact row.</p>
        </div>
      {:else}
        <div class="space-y-3">
          {#each intents as intent (intent.id)}
            <div class="rounded-lg border p-4 {intent.enabled ? 'bg-white' : 'bg-gray-50 opacity-75'}">
              <div class="flex items-start justify-between">
                <div class="flex-1">
                  <div class="flex items-center gap-2">
                    <h3 class="font-semibold text-sm">{intent.name}</h3>
                    <span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium {intent.type === 'ai_planned' ? 'bg-purple-100 text-purple-800' : 'bg-blue-100 text-blue-800'}">
                      {intent.type === 'ai_planned' ? 'AI Planned' : 'Hardcoded'}
                    </span>
                    {#if !intent.enabled}
                      <span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium bg-gray-200 text-gray-600">Disabled</span>
                    {/if}
                    {#if intent.requiresApproval}
                      <span class="inline-flex rounded-full px-2 py-0.5 text-xs font-medium bg-amber-100 text-amber-800">Approval Required</span>
                    {/if}
                  </div>
                  {#if intent.description}
                    <p class="text-xs text-muted-foreground mt-1">{intent.description}</p>
                  {/if}
                  <div class="flex flex-wrap gap-3 mt-2 text-xs text-muted-foreground">
                    {#if intent.triggerDomain}
                      <span>Trigger: <code class="font-mono bg-gray-100 px-1 rounded">{intent.triggerDomain}:{intent.triggerKey ?? '*'}</code></span>
                    {/if}
                    <span>Retries: {intent.maxRetries}</span>
                    <span>Timeout: {intent.timeoutSeconds}s</span>
                    {#if intent.allowList && Array.isArray(intent.allowList)}
                      <span>Allow list: {intent.allowList.length} commands</span>
                    {/if}
                    {#if intent.steps && Array.isArray(intent.steps)}
                      <span>Steps: {intent.steps.length}</span>
                    {/if}
                  </div>
                </div>
                <div class="flex gap-1 ml-3">
                  <button
                    class="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-gray-700"
                    title={intent.enabled ? 'Disable' : 'Enable'}
                    on:click={() => toggleIntentEnabled(intent)}
                  >
                    <Zap class="h-4 w-4 {intent.enabled ? 'text-green-500' : 'text-gray-400'}" />
                  </button>
                  <button
                    class="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-blue-600"
                    title="Edit"
                    on:click={() => openEditIntent(intent)}
                  >
                    <Edit class="h-4 w-4" />
                  </button>
                  <button
                    class="p-1.5 rounded hover:bg-gray-100 text-gray-500 hover:text-red-600"
                    title="Delete"
                    on:click={() => deleteIntent(intent.id)}
                  >
                    <Trash2 class="h-4 w-4" />
                  </button>
                </div>
              </div>
            </div>
          {/each}
        </div>
      {/if}
    </CardContent>
  </Card>

  {#if showIntentForm}
  <Card>
    <CardHeader>
      <CardTitle>{intentFormMode === 'edit' ? 'Edit Intent' : 'Create Remediation Intent'}</CardTitle>
      <CardDescription>
        {#if intentFormMode === 'create'}
          Describe what you want the intent to do, or fill in the fields manually. In a future version, an AI model will generate the steps from your description.
        {:else}
          Update the intent configuration.
        {/if}
      </CardDescription>
    </CardHeader>
    <CardContent>
      {#if intentSaveError}
        <div class="mb-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          <AlertCircle class="h-4 w-4 inline mr-1" />{intentSaveError}
        </div>
      {/if}

      {#if intentFormMode === 'create'}
        <div class="mb-6 p-4 rounded-lg bg-purple-50 border border-purple-200">
          <Label for="intent-ai-description" className="block mb-1 font-medium text-purple-900">Describe the intent (AI-powered)</Label>
          <p class="text-xs text-purple-700 mb-2">Describe what should happen when this baseline shifts. In a future update, an LLM will generate the intent fields from this description. For now, use it as your planning notes.</p>
          <textarea
            id="intent-ai-description"
            class="glass-input w-full h-20 text-sm"
            placeholder="e.g., When the VPN client is uninstalled, check if the installer exists on the network share and silently reinstall it..."
            bind:value={intentForm.userPrompt}
          ></textarea>
        </div>
      {/if}

      <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
        <div>
          <Label for="intent-name" className="block mb-1">Name</Label>
          <Input id="intent-name" bind:value={intentForm.name} placeholder="e.g., vpn-reinstall" />
        </div>
        <div>
          <Label for="intent-type" className="block mb-1">Type</Label>
          <select id="intent-type" class="glass-input h-10 w-full" bind:value={intentForm.type}>
            <option value="hardcoded">Hardcoded Steps</option>
            <option value="ai_planned">AI Planned</option>
          </select>
        </div>
        <div class="md:col-span-2">
          <Label for="intent-desc" className="block mb-1">Description</Label>
          <Input id="intent-desc" bind:value={intentForm.description} placeholder="What does this intent do?" />
        </div>
        <div>
          <Label for="intent-trigger-domain" className="block mb-1">Trigger Domain</Label>
          <select id="intent-trigger-domain" class="glass-input h-10 w-full" bind:value={intentForm.triggerDomain}>
            <option value="">Any</option>
            <option value="baseline">Baseline Shift</option>
            <option value="scope_drift">Scope Drift</option>
            <option value="event">Event</option>
          </select>
        </div>
        <div>
          <Label for="intent-trigger-key" className="block mb-1">Trigger Key</Label>
          <Input id="intent-trigger-key" bind:value={intentForm.triggerKey} placeholder="e.g., app.cisco_anyconnect.installed" />
        </div>
        <div>
          <Label for="intent-retries" className="block mb-1">Max Retries</Label>
          <input id="intent-retries" type="number" class="glass-input h-10 w-full" bind:value={intentForm.maxRetries} />
        </div>
        <div>
          <Label for="intent-timeout" className="block mb-1">Timeout (seconds)</Label>
          <input id="intent-timeout" type="number" class="glass-input h-10 w-full" bind:value={intentForm.timeoutSeconds} />
        </div>
        <div class="md:col-span-2">
          <Label for="intent-allow-list" className="block mb-1">Command Allow List (one per line)</Label>
          <textarea
            id="intent-allow-list"
            class="glass-input w-full h-20 text-sm font-mono"
            placeholder="Get-Service&#10;Restart-Service&#10;Start-Service"
            bind:value={intentForm.allowListText}
          ></textarea>
        </div>
        <div class="md:col-span-2">
          <Label for="intent-steps" className="block mb-1">Steps (JSON array)</Label>
          <textarea
            id="intent-steps"
            class="glass-input w-full h-32 text-sm font-mono"
            placeholder={'[{"command": "Get-Service -Name WinDefend", "description": "Check service status"}]'}
            bind:value={intentForm.stepsText}
          ></textarea>
        </div>
        {#if intentForm.type === 'ai_planned'}
          <div class="md:col-span-2">
            <Label for="intent-ai-prompt" className="block mb-1">AI System Prompt</Label>
            <textarea
              id="intent-ai-prompt"
              class="glass-input w-full h-20 text-sm"
              placeholder="You are an expert Windows administrator. Diagnose and fix..."
              bind:value={intentForm.aiPrompt}
            ></textarea>
          </div>
        {/if}
        <div class="flex items-center gap-4">
          <label class="flex items-center gap-2 text-sm cursor-pointer">
            <input type="checkbox" bind:checked={intentForm.requiresApproval} class="aero-checkbox" />
            Requires Approval
          </label>
          <label class="flex items-center gap-2 text-sm cursor-pointer">
            <input type="checkbox" bind:checked={intentForm.enabled} class="aero-checkbox" />
            Enabled
          </label>
        </div>
      </div>

      <div class="mt-4 flex gap-2">
        <Button on:click={saveIntent} disabled={intentSaving || !intentForm.name.trim()}>
          {#if intentSaving}
            <RefreshCw class="h-4 w-4 animate-spin mr-1" />
          {/if}
          {intentFormMode === 'edit' ? 'Update Intent' : 'Create Intent'}
        </Button>
        <Button variant="outline" on:click={() => showIntentForm = false}>Cancel</Button>
      </div>
    </CardContent>
  </Card>
  {/if}
  {/if}

  {#if activeTab === 'overrides'}
  <Card>
    <CardHeader>
      <div class="flex items-center justify-between">
        <div>
          <CardTitle>Stability Overrides</CardTitle>
          <CardDescription>Override the default stability class for specific fact key patterns. Use this to suppress noisy baselines or force-track specific facts.</CardDescription>
        </div>
        <Button on:click={() => { overrideFormMode = 'create'; editingOverrideId = null; overrideForm = resetOverrideForm(); showOverrideForm = true; scheduleOverridePreview(); }}>
          <Plus class="h-4 w-4 mr-1" /> New Override
        </Button>
      </div>
    </CardHeader>
    <CardContent>
      {#if overridesError}
        <div class="mb-4 rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          <AlertCircle class="h-4 w-4 inline mr-1" />{overridesError}
        </div>
      {/if}

      {#if showOverrideForm}
        <div class="mb-4 baseline-panel baseline-panel-subtle p-4">
          <div class="mb-3">
            <div class="text-sm font-medium">{overrideFormMode === 'edit' ? 'Edit stability override' : 'Create stability override'}</div>
            <div class="text-xs text-muted-foreground mt-1">
              <code class="baseline-inline-code">stable</code> forces baseline eligibility, <code class="baseline-inline-code">noisy</code> keeps the fact in current state only, and <code class="baseline-inline-code">ignored</code> suppresses baseline promotion entirely.
            </div>
          </div>
          <div class="grid grid-cols-1 md:grid-cols-3 gap-3">
            <div>
              <Label for="override-pattern" className="block mb-1">Fact Key Pattern</Label>
              <Input id="override-pattern" bind:value={overrideForm.factKeyPattern} placeholder="e.g., network.adapter.*" on:input={scheduleOverridePreview} />
            </div>
            <div>
              <Label for="override-class" className="block mb-1">Stability Class</Label>
              <select id="override-class" class="glass-input h-10 w-full" bind:value={overrideForm.stabilityClass}>
                <option value="stable">Stable</option>
                <option value="noisy">Noisy</option>
                <option value="ignored">Ignored</option>
              </select>
            </div>
            <div>
              <Label for="override-reason" className="block mb-1">Reason</Label>
              <Input id="override-reason" bind:value={overrideForm.reason} placeholder="e.g., False positive from DHCP renewal" />
            </div>
          </div>
          <div class="mt-4 baseline-panel p-4">
            <div class="flex items-center justify-between gap-2">
              <div>
                <div class="text-sm font-medium">Override impact preview</div>
                <div class="text-xs text-muted-foreground mt-1">Matches are based on current facts and scoped baseline keys already seen in the organization.</div>
              </div>
              <Button variant="outline" on:click={() => loadOverridePreview(overrideForm.factKeyPattern)} disabled={!overrideForm.factKeyPattern.trim() || overridePreviewLoading}>
                {#if overridePreviewLoading}
                  <RefreshCw class="h-4 w-4 animate-spin mr-1" />
                {/if}
                Refresh Preview
              </Button>
            </div>

            {#if overridePreviewError}
              <div class="mt-3 aero-alert-error">{overridePreviewError}</div>
            {:else if overridePreview}
              <div class="mt-3 grid grid-cols-1 md:grid-cols-3 gap-3 text-sm">
                <div class="baseline-panel baseline-panel-subtle p-3">
                  <div class="text-xs uppercase tracking-wide text-muted-foreground">Matched Fact Keys</div>
                  <div class="mt-1 text-lg font-semibold">{overridePreview.matchedFactKeyCount}</div>
                </div>
                <div class="baseline-panel baseline-panel-subtle p-3">
                  <div class="text-xs uppercase tracking-wide text-muted-foreground">Current Facts</div>
                  <div class="mt-1 text-lg font-semibold">{overridePreview.matchedCurrentFactCount}</div>
                </div>
                <div class="baseline-panel baseline-panel-subtle p-3">
                  <div class="text-xs uppercase tracking-wide text-muted-foreground">Scoped Baselines</div>
                  <div class="mt-1 text-lg font-semibold">{overridePreview.matchedScopedBaselineCount}</div>
                </div>
              </div>

              {#if overridePreview.items.length > 0}
                <div class="mt-3 space-y-2">
                  {#each overridePreview.items as item (item.factKey)}
                    <div class="baseline-panel baseline-panel-subtle px-3 py-2 text-xs text-muted-foreground">
                      <div class="font-mono text-foreground">{item.factKey}</div>
                      <div class="mt-1">Current facts: {item.currentFactCount} • Scoped baselines: {item.scopedBaselineCount} • Last seen: {dateLabel(item.latestSeenAt)}</div>
                    </div>
                  {/each}
                </div>
              {:else}
                <div class="mt-3 text-sm text-muted-foreground">No matching fact keys found yet for this pattern.</div>
              {/if}
            {:else}
              <div class="mt-3 text-sm text-muted-foreground">Enter a fact key pattern to preview the impact before saving.</div>
            {/if}
          </div>
          <div class="mt-3 flex gap-2">
            <Button on:click={saveOverride} disabled={overrideSaving || !overrideForm.factKeyPattern.trim()}>
              {#if overrideSaving}<RefreshCw class="h-4 w-4 animate-spin mr-1" />{/if}
              {overrideFormMode === 'edit' ? 'Update Override' : 'Save Override'}
            </Button>
            <Button variant="outline" on:click={closeOverrideForm}>Cancel</Button>
          </div>
        </div>
      {/if}

      {#if overridesLoading}
        <div class="flex h-24 items-center justify-center">
          <RefreshCw class="h-6 w-6 animate-spin text-muted-foreground" />
        </div>
      {:else if overrides.length === 0}
        <div class="text-center py-10 text-sm text-muted-foreground">
          <Settings class="h-8 w-8 mx-auto mb-2 text-gray-300" />
          <p>No stability overrides configured.</p>
          <p class="mt-1 text-xs">Create overrides to adjust how specific facts are classified (stable, noisy, or ignored).</p>
        </div>
      {:else}
        <div class="baseline-panel overflow-x-auto">
          <Table className="text-sm">
            <TableHeader>
              <TableRow>
                <TableHead>Fact Key Pattern</TableHead>
                <TableHead className="w-[130px]">Stability Class</TableHead>
                <TableHead className="w-[220px]">Impact</TableHead>
                <TableHead>Reason</TableHead>
                <TableHead className="w-[160px]">Created</TableHead>
                <TableHead className="w-[110px]">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {#each overrides as override (override.id)}
                <TableRow>
                  <TableCell className="font-mono text-xs">{override.factKeyPattern}</TableCell>
                  <TableCell>
                    <span class={`inline-flex rounded-full px-2 py-0.5 text-xs font-medium ${stabilityClassClass(override.stabilityClass)}`}>
                      {override.stabilityClass}
                    </span>
                  </TableCell>
                  <TableCell className="text-xs text-muted-foreground">
                    <div>{override.matchedFactKeyCount} fact keys • {override.matchedCurrentFactCount} current facts</div>
                    <div class="mt-1">{override.matchedScopedBaselineCount} scoped baselines</div>
                    {#if override.sampleFactKeys.length > 0}
                      <div class="mt-2 flex flex-wrap gap-1">
                        {#each override.sampleFactKeys.slice(0, 3) as factKey}
                          <span class="baseline-token inline-flex rounded-full px-2 py-0.5 font-mono text-[11px]">
                            {factKey}
                          </span>
                        {/each}
                      </div>
                    {/if}
                  </TableCell>
                  <TableCell className="text-xs text-muted-foreground">{override.reason ?? '—'}</TableCell>
                  <TableCell className="text-xs text-muted-foreground">
                    <div>{dateLabel(override.createdAt)}</div>
                    <div class="mt-1">Updated {dateLabel(override.updatedAt)}</div>
                  </TableCell>
                  <TableCell>
                    <div class="flex gap-1">
                      <button
                        class="baseline-icon-btn p-1 rounded"
                        title="Edit override"
                        on:click={() => openEditOverride(override)}
                      >
                        <Edit class="h-4 w-4" />
                      </button>
                      <button
                        class="baseline-icon-btn baseline-icon-btn-danger p-1 rounded"
                        title="Delete override"
                        on:click={() => deleteOverride(override.id)}
                      >
                        <Trash2 class="h-4 w-4" />
                      </button>
                    </div>
                  </TableCell>
                </TableRow>
              {/each}
            </TableBody>
          </Table>
        </div>
      {/if}
    </CardContent>
  </Card>
  {/if}
</div>

<style>
  .baseline-panel {
    border-radius: 0.875rem;
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: rgba(255, 255, 255, 0.05);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.08),
      0 8px 28px rgba(0, 0, 0, 0.18);
    transition: border-color 0.2s ease, background 0.2s ease, box-shadow 0.2s ease;
  }

  .baseline-panel-subtle {
    background: rgba(255, 255, 255, 0.035);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.05);
  }

  .baseline-inline-code,
  .baseline-token {
    border: 1px solid rgba(255, 255, 255, 0.12);
    background: rgba(255, 255, 255, 0.07);
    color: rgba(220, 240, 255, 0.9);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06);
  }

  .baseline-inline-code {
    border-radius: 0.375rem;
    padding: 0.15rem 0.4rem;
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
    font-size: 0.75rem;
  }

  .baseline-token {
    backdrop-filter: blur(8px);
    -webkit-backdrop-filter: blur(8px);
  }

  .baseline-icon-btn {
    color: rgba(160, 205, 255, 0.68);
    transition: background 0.18s ease, color 0.18s ease;
  }

  .baseline-icon-btn:hover {
    background: rgba(255, 255, 255, 0.08);
    color: rgba(220, 240, 255, 0.95);
  }

  .baseline-icon-btn-danger:hover {
    color: rgba(255, 160, 160, 0.95);
  }

  :global(html.light) .baseline-panel {
    border-color: rgba(255, 255, 255, 0.58);
    background: rgba(255, 255, 255, 0.18);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.74),
      0 8px 26px rgba(0, 38, 120, 0.08);
  }

  :global(html.light) .baseline-panel-subtle {
    background: rgba(255, 255, 255, 0.12);
    border-color: rgba(255, 255, 255, 0.48);
  }

  :global(html.light) .baseline-inline-code,
  :global(html.light) .baseline-token {
    border-color: rgba(100, 158, 220, 0.28);
    background: rgba(255, 255, 255, 0.58);
    color: rgba(10, 42, 108, 0.82);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.72);
  }

  :global(html.light) .baseline-icon-btn {
    color: rgba(10, 42, 108, 0.52);
  }

  :global(html.light) .baseline-icon-btn:hover {
    background: rgba(50, 100, 200, 0.08);
    color: rgba(20, 76, 196, 0.92);
  }

  :global(html.light) .baseline-icon-btn-danger:hover {
    color: rgba(190, 45, 45, 0.92);
  }
</style>
