<script lang="ts">
  import { env } from '$env/dynamic/public';
  import { ExternalLink, Info, LockKeyhole, ShieldAlert } from 'lucide-svelte';
  import { resolveOperatorContent } from '$lib/operatorContent';

  const operator = resolveOperatorContent(env);
  const operatorName = operator.name ?? 'The operator of this deployment';
</script>

<svelte:head>
  <title>Operator privacy notice | Talos</title>
</svelte:head>

<main class="mx-auto max-w-3xl space-y-8 px-6 py-12 text-white/70">
  <header class="space-y-4 text-center">
    <div class="flex justify-center"><LockKeyhole class="h-14 w-14 text-emerald-300" /></div>
    <h1 class="text-4xl font-bold text-white">Operator privacy notice</h1>
    <p class="mx-auto max-w-2xl text-white/55">
      Talos Community Edition does not know who operates a particular installation or what legal
      obligations, retention rules, subprocessors, and deployment controls apply to it.
    </p>
  </header>

  <section class="rounded-xl border border-amber-300/25 bg-amber-300/8 p-5">
    <div class="flex gap-3">
      <ShieldAlert class="mt-0.5 h-5 w-5 shrink-0 text-amber-200" />
      <div class="space-y-2">
        <h2 class="font-semibold text-amber-100">This is not a privacy policy</h2>
        <p class="text-sm leading-6 text-amber-100/75">
          The source distribution intentionally makes no claim about a deployment's data controller,
          lawful basis, retention period, regulatory compliance, or response process. Those
          statements must come from the deployment operator after a deployment-specific legal and
          security review.
        </p>
      </div>
    </div>
  </section>

  <section class="space-y-4 rounded-xl border border-white/10 bg-white/5 p-6">
    <div class="flex items-center gap-2">
      <Info class="h-5 w-5 text-sky-300" />
      <h2 class="text-xl font-semibold text-white">Deployment information</h2>
    </div>
    <p>
      {operatorName} is responsible for explaining how this installation handles account data, device
      inventory, telemetry, remote-session information, command evidence, logs, backups, and any configured
      third-party integrations.
    </p>

    {#if operator.privacyUrl}
      <a
        class="inline-flex items-center gap-2 text-sky-300 hover:text-sky-200"
        href={operator.privacyUrl}
        target="_blank"
        rel="noopener noreferrer"
      >
        Read the operator's privacy notice
        <ExternalLink class="h-4 w-4" />
      </a>
    {:else}
      <p class="rounded-lg border border-white/10 bg-black/15 p-4 text-sm text-white/55">
        No operator privacy URL is configured. Ask the deployment administrator for its privacy
        notice before supplying personal or customer data. Operators can configure
        <code>PUBLIC_PRIVACY_URL</code> without modifying this page.
      </p>
    {/if}
  </section>
</main>
