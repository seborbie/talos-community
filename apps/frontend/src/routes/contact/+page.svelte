<script lang="ts">
  import { env } from '$env/dynamic/public';
  import { ExternalLink, LifeBuoy, MessageSquare, ShieldAlert } from 'lucide-svelte';
  import { resolveOperatorContent } from '$lib/operatorContent';
  import Card from '$lib/ui/Card.svelte';
  import CardContent from '$lib/ui/CardContent.svelte';
  import CardDescription from '$lib/ui/CardDescription.svelte';
  import CardHeader from '$lib/ui/CardHeader.svelte';
  import CardTitle from '$lib/ui/CardTitle.svelte';

  const operator = resolveOperatorContent(env);
  const operatorName = operator.name ?? 'this Talos deployment';
</script>

<svelte:head>
  <title>Deployment support | Talos</title>
</svelte:head>

<main class="mx-auto max-w-3xl space-y-8 px-4 py-12">
  <header class="space-y-4 text-center">
    <div class="flex justify-center">
      <MessageSquare class="h-14 w-14 text-sky-300" />
    </div>
    <h1 class="text-4xl font-bold aero-gradient-text">Deployment support</h1>
    <p class="mx-auto max-w-2xl text-white/55">
      Community Edition is self-hosted. Support, response times, billing, and service commitments
      come from the organization operating this deployment—not from the source repository.
    </p>
  </header>

  <Card>
    <CardHeader>
      <CardTitle className="flex items-center gap-2">
        <LifeBuoy class="h-5 w-5" />
        Contact {operatorName}
      </CardTitle>
      <CardDescription>
        The deployment operator controls the support destination shown here.
      </CardDescription>
    </CardHeader>
    <CardContent className="space-y-5">
      {#if operator.supportUrl}
        <p class="text-sm text-white/60">
          Use the operator's configured support site for account, deployment, or service questions.
        </p>
        <a
          class="inline-flex items-center gap-2 rounded-md bg-sky-500/15 px-4 py-2 text-sm font-medium text-sky-200 hover:bg-sky-500/25"
          href={operator.supportUrl}
          target="_blank"
          rel="noopener noreferrer"
        >
          Open operator support
          <ExternalLink class="h-4 w-4" />
        </a>
      {:else}
        <div class="flex gap-3 rounded-lg border border-amber-300/25 bg-amber-300/8 p-4">
          <ShieldAlert class="mt-0.5 h-5 w-5 shrink-0 text-amber-200" />
          <div class="space-y-1">
            <p class="font-medium text-amber-100">No support destination is configured</p>
            <p class="text-sm text-amber-100/70">
              Contact the administrator who supplied this deployment. Operators can set
              <code>PUBLIC_SUPPORT_URL</code> to publish a support destination without changing the application
              source.
            </p>
          </div>
        </div>
      {/if}
    </CardContent>
  </Card>

  <p class="text-center text-xs text-white/40">
    This page does not collect, transmit, or log contact-form data.
  </p>
</main>
