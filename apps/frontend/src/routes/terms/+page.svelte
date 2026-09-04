<script lang="ts">
  import { env } from '$env/dynamic/public';
  import { ExternalLink, FileWarning, Scale, ServerCog } from 'lucide-svelte';
  import { resolveOperatorContent } from '$lib/operatorContent';

  const operator = resolveOperatorContent(env);
  const operatorName = operator.name ?? 'The operator of this deployment';
</script>

<svelte:head>
  <title>Operator terms | Talos</title>
</svelte:head>

<main class="mx-auto max-w-3xl space-y-8 px-6 py-12 text-white/70">
  <header class="space-y-4 text-center">
    <div class="flex justify-center"><Scale class="h-14 w-14 text-sky-300" /></div>
    <h1 class="text-4xl font-bold text-white">Operator terms</h1>
    <p class="mx-auto max-w-2xl text-white/55">
      Community Edition is software that an independent operator deploys. The source repository does
      not provide a hosted service, uptime promise, payment agreement, or support contract.
    </p>
  </header>

  <section class="rounded-xl border border-amber-300/25 bg-amber-300/8 p-5">
    <div class="flex gap-3">
      <FileWarning class="mt-0.5 h-5 w-5 shrink-0 text-amber-200" />
      <div class="space-y-2">
        <h2 class="font-semibold text-amber-100">No default service terms are supplied</h2>
        <p class="text-sm leading-6 text-amber-100/75">
          This configuration notice is not a contract or legal advice. A deployment operator must
          publish terms appropriate to its organization, jurisdiction, service model, and users.
          Software copying and modification are governed separately by the license distributed with
          the source code.
        </p>
      </div>
    </div>
  </section>

  <section class="space-y-4 rounded-xl border border-white/10 bg-white/5 p-6">
    <div class="flex items-center gap-2">
      <ServerCog class="h-5 w-5 text-sky-300" />
      <h2 class="text-xl font-semibold text-white">Deployment responsibility</h2>
    </div>
    <p>
      {operatorName} is responsible for authorizing remote-management activity, securing the deployment,
      defining acceptable use, maintaining backups, handling support, and supplying any warranties or
      service commitments that apply to its users.
    </p>

    {#if operator.termsUrl}
      <a
        class="inline-flex items-center gap-2 text-sky-300 hover:text-sky-200"
        href={operator.termsUrl}
        target="_blank"
        rel="noopener noreferrer"
      >
        Read the operator's terms
        <ExternalLink class="h-4 w-4" />
      </a>
    {:else}
      <p class="rounded-lg border border-white/10 bg-black/15 p-4 text-sm text-white/55">
        No operator terms URL is configured. Ask the deployment administrator which policies apply
        before using remote-management features. Operators can configure
        <code>PUBLIC_TERMS_URL</code> without modifying this page.
      </p>
    {/if}
  </section>
</main>
