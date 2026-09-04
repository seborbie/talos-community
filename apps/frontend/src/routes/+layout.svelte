<script lang="ts">
  import '../app.css';
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import AlphaWatermark from '$lib/components/AlphaWatermark.svelte';
  import Toaster from '$lib/components/Toaster.svelte';
  import { isLightMode, toggleTheme, initTheme } from '$lib/theme';
  import { cursorEffectsEnabled, initCursorEffects, toggleCursorEffects } from '$lib/cursor-effects';
  import { MousePointer2, Sun, Moon } from 'lucide-svelte';

  $: onDashboard = $page.url.pathname.startsWith('/dashboard');
  $: if (applyCursorEffectMode) applyCursorEffectMode();

  type CursorEffectMode = 'off' | 'light';

  const GLOW_RADIUS = 110;
  const HIDDEN_GLOW_TRANSFORM = 'translate3d(-9999px, -9999px, 0)';

  let glowEl: HTMLDivElement | null = null;
  let applyCursorEffectMode: (() => void) | null = null;

  onMount(() => {
    initTheme();
    initCursorEffects();

    const root = document.documentElement;
    const mediaQueries = [
      window.matchMedia('(pointer: fine)'),
      window.matchMedia('(prefers-reduced-motion: reduce)')
    ] as const;
    let cursorEffectsOptIn = true;
    let effectsAllowedByEnvironment = false;
    let cursorEffectMode: CursorEffectMode = 'off';

    const applyEffectMode = () => {
      const [finePointer, reducedMotion] = mediaQueries;
      effectsAllowedByEnvironment = finePointer.matches && !reducedMotion.matches;
      cursorEffectMode = cursorEffectsOptIn && effectsAllowedByEnvironment ? 'light' : 'off';
      root.dataset.cursorEffects = cursorEffectMode;
    };
    applyCursorEffectMode = applyEffectMode;

    const effectsPreferenceUnsubscribe = cursorEffectsEnabled.subscribe((enabled) => {
      cursorEffectsOptIn = enabled;
      applyEffectMode();
    });

    applyEffectMode();

    let rafId: number | null = null;
    let pendingX = 0;
    let pendingY = 0;

    const flush = () => {
      rafId = null;
      if (cursorEffectMode === 'off' || !glowEl) return;
      glowEl.style.transform = `translate3d(${pendingX - GLOW_RADIUS}px, ${pendingY - GLOW_RADIUS}px, 0)`;
    };

    const onMouseMove = (e: MouseEvent) => {
      if (cursorEffectMode === 'off' || !glowEl) return;
      pendingX = e.clientX;
      pendingY = e.clientY;
      if (rafId === null) rafId = requestAnimationFrame(flush);
    };

    const onMouseDown = (e: MouseEvent) => {
      if (cursorEffectMode === 'off') return;

      const ripple = document.createElement('div');
      ripple.className = 'aero-click-ripple';
      ripple.style.left = e.clientX + 'px';
      ripple.style.top  = e.clientY + 'px';
      document.body.appendChild(ripple);
      ripple.addEventListener('animationend', () => ripple.remove(), { once: true });
    };

    const onMouseLeave = () => {
      if (glowEl) glowEl.style.transform = HIDDEN_GLOW_TRANSFORM;
    };
    const onMediaChange = () => {
      applyEffectMode();
    };

    document.addEventListener('mousemove',  onMouseMove,  { passive: true });
    document.addEventListener('mousedown',  onMouseDown,  { passive: true });
    document.documentElement.addEventListener('mouseleave', onMouseLeave);
    mediaQueries.forEach((query) => query.addEventListener('change', onMediaChange));

    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      applyCursorEffectMode = null;
      effectsPreferenceUnsubscribe();
      delete root.dataset.cursorEffects;
      document.removeEventListener('mousemove',  onMouseMove);
      document.removeEventListener('mousedown',  onMouseDown);
      document.documentElement.removeEventListener('mouseleave', onMouseLeave);
      mediaQueries.forEach((query) => query.removeEventListener('change', onMediaChange));
    };
  });
</script>

{#if $cursorEffectsEnabled}
  <!-- Cursor spotlight -->
  <div class="aero-cursor-glow" bind:this={glowEl} aria-hidden="true"></div>

  {#if !onDashboard}
    <!-- Ambient orbs -->
    <div class="aero-orbs-layer" aria-hidden="true">
      <div class="aero-orb aero-orb-1"></div>
      <div class="aero-orb aero-orb-2"></div>
      <div class="aero-orb aero-orb-3"></div>
      <div class="aero-orb aero-orb-4"></div>
    </div>
  {/if}
{/if}

<slot></slot>

<!-- Theme toggle — bottom left, only when NOT on dashboard (sidebar has its own) -->
{#if !onDashboard}
  <div class="aero-toggle-stack">
    <button class="aero-theme-toggle" on:click={toggleCursorEffects} aria-label="Toggle cursor effects">
      <MousePointer2 class="h-3.5 w-3.5" />
      <span class="aero-toggle-label">Cursor FX</span>
      <div class="aero-toggle-track">
        <div class="aero-toggle-thumb" class:is-active={$cursorEffectsEnabled}></div>
      </div>
    </button>
    <button class="aero-theme-toggle" on:click={toggleTheme} aria-label="Toggle light/dark mode">
      <Moon class="h-3.5 w-3.5" />
      <span class="aero-toggle-label">Theme</span>
      <div class="aero-toggle-track">
        <div class="aero-toggle-thumb" class:is-light={$isLightMode}></div>
      </div>
      <Sun class="h-3.5 w-3.5" />
    </button>
  </div>
{/if}

<AlphaWatermark />
<Toaster />
