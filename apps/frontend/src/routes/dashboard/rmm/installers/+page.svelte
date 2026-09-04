<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
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
  import { customerApi, installerApi, siteApi } from '$lib/api';
  import { detectViewerInstallerPlatform } from '$lib/viewer-launcher';
  import type {
    Customer,
    Site,
    LinuxAgentInstallerInfo,
    MacosPackageInstallerInfo,
    RmmInstallerDownloadResponse,
    RmmLinuxInstallerResponse,
    RmmMacosInstallerResponse,
    RmmInstallerProfile,
    RmmInstallerScopeType,
    ViewerInstallerInfo,
  } from '$lib/types';

  let profiles: RmmInstallerProfile[] = [];
  let customers: Customer[] = [];
  let sites: Site[] = [];
  let loading = true;
  let error: string | null = null;
  let activeTab: 'agent' | 'viewer' = 'agent';
  let viewerInfo: ViewerInstallerInfo | null = null;
  let linuxInfo: LinuxAgentInstallerInfo | null = null;
  let macosInfo: MacosPackageInstallerInfo | null = null;
  let viewerDownloading = false;

  let creating = false;
  let issuingProfileId: string | null = null;
  let downloadingMacosPackageProfileId: string | null = null;
  let issuingLinuxProfileId: string | null = null;
  let issuingMacosProfileId: string | null = null;
  let revokingProfileId: string | null = null;

  let scopeType: RmmInstallerScopeType = 'organization';
  let selectedCustomerId = '';
  let selectedSiteId = '';
  let profileName = '';
  let profileExpiresAt = '';
  let profileMaxUses = '';

  let downloadExpiresAt = '';
  let downloadMaxUses = '';
  let sitesForSelectedCustomer: Site[] = [];

  let latestDownload: RmmInstallerDownloadResponse | null = null;
  let latestLinuxInstall: RmmLinuxInstallerResponse | null = null;
  let latestMacosInstall: RmmMacosInstallerResponse | null = null;
  let actionMessage: string | null = null;

  const parseScopeType = (value: string | null): RmmInstallerScopeType => {
    if (value === 'customer' || value === 'site') return value;
    return 'organization';
  };

  const toIsoOrNull = (value: string): string | null => {
    const trimmed = value.trim();
    if (!trimmed) return null;
    const parsed = new Date(trimmed);
    return Number.isNaN(parsed.getTime()) ? null : parsed.toISOString();
  };

  const toMaxUsesOrNull = (value: string): number | null => {
    const trimmed = value.trim();
    if (!trimmed) return null;
    const parsed = Number(trimmed);
    if (!Number.isInteger(parsed) || parsed <= 0) return null;
    return parsed;
  };

  const formatDate = (value?: string | null) => {
    if (!value) return 'Never';
    return new Date(value).toLocaleString();
  };

  const formatScope = (profile: RmmInstallerProfile) => {
    if (profile.scopeType === 'site')
      return `Site: ${profile.siteName ?? profile.siteId ?? 'Unknown'}`;
    if (profile.scopeType === 'customer')
      return `Customer: ${profile.customerName ?? profile.customerId ?? 'Unknown'}`;
    return 'Organization';
  };

  const saveTextFile = (filename: string, content: string) => {
    const blob = new Blob([content], { type: 'application/json' });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const saveBlobFile = (filename: string, blob: Blob) => {
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const refreshData = async () => {
    loading = true;
    error = null;
    try {
      const [
        profileRows,
        customerRows,
        siteRows,
        viewerInstallerInfo,
        linuxInstallerInfo,
        macosInstallerInfo,
      ] = await Promise.all([
        installerApi.listProfiles(),
        customerApi.getCustomers(),
        siteApi.getSites(),
        installerApi.getViewerInstallerInfo(detectViewerInstallerPlatform()),
        installerApi.getLinuxAgentInfo(),
        installerApi.getMacosPackageInfo(),
      ]);
      profiles = profileRows;
      customers = customerRows;
      sites = siteRows;
      viewerInfo = viewerInstallerInfo;
      linuxInfo = linuxInstallerInfo;
      macosInfo = macosInstallerInfo;
    } catch (err) {
      console.error('Failed to load installer data', err);
      error = err instanceof Error ? err.message : 'Failed to load installer data';
    } finally {
      loading = false;
    }
  };

  const applyPrefillFromQuery = () => {
    scopeType = parseScopeType($page.url.searchParams.get('scopeType'));
    const customerId = ($page.url.searchParams.get('customerId') || '').trim();
    const siteId = ($page.url.searchParams.get('siteId') || '').trim();
    if (customerId) {
      selectedCustomerId = customerId;
    }
    if (siteId) {
      selectedSiteId = siteId;
      if (scopeType === 'organization') {
        scopeType = 'site';
      }
    }
    if (scopeType === 'customer' && !selectedCustomerId && customerId) {
      selectedCustomerId = customerId;
    }
  };

  const createProfile = async () => {
    if (scopeType === 'customer' && !selectedCustomerId) {
      alert('Select a customer for customer-scoped installers.');
      return;
    }
    if (scopeType === 'site' && !selectedSiteId) {
      alert('Select a site for site-scoped installers.');
      return;
    }

    const expiresAt = toIsoOrNull(profileExpiresAt);
    if (profileExpiresAt.trim() && !expiresAt) {
      alert('Profile expiry must be a valid date/time.');
      return;
    }
    const maxUses = toMaxUsesOrNull(profileMaxUses);
    if (profileMaxUses.trim() && maxUses === null) {
      alert('Profile max uses must be a positive integer.');
      return;
    }

    creating = true;
    actionMessage = null;
    try {
      const created = await installerApi.createProfile({
        scopeType,
        customerId: scopeType === 'organization' ? undefined : selectedCustomerId || undefined,
        siteId: scopeType === 'site' ? selectedSiteId || undefined : undefined,
        name: profileName.trim() || undefined,
        expiresAt,
        maxUses,
      });
      actionMessage = `Created installer profile "${created.profile.name}"`;
      latestDownload = null;
      latestLinuxInstall = null;
      latestMacosInstall = null;
      await refreshData();
    } catch (err: any) {
      alert(err?.message || 'Failed to create installer profile');
    } finally {
      creating = false;
    }
  };

  const issueDownload = async (profile: RmmInstallerProfile) => {
    const expiresAt = toIsoOrNull(downloadExpiresAt);
    if (downloadExpiresAt.trim() && !expiresAt) {
      alert('Download expiry must be a valid date/time.');
      return;
    }
    const maxUses = toMaxUsesOrNull(downloadMaxUses);
    if (downloadMaxUses.trim() && maxUses === null) {
      alert('Download max uses must be a positive integer.');
      return;
    }

    issuingProfileId = profile.id;
    actionMessage = null;
    try {
      const issued = await installerApi.issueDownload(profile.id, {
        expiresAt,
        maxUses,
      });
      latestDownload = issued;
      latestLinuxInstall = null;
      latestMacosInstall = null;
      actionMessage = `Issued token payload for "${profile.name}"`;
      saveTextFile(issued.filename, JSON.stringify(issued.payload, null, 2));
      await refreshData();
    } catch (err: any) {
      alert(err?.message || 'Failed to issue installer download');
    } finally {
      issuingProfileId = null;
    }
  };

  const issueLinuxInstaller = async (profile: RmmInstallerProfile) => {
    const expiresAt = toIsoOrNull(downloadExpiresAt);
    if (downloadExpiresAt.trim() && !expiresAt) {
      alert('Download expiry must be a valid date/time.');
      return;
    }
    const maxUses = toMaxUsesOrNull(downloadMaxUses);
    if (downloadMaxUses.trim() && maxUses === null) {
      alert('Download max uses must be a positive integer.');
      return;
    }

    issuingLinuxProfileId = profile.id;
    actionMessage = null;
    try {
      const issued = await installerApi.issueLinuxInstaller(profile.id, {
        expiresAt,
        maxUses,
      });
      latestLinuxInstall = issued;
      latestMacosInstall = null;
      latestDownload = null;
      actionMessage = `Generated Linux install command for "${profile.name}"`;
      await refreshData();
    } catch (err: any) {
      alert(err?.message || 'Failed to generate Linux install command');
    } finally {
      issuingLinuxProfileId = null;
    }
  };

  const issueMacosInstaller = async (profile: RmmInstallerProfile) => {
    const expiresAt = toIsoOrNull(downloadExpiresAt);
    if (downloadExpiresAt.trim() && !expiresAt) {
      alert('Download expiry must be a valid date/time.');
      return;
    }
    const maxUses = toMaxUsesOrNull(downloadMaxUses);
    if (downloadMaxUses.trim() && maxUses === null) {
      alert('Download max uses must be a positive integer.');
      return;
    }

    issuingMacosProfileId = profile.id;
    actionMessage = null;
    try {
      const issued = await installerApi.issueMacosInstaller(profile.id, {
        expiresAt,
        maxUses,
      });
      latestMacosInstall = issued;
      latestLinuxInstall = null;
      latestDownload = null;
      actionMessage = `Generated macOS install command for "${profile.name}"`;
      await refreshData();
    } catch (err: any) {
      alert(err?.message || 'Failed to generate macOS install command');
    } finally {
      issuingMacosProfileId = null;
    }
  };

  const downloadMacosPackage = async (profile: RmmInstallerProfile) => {
    const expiresAt = toIsoOrNull(downloadExpiresAt);
    if (downloadExpiresAt.trim() && !expiresAt) {
      alert('Download expiry must be a valid date/time.');
      return;
    }
    const maxUses = toMaxUsesOrNull(downloadMaxUses);
    if (downloadMaxUses.trim() && maxUses === null) {
      alert('Download max uses must be a positive integer.');
      return;
    }

    downloadingMacosPackageProfileId = profile.id;
    actionMessage = null;
    try {
      const result = await installerApi.downloadMacosPackage(profile.id, {
        expiresAt,
        maxUses,
      });
      saveBlobFile(result.filename, result.blob);
      latestDownload = null;
      latestLinuxInstall = null;
      latestMacosInstall = null;
      actionMessage = `Downloaded tokenized macOS package for "${profile.name}"`;
      await refreshData();
    } catch (err: any) {
      alert(err?.message || 'Failed to download macOS package');
    } finally {
      downloadingMacosPackageProfileId = null;
    }
  };

  const revokeProfile = async (profile: RmmInstallerProfile) => {
    if (profile.revokedAt) return;
    if (!confirm(`Revoke installer profile "${profile.name}"?`)) return;
    revokingProfileId = profile.id;
    actionMessage = null;
    try {
      await installerApi.revokeProfile(profile.id);
      latestDownload = null;
      latestLinuxInstall = null;
      latestMacosInstall = null;
      actionMessage = `Revoked "${profile.name}"`;
      await refreshData();
    } catch (err: any) {
      alert(err?.message || 'Failed to revoke installer profile');
    } finally {
      revokingProfileId = null;
    }
  };

  const copyValue = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      actionMessage = 'Copied to clipboard';
    } catch {
      actionMessage = 'Clipboard copy failed';
    }
  };

  const copyLatestLinuxInstallCommand = async () => {
    if (!latestLinuxInstall) return;
    await copyValue(latestLinuxInstall.linuxInstallCommand);
  };

  const copyLatestMacosInstallCommand = async () => {
    if (!latestMacosInstall) return;
    await copyValue(latestMacosInstall.macosInstallCommand);
  };

  const downloadViewerInstaller = async () => {
    viewerDownloading = true;
    actionMessage = null;
    try {
      const result = await installerApi.downloadViewerInstaller(detectViewerInstallerPlatform());
      saveBlobFile(result.filename, result.blob);
      actionMessage = `Downloaded ${result.filename}`;
      await refreshData();
    } catch (err: any) {
      alert(err?.message || 'Failed to download viewer installer');
    } finally {
      viewerDownloading = false;
    }
  };

  $: if (scopeType === 'organization') {
    selectedCustomerId = '';
    selectedSiteId = '';
  }

  $: if (scopeType === 'customer') {
    selectedSiteId = '';
  }

  $: sitesForSelectedCustomer = selectedCustomerId
    ? sites.filter((site) => site.customerId === selectedCustomerId)
    : sites;

  onMount(async () => {
    applyPrefillFromQuery();
    await refreshData();
  });
</script>

<div class="space-y-6">
  <div>
    <h1 class="text-3xl font-bold aero-gradient-text">Installers</h1>
    <p class="text-sm aero-muted mt-1">
      Download the Talos agent and viewer installers from one place.
    </p>
  </div>

  <div class="inline-flex rounded-xl border border-white/10 bg-white/5 p-1">
    <Button
      variant={activeTab === 'agent' ? 'default' : 'ghost'}
      size="sm"
      on:click={() => {
        activeTab = 'agent';
      }}
    >
      Agent
    </Button>
    <Button
      variant={activeTab === 'viewer' ? 'default' : 'ghost'}
      size="sm"
      on:click={() => {
        activeTab = 'viewer';
      }}
    >
      Viewer
    </Button>
  </div>

  {#if activeTab === 'agent'}
    <Card>
      <CardHeader>
        <CardTitle>Create Installer Profile</CardTitle>
        <CardDescription>
          Default behavior is evergreen: no expiry and unlimited uses unless you set constraints.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div class="grid gap-4 md:grid-cols-3">
          <div class="space-y-2">
            <Label for="scopeType">Scope</Label>
            <select id="scopeType" bind:value={scopeType} class="glass-input w-full">
              <option value="organization">Organization</option>
              <option value="customer">Customer</option>
              <option value="site">Site</option>
            </select>
          </div>

          {#if scopeType !== 'organization'}
            <div class="space-y-2">
              <Label for="customerId">Customer</Label>
              <select id="customerId" bind:value={selectedCustomerId} class="glass-input w-full">
                <option value="">Select customer</option>
                {#each customers.filter((customer) => !customer.isUnassigned) as customer}
                  <option value={customer.id}>{customer.name}</option>
                {/each}
              </select>
            </div>
          {/if}

          {#if scopeType === 'site'}
            <div class="space-y-2">
              <Label for="siteId">Site</Label>
              <select id="siteId" bind:value={selectedSiteId} class="glass-input w-full">
                <option value="">Select site</option>
                {#each sitesForSelectedCustomer as site}
                  <option value={site.id}>{site.name}</option>
                {/each}
              </select>
            </div>
          {/if}
        </div>

        <div class="grid gap-4 md:grid-cols-3 mt-4">
          <div class="space-y-2">
            <Label for="profileName">Profile Name (optional)</Label>
            <Input id="profileName" bind:value={profileName} placeholder="Acme Site A Installer" />
          </div>
          <div class="space-y-2">
            <Label for="profileExpiresAt">Profile Expiry (optional)</Label>
            <Input id="profileExpiresAt" type="datetime-local" bind:value={profileExpiresAt} />
          </div>
          <div class="space-y-2">
            <Label for="profileMaxUses">Profile Max Uses (optional)</Label>
            <Input id="profileMaxUses" bind:value={profileMaxUses} placeholder="Unlimited" />
          </div>
        </div>

        <div class="flex justify-end mt-4">
          <Button on:click={createProfile} disabled={creating}>
            {creating ? 'Creating...' : 'Create Profile'}
          </Button>
        </div>
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle>Token Override For Downloads</CardTitle>
        <CardDescription>
          Leave blank for evergreen download tokens. Set values to generate expiring or max-use
          installers.
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div class="grid gap-4 md:grid-cols-2">
          <div class="space-y-2">
            <Label for="downloadExpiresAt">Download Expiry (optional)</Label>
            <Input id="downloadExpiresAt" type="datetime-local" bind:value={downloadExpiresAt} />
          </div>
          <div class="space-y-2">
            <Label for="downloadMaxUses">Download Max Uses (optional)</Label>
            <Input id="downloadMaxUses" bind:value={downloadMaxUses} placeholder="Unlimited" />
          </div>
        </div>
      </CardContent>
    </Card>

    <Card>
      <CardHeader>
        <CardTitle>Installer Profiles</CardTitle>
        <CardDescription
          >Generate scoped enrollment material and Linux/macOS install commands. A publishable
          Windows bootstrapper is not available until an operator configures the separately signed
          bootstrapper flow.</CardDescription
        >
      </CardHeader>
      <CardContent>
        {#if linuxInfo && !linuxInfo.available}
          <div class="aero-alert-error mb-4">
            Linux agent artifact unavailable: {linuxInfo.error ??
              'Build and mount the Linux agent binary before issuing Linux installs.'}
          </div>
        {/if}
        {#if macosInfo && !macosInfo.available}
          <div class="aero-alert-error mb-4">
            macOS package unavailable: {macosInfo.error ??
              'Build and mount the macOS package before issuing macOS installs.'}
          </div>
        {/if}
        {#if loading}
          <div class="flex items-center justify-center h-24">
            <div
              class="animate-spin rounded-full h-6 w-6 border-b-2"
              style="border-color: rgba(55,130,255,0.8)"
            ></div>
          </div>
        {:else if error}
          <div class="aero-alert-error">{error}</div>
        {:else if profiles.length === 0}
          <p class="text-sm aero-empty-state">No profiles yet.</p>
        {:else}
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Scope</TableHead>
                <TableHead>Profile Expiry</TableHead>
                <TableHead>Latest Token</TableHead>
                <TableHead>Status</TableHead>
                <TableHead className="text-right">Actions</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {#each profiles as profile}
                <TableRow>
                  <TableCell className="font-medium">{profile.name}</TableCell>
                  <TableCell>{formatScope(profile)}</TableCell>
                  <TableCell>{formatDate(profile.expiresAt)}</TableCell>
                  <TableCell>
                    {#if profile.latestToken}
                      <div class="text-xs">
                        <div>Prefix: {profile.latestToken.tokenPrefix}</div>
                        <div>
                          Uses: {profile.latestToken.usedCount}
                          {#if profile.latestToken.maxUses !== null}
                            / {profile.latestToken.maxUses}
                          {:else}
                            / unlimited
                          {/if}
                        </div>
                      </div>
                    {:else}
                      <span class="text-muted-foreground">—</span>
                    {/if}
                  </TableCell>
                  <TableCell>
                    {#if profile.revokedAt}
                      <span class="aero-badge-red">Revoked</span>
                    {:else}
                      <span class="aero-badge-online">Active</span>
                    {/if}
                  </TableCell>
                  <TableCell className="text-right">
                    <div class="flex justify-end gap-2">
                      <Button
                        variant="outline"
                        disabled={!!profile.revokedAt ||
                          !linuxInfo?.available ||
                          issuingLinuxProfileId === profile.id ||
                          issuingMacosProfileId === profile.id ||
                          downloadingMacosPackageProfileId === profile.id}
                        on:click={() => issueLinuxInstaller(profile)}
                      >
                        {issuingLinuxProfileId === profile.id ? 'Generating...' : 'Linux Script'}
                      </Button>
                      <Button
                        variant="outline"
                        disabled={!!profile.revokedAt ||
                          !macosInfo?.available ||
                          downloadingMacosPackageProfileId === profile.id ||
                          issuingMacosProfileId === profile.id ||
                          issuingLinuxProfileId === profile.id}
                        on:click={() => downloadMacosPackage(profile)}
                      >
                        {downloadingMacosPackageProfileId === profile.id
                          ? 'Downloading...'
                          : 'macOS PKG'}
                      </Button>
                      <Button
                        variant="outline"
                        disabled={!!profile.revokedAt ||
                          !macosInfo?.available ||
                          issuingMacosProfileId === profile.id ||
                          issuingLinuxProfileId === profile.id ||
                          downloadingMacosPackageProfileId === profile.id}
                        on:click={() => issueMacosInstaller(profile)}
                      >
                        {issuingMacosProfileId === profile.id ? 'Generating...' : 'macOS Script'}
                      </Button>
                      <Button
                        variant="outline"
                        disabled={!!profile.revokedAt ||
                          issuingProfileId === profile.id ||
                          issuingLinuxProfileId === profile.id ||
                          issuingMacosProfileId === profile.id ||
                          downloadingMacosPackageProfileId === profile.id}
                        on:click={() => issueDownload(profile)}
                      >
                        {issuingProfileId === profile.id ? 'Issuing...' : 'Issue Token'}
                      </Button>
                      <Button
                        variant="destructive"
                        disabled={!!profile.revokedAt ||
                          revokingProfileId === profile.id ||
                          issuingLinuxProfileId === profile.id ||
                          issuingMacosProfileId === profile.id ||
                          downloadingMacosPackageProfileId === profile.id}
                        on:click={() => revokeProfile(profile)}
                      >
                        {revokingProfileId === profile.id ? 'Revoking...' : 'Revoke'}
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              {/each}
            </TableBody>
          </Table>
        {/if}
      </CardContent>
    </Card>

    {#if actionMessage}
      <p class="text-sm text-muted-foreground">{actionMessage}</p>
    {/if}

    {#if latestLinuxInstall}
      <Card>
        <CardHeader>
          <CardTitle>Linux Install Command</CardTitle>
          <CardDescription>
            Run this on a Debian-based endpoint with sudo. The short link expires after seven days.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div class="space-y-4">
            <pre
              class="overflow-x-auto rounded-lg border border-white/10 bg-black/30 p-4 text-xs text-[rgba(232,244,255,0.9)]"><code
                >{latestLinuxInstall.linuxInstallCommand}</code
              ></pre>
            <div class="flex flex-wrap gap-2">
              <Button variant="outline" on:click={copyLatestLinuxInstallCommand}>
                Copy Command
              </Button>
            </div>
            <div class="grid gap-3 text-xs">
              <div>
                <p class="text-sm font-bold aero-detail-label">Expiry</p>
                <p class="aero-detail-value break-all font-bold">
                  {formatDate(latestLinuxInstall.linuxShortScriptExpiresAt)}
                </p>
              </div>
            </div>
          </div>
        </CardContent>
      </Card>
    {/if}

    {#if latestMacosInstall}
      <Card>
        <CardHeader>
          <CardTitle>macOS Install Command</CardTitle>
          <CardDescription>
            Run this on a macOS endpoint with sudo. The short link expires after seven days.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div class="space-y-4">
            <pre
              class="overflow-x-auto rounded-lg border border-white/10 bg-black/30 p-4 text-xs text-[rgba(232,244,255,0.9)]"><code
                >{latestMacosInstall.macosInstallCommand}</code
              ></pre>
            <div class="flex flex-wrap gap-2">
              <Button variant="outline" on:click={copyLatestMacosInstallCommand}>
                Copy Command
              </Button>
            </div>
            <div class="grid gap-3 text-xs">
              <div>
                <p class="text-sm font-bold aero-detail-label">Expiry</p>
                <p class="aero-detail-value break-all font-bold">
                  {formatDate(latestMacosInstall.macosShortScriptExpiresAt)}
                </p>
              </div>
            </div>
          </div>
        </CardContent>
      </Card>
    {/if}

    {#if latestDownload}
      <Card>
        <CardHeader>
          <CardTitle>Latest Issued Token</CardTitle>
          <CardDescription>
            Advanced/manual path: use this token with installer arguments if needed.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div class="space-y-3">
            <div>
              <p class="text-sm font-medium aero-detail-label">Enrollment Blob (base64url)</p>
              <p class="text-xs aero-detail-value break-all">{latestDownload.enrollmentBlob}</p>
              <div class="mt-2">
                <Button
                  variant="outline"
                  on:click={() => latestDownload && copyValue(latestDownload.enrollmentBlob)}
                >
                  Copy Blob
                </Button>
              </div>
            </div>
            <div>
              <p class="text-sm font-medium aero-detail-label">Raw Registration Token</p>
              <p class="text-xs aero-detail-value break-all">
                {latestDownload.issuedToken.token ?? 'Hidden'}
              </p>
              {#if latestDownload.issuedToken.token}
                <div class="mt-2">
                  <Button
                    variant="outline"
                    on:click={() =>
                      latestDownload?.issuedToken.token &&
                      copyValue(latestDownload.issuedToken.token)}
                  >
                    Copy Token
                  </Button>
                </div>
              {/if}
            </div>
          </div>
        </CardContent>
      </Card>
    {/if}
  {:else}
    <Card>
      <CardHeader>
        <CardTitle>Talos Viewer Installer</CardTitle>
        <CardDescription>
          Static desktop viewer installer and deep-link registration for this operating system.
        </CardDescription>
      </CardHeader>
      <CardContent>
        {#if loading}
          <div class="flex items-center justify-center h-24">
            <div
              class="animate-spin rounded-full h-6 w-6 border-b-2"
              style="border-color: rgba(55,130,255,0.8)"
            ></div>
          </div>
        {:else if error}
          <div class="aero-alert-error">{error}</div>
        {:else if !viewerInfo?.available}
          <div class="space-y-3">
            <div class="aero-alert-error">
              {viewerInfo?.error ?? 'Viewer installer artifacts are not available.'}
            </div>
            <p class="text-sm aero-muted">
              Build the viewer installer artifacts and mount them into the API container via the
              viewer artifact env vars.
            </p>
          </div>
        {:else}
          <div class="space-y-4">
            <div class="aero-alert-error">
              {#if detectViewerInstallerPlatform() === 'windows'}
                Initial official Community binaries are intentionally unsigned. Windows may show
                Unknown publisher or SmartScreen/reputation warnings. Verify the SHA-256 below
                against the release's SHA256SUMS and follow your organisation's approval policy; do
                not disable security controls globally.
              {:else}
                No Apple notarization is claimed for Community artifacts. Do not treat this package
                as an official macOS release unless its release notes include notarization,
                stapling, Gatekeeper, checksum, and provenance evidence.
              {/if}
            </div>
            <div class="rounded-xl border border-white/10 bg-white/5 p-4 text-sm">
              <div>
                <span class="font-semibold">File:</span>
                {viewerInfo.installer?.fileName ??
                  (detectViewerInstallerPlatform() === 'macos'
                    ? 'Talos.Viewer.macos.pkg'
                    : 'Talos.Viewer.x64.msi')}
              </div>
              <div>
                <span class="font-semibold">Profile:</span>
                {viewerInfo.profile ?? 'Unknown'}
              </div>
              <div>
                <span class="font-semibold">Generated:</span>
                {formatDate(viewerInfo.generatedAtUtc)}
              </div>
              <div>
                <span class="font-semibold">Size:</span>
                {viewerInfo.installer
                  ? `${Math.round((viewerInfo.installer.sizeBytes / 1024 / 1024) * 10) / 10} MB`
                  : 'Unknown'}
              </div>
              <div class="break-all">
                <span class="font-semibold">SHA-256:</span>
                {viewerInfo.installer?.sha256 ?? 'Unknown'}
              </div>
            </div>

            <div
              class="rounded-xl border border-sky-400/15 bg-sky-400/5 p-4 text-sm text-[rgba(210,232,255,0.82)]"
            >
              Install this on workstations that initiate remote desktop, shell, file transfer, or
              registry sessions. Windows installs register the <code>rmm://</code> handler through
              WiX. macOS installs place Talos Viewer in <code>/Applications</code> and register the same
              handler through the app bundle.
            </div>

            <div class="flex justify-end">
              <Button on:click={downloadViewerInstaller} disabled={viewerDownloading}>
                {viewerDownloading ? 'Downloading...' : 'Download Viewer Installer'}
              </Button>
            </div>
          </div>
        {/if}
      </CardContent>
    </Card>
  {/if}
</div>
