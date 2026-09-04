<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import { authApi, authUtils } from '$lib/api';
  import { env } from '$env/dynamic/public';
  import { resolveOperatorContent } from '$lib/operatorContent';
  import { isLightMode } from '$lib/theme';
  import {
    Server,
    MessageSquare,
    Zap,
    Shield,
    BarChart3,
    CheckCircle,
    User,
    Activity
  } from 'lucide-svelte';

  let isAuthed  = false;
  let registrationOpen = false;
  let menuOpen  = false;
  let menuRef: HTMLDivElement | null = null;
  const operator = resolveOperatorContent(env);

  onMount(() => {
    isAuthed = authUtils.isAuthenticated();
    void authApi.getRegistrationStatus()
      .then((status) => { registrationOpen = status.registrationOpen; })
      .catch(() => { registrationOpen = false; });
    const onClickOutside = (event: MouseEvent) => {
      if (menuRef && !menuRef.contains(event.target as Node)) menuOpen = false;
    };
    document.addEventListener('mousedown', onClickOutside);
    return () => document.removeEventListener('mousedown', onClickOutside);
  });

  const handleLogout = () => {
    authUtils.removeToken();
    isAuthed = false;
    menuOpen = false;
    goto('/');
  };

  const features = [
    { icon: MessageSquare, title: 'Remote Access',       desc: 'Connect to any registered device for remote support and screen viewing.' },
    { icon: Zap,           title: 'Real-Time Monitoring',desc: 'Live device status, inventory, and command execution with minimal latency.' },
    { icon: Shield,        title: 'Security-Aware Design', desc: 'Policy, audit, and deployment controls designed for transparent self-hosted operation.' },
    { icon: BarChart3,     title: 'Command Policies',    desc: 'Control which commands run per organization, customer, or role with allow/deny policies.' },
    { icon: Server,        title: 'Device Fleet',        desc: 'Register agents on your machines and manage them from one place.' },
    { icon: CheckCircle,   title: 'Customer Scoping',    desc: 'Assign devices to customers and scope policies for multi-tenant use.' },
  ];

  const stats = [
    { icon: Server,   label: 'Online Devices',   value: '48' },
    { icon: Activity, label: 'Active Sessions',  value: '7'  },
    { icon: Shield,   label: 'Policy Rules',     value: '124'},
  ];
</script>

<div class="aero-page" class:light={$isLightMode}>
  <!-- ── Navigation ── -->
  <header class="aero-nav">
    <nav class="container mx-auto px-6 py-4 flex items-center justify-between">
      <div class="flex items-center gap-3">
        <div class="aero-logo-icon">
          <Server class="h-5 w-5 text-white" />
        </div>
        <span class="aero-wordmark">Talos</span>
      </div>

      {#if !isAuthed}
        <div class="flex items-center gap-3">
          <a href="/login" class="aero-btn-ghost">Login</a>
          {#if registrationOpen}
            <a href="/register" class="aero-btn-nav">Set Up Talos</a>
          {/if}
        </div>
      {:else}
        <div class="relative" bind:this={menuRef}>
          <button
            type="button"
            on:click={() => (menuOpen = !menuOpen)}
            class="aero-btn-ghost flex items-center gap-2"
            aria-haspopup="menu"
            aria-expanded={menuOpen}
          >
            <div class="w-6 h-6 rounded-full bg-white/15 border border-white/20 flex items-center justify-center">
              <User class="h-3.5 w-3.5 text-white" />
            </div>
            <span>Profile</span>
          </button>

          {#if menuOpen}
            <div role="menu" class="aero-dropdown absolute right-0 mt-2 w-44">
              <button
                type="button"
                class="aero-dropdown-item"
                on:click={() => { menuOpen = false; goto('/dashboard'); }}
                role="menuitem"
              >Dashboard</button>
              <div class="mx-3 h-px bg-white/10"></div>
              <button
                type="button"
                class="aero-dropdown-item aero-dropdown-danger"
                on:click={handleLogout}
                role="menuitem"
              >Log Out</button>
            </div>
          {/if}
        </div>
      {/if}
    </nav>
  </header>

  <!-- ── Hero ── -->
  <section class="hero-section">
    <div class="container mx-auto px-6 flex flex-col items-center text-center">

      <div class="aero-badge mb-7">
        <Zap class="h-3.5 w-3.5" />
        <span>Open-Source RMM · Alpha</span>
      </div>

      <h1 class="aero-hero-title">
        Remote Monitoring &amp;<br />Management for Your Fleet
      </h1>

      <p class="aero-hero-sub">
        Monitor and manage your devices remotely. Run commands, view screens,<br />
        and enforce policies across your organization with a single glass dashboard.
      </p>

      <div class="flex flex-wrap justify-center gap-4 mt-10">
        {#if registrationOpen}
          <a href="/register" class="aero-btn-cta">Set Up This Deployment</a>
        {/if}
        <a href="/login"    class="aero-btn-secondary">Sign In</a>
      </div>

      <!-- Mock dashboard glass panel -->
      <div class="aero-preview mt-20 w-full max-w-2xl">
        <div class="grid grid-cols-3 gap-3 p-4">
          {#each stats as s}
            <div class="aero-stat-card">
              <svelte:component this={s.icon} class="h-5 w-5 text-sky-300 mb-2 opacity-80" />
              <div class="stat-value">{s.value}</div>
              <div class="stat-label">{s.label}</div>
            </div>
          {/each}
        </div>
        <div class="px-4 pb-4 space-y-2">
          {#each ['WIN-DESK-04 · Online · Windows 11', 'SRV-PROD-01 · Online · Ubuntu 22.04', 'MAC-DEV-12 · Idle · macOS 14'] as row}
            <div class="aero-row">{row}</div>
          {/each}
        </div>
      </div>
    </div>
  </section>

  <!-- ── Features ── -->
  <section class="container mx-auto px-6 py-28">
    <div class="text-center mb-16">
      <h2 class="aero-section-title">Core tools for managing a device fleet</h2>
      <p class="aero-feature-sub">
        An evolving foundation for remote monitoring, command execution, and device policy.
      </p>
    </div>

    <div class="grid md:grid-cols-3 gap-5">
      {#each features as f}
        <div class="aero-card">
          <div class="aero-card-icon">
            <svelte:component this={f.icon} class="h-5 w-5 text-sky-300" />
          </div>
          <h3 class="text-white font-semibold text-base mb-2">{f.title}</h3>
          <p class="aero-card-desc text-sm leading-relaxed">{f.desc}</p>
        </div>
      {/each}
    </div>
  </section>

  <!-- ── Footer ── -->
  <footer class="aero-footer">
    <div class="container mx-auto px-6 py-8 flex flex-col items-center gap-4">
      <div class="flex items-center gap-2">
        <Server class="h-4 w-4 text-sky-400/70" />
        <span class="font-semibold text-white/60 text-sm">Talos</span>
      </div>
      <div class="flex gap-6 text-sm">
        <a href="/privacy" class="aero-footer-link">Operator privacy</a>
        <a href="/terms"   class="aero-footer-link">Operator terms</a>
        <a href="/contact" class="aero-footer-link">Deployment support</a>
        {#if operator.sourceUrl}
          <a href={operator.sourceUrl} class="aero-footer-link" target="_blank" rel="noopener noreferrer">Source code</a>
        {/if}
      </div>
      <p class="text-white/20 text-xs">Talos Community Edition</p>
    </div>
  </footer>
</div>

<style>
  /* ════════════════════════════════════════════════
     AERO / LIQUID GLASS  — landing page styles
  ════════════════════════════════════════════════ */

  /* ── Base canvas ── */
  .aero-page {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    position: relative;
    color: white;
  }

  /* ── Navigation bar ── */
  .aero-nav {
    position: sticky;
    top: 0;
    z-index: 50;
    background: rgba(6, 14, 40, 0.55);
    backdrop-filter: blur(28px) saturate(180%);
    -webkit-backdrop-filter: blur(28px) saturate(180%);
    border-bottom: 1px solid rgba(255, 255, 255, 0.07);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06), 0 4px 32px rgba(0, 0, 0, 0.45);
  }

  .aero-logo-icon {
    width: 36px; height: 36px;
    border-radius: 10px;
    background: linear-gradient(145deg, rgba(70, 150, 255, 0.85), rgba(20, 80, 210, 0.9));
    border: 1px solid rgba(120, 190, 255, 0.3);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.35), 0 0 16px rgba(50, 130, 255, 0.4);
    display: flex; align-items: center; justify-content: center;
  }

  .aero-wordmark {
    font-size: 1.2rem;
    font-weight: 700;
    letter-spacing: -0.02em;
    background: linear-gradient(180deg, #ffffff 0%, rgba(160, 210, 255, 0.8) 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }

  .aero-btn-ghost {
    padding: 0.4rem 0.9rem;
    border-radius: 8px;
    font-size: 0.875rem;
    font-weight: 500;
    color: rgba(200, 225, 255, 0.7);
    text-decoration: none;
    display: inline-flex; align-items: center;
    border: 1px solid transparent;
    background: transparent;
    transition: background 0.18s, border-color 0.18s, color 0.18s;
  }
  .aero-btn-ghost:hover {
    background: rgba(255, 255, 255, 0.07);
    border-color: rgba(255, 255, 255, 0.11);
    color: rgba(220, 240, 255, 0.95);
  }

  .aero-btn-nav {
    position: relative;
    overflow: hidden;
    padding: 0.4rem 1.1rem;
    border-radius: 8px;
    font-size: 0.875rem;
    font-weight: 600;
    color: rgba(225, 240, 255, 0.9);
    text-decoration: none;
    display: inline-flex; align-items: center;
    background: linear-gradient(180deg, rgba(255,255,255,0.13) 0%, rgba(255,255,255,0.06) 100%);
    border-top:    1px solid rgba(255, 255, 255, 0.35);
    border-left:   1px solid rgba(255, 255, 255, 0.2);
    border-right:  1px solid rgba(255, 255, 255, 0.1);
    border-bottom: 1px solid rgba(0, 0, 0, 0.3);
    backdrop-filter: blur(24px) saturate(200%) brightness(1.05);
    -webkit-backdrop-filter: blur(24px) saturate(200%) brightness(1.05);
    box-shadow:
      0 3px 0 rgba(18, 48, 160, 0.55),
      0 5px 0 rgba(8, 24, 80, 0.4),
      0 7px 16px rgba(0, 0, 0, 0.45),
      0 0 20px rgba(80, 160, 255, 0.16);
    transition: transform 0.12s ease, box-shadow 0.12s ease, background 0.18s ease;
  }
  .aero-btn-nav:hover {
    background: linear-gradient(180deg, rgba(255,255,255,0.18) 0%, rgba(255,255,255,0.09) 100%);
    transform: translateY(-1px);
    box-shadow:
      0 4px 0 rgba(18, 48, 160, 0.62),
      0 6px 0 rgba(8, 24, 80, 0.46),
      0 9px 22px rgba(0, 0, 0, 0.5),
      0 0 28px rgba(80, 160, 255, 0.22);
  }
  .aero-btn-nav:active {
    transform: translateY(3px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.35);
  }

  /* ── Hero section ── */
  .hero-section {
    flex: 1;
    padding: 7rem 0 5rem;
    display: flex;
    align-items: center;
  }

  .aero-badge {
    display: inline-flex; align-items: center; gap: 6px;
    padding: 5px 14px;
    border-radius: 999px;
    background: rgba(60, 140, 255, 0.1);
    border: 1px solid rgba(80, 160, 255, 0.28);
    font-size: 0.78rem;
    font-weight: 500;
    color: rgba(130, 200, 255, 0.9);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.09), 0 0 16px rgba(60, 140, 255, 0.12);
    backdrop-filter: blur(8px);
  }

  .aero-hero-title {
    font-size: clamp(2.6rem, 5.5vw, 4.2rem);
    font-weight: 800;
    line-height: 1.15;
    letter-spacing: -0.03em;
    background: linear-gradient(180deg, #ffffff 0%, rgba(170, 215, 255, 0.88) 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
    margin-bottom: 1.5rem;
    padding: 0.08em 0.12em 0.18em;
  }

  .aero-hero-sub {
    font-size: 1.05rem;
    line-height: 1.75;
    color: rgba(160, 205, 255, 0.55);
  }

  /* ── CTA buttons ── */
  .aero-btn-cta,
  .aero-btn-secondary {
    position: relative;
    overflow: hidden;
    padding: 0.7rem 2rem;
    border-radius: 10px;
    font-size: 0.975rem;
    font-weight: 600;
    text-decoration: none;
    display: inline-flex; align-items: center;
    backdrop-filter: blur(28px) saturate(200%) brightness(1.05);
    -webkit-backdrop-filter: blur(28px) saturate(200%) brightness(1.05);
    transition: transform 0.12s ease, box-shadow 0.12s ease, background 0.18s ease;
  }

  .aero-btn-cta {
    color: rgba(230, 245, 255, 0.95);
    background: linear-gradient(180deg, rgba(255,255,255,0.14) 0%, rgba(255,255,255,0.06) 100%);
    border-top:    1px solid rgba(255, 255, 255, 0.38);
    border-left:   1px solid rgba(255, 255, 255, 0.22);
    border-right:  1px solid rgba(255, 255, 255, 0.12);
    border-bottom: 1px solid rgba(0, 0, 0, 0.32);
    box-shadow:
      0 3px 0 rgba(18, 48, 160, 0.6),
      0 5px 0 rgba(8, 24, 80, 0.45),
      0 8px 22px rgba(0, 0, 0, 0.55),
      0 0 28px rgba(80, 160, 255, 0.2);
  }
  .aero-btn-cta:hover {
    background: linear-gradient(180deg, rgba(255,255,255,0.19) 0%, rgba(255,255,255,0.09) 100%);
    border-top-color: rgba(255, 255, 255, 0.46);
    box-shadow:
      0 4px 0 rgba(18, 48, 160, 0.65),
      0 7px 0 rgba(8, 24, 80, 0.5),
      0 10px 28px rgba(0, 0, 0, 0.6),
      0 0 36px rgba(80, 160, 255, 0.28);
    transform: translateY(-1px);
  }
  .aero-btn-cta:active {
    transform: translateY(4px);
    background: linear-gradient(180deg, rgba(255,255,255,0.07) 0%, rgba(255,255,255,0.12) 100%);
    box-shadow: 0 2px 10px rgba(0, 0, 0, 0.4);
  }

  .aero-btn-secondary {
    color: rgba(200, 225, 255, 0.75);
    background: linear-gradient(180deg, rgba(255,255,255,0.10) 0%, rgba(255,255,255,0.04) 100%);
    border-top:    1px solid rgba(255, 255, 255, 0.24);
    border-left:   1px solid rgba(255, 255, 255, 0.14);
    border-right:  1px solid rgba(255, 255, 255, 0.08);
    border-bottom: 1px solid rgba(0, 0, 0, 0.28);
    box-shadow:
      0 3px 0 rgba(4, 12, 50, 0.55),
      0 5px 0 rgba(2, 6, 28, 0.42),
      0 8px 20px rgba(0, 0, 0, 0.45);
  }
  .aero-btn-secondary:hover {
    background: linear-gradient(180deg, rgba(255,255,255,0.14) 0%, rgba(255,255,255,0.07) 100%);
    border-top-color: rgba(255, 255, 255, 0.32);
    color: rgba(220, 240, 255, 0.9);
    transform: translateY(-1px);
    box-shadow:
      0 4px 0 rgba(4, 12, 50, 0.6),
      0 7px 0 rgba(2, 6, 28, 0.46),
      0 10px 24px rgba(0, 0, 0, 0.5);
  }
  .aero-btn-secondary:active {
    transform: translateY(4px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.35);
  }

  /* ── Mock dashboard preview ── */
  .aero-preview {
    background: rgba(8, 18, 52, 0.55);
    backdrop-filter: blur(28px) saturate(160%);
    -webkit-backdrop-filter: blur(28px) saturate(160%);
    border: 1px solid rgba(70, 140, 255, 0.18);
    border-radius: 16px;
    overflow: hidden;
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.07),
      0 0 0 1px rgba(30, 80, 200, 0.15),
      0 40px 100px rgba(0, 10, 60, 0.7),
      0 10px 40px rgba(0, 0, 0, 0.55);
  }
  .aero-stat-card {
    background: rgba(255, 255, 255, 0.04);
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 10px;
    padding: 14px;
    text-align: left;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06);
  }
  .stat-value {
    font-size: 1.5rem;
    font-weight: 700;
    color: rgba(220, 240, 255, 0.95);
    font-variant-numeric: tabular-nums;
  }
  .stat-label {
    font-size: 0.75rem;
    margin-top: 2px;
    color: rgba(160, 205, 255, 0.45);
  }
  .aero-row {
    height: 32px;
    border-radius: 6px;
    background: rgba(255, 255, 255, 0.03);
    border: 1px solid rgba(255, 255, 255, 0.06);
    display: flex; align-items: center;
    padding: 0 12px;
    font-size: 0.73rem;
    color: rgba(180, 210, 255, 0.4);
    font-family: 'Consolas', 'SF Mono', monospace;
  }

  /* ── Section heading ── */
  .aero-section-title {
    font-size: clamp(1.8rem, 3vw, 2.4rem);
    font-weight: 700;
    letter-spacing: -0.025em;
    background: linear-gradient(180deg, #ffffff 0%, rgba(170, 215, 255, 0.82) 100%);
    -webkit-background-clip: text;
    -webkit-text-fill-color: transparent;
    background-clip: text;
  }
  .aero-feature-sub {
    color: rgba(160, 200, 255, 0.45);
    font-size: 1.05rem;
    margin-top: 1rem;
    max-width: 36rem;
    margin-left: auto;
    margin-right: auto;
  }

  /* ── Feature cards ── */
  .aero-card {
    position: relative;
    background: rgba(255, 255, 255, 0.042);
    backdrop-filter: blur(18px) saturate(140%);
    -webkit-backdrop-filter: blur(18px) saturate(140%);
    border: 1px solid rgba(255, 255, 255, 0.09);
    border-radius: 14px;
    padding: 26px 22px;
    overflow: hidden;
    transition:
      transform    0.22s cubic-bezier(0.34, 1.4, 0.64, 1),
      box-shadow   0.28s cubic-bezier(0.22, 1, 0.36, 1),
      border-color 0.3s ease,
      background   0.3s ease;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.1), 0 4px 28px rgba(0, 0, 0, 0.32);
  }
  .aero-card::before {
    content: '';
    position: absolute;
    top: 0; left: 10%; right: 10%;
    height: 1px;
    background: linear-gradient(90deg, transparent, rgba(80, 160, 255, 0.55), transparent);
    pointer-events: none;
  }
  .aero-card:hover {
    background: rgba(255, 255, 255, 0.068);
    border-color: rgba(80, 160, 255, 0.22);
    transform: translateY(-6px);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.14),
      0 20px 56px rgba(0, 0, 0, 0.38),
      0 8px 24px rgba(0, 0, 0, 0.22),
      0 0 0 1px rgba(80, 160, 255, 0.12);
  }
  .aero-card-icon {
    width: 44px; height: 44px;
    border-radius: 11px;
    background: rgba(60, 140, 255, 0.1);
    border: 1px solid rgba(80, 160, 255, 0.2);
    display: flex; align-items: center; justify-content: center;
    margin-bottom: 14px;
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.09), 0 0 14px rgba(60, 140, 255, 0.09);
  }
  .aero-card-desc { color: rgba(160, 200, 255, 0.42); }

  /* ── Dropdown ── */
  .aero-dropdown {
    background: rgba(8, 18, 55, 0.88);
    backdrop-filter: blur(28px) saturate(160%);
    -webkit-backdrop-filter: blur(28px) saturate(160%);
    border: 1px solid rgba(255, 255, 255, 0.11);
    border-radius: 10px;
    padding: 5px 0;
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.07), 0 20px 50px rgba(0, 0, 0, 0.55);
    overflow: hidden;
  }
  .aero-dropdown-item {
    display: block;
    width: 100%;
    text-align: left;
    padding: 8px 16px;
    font-size: 0.875rem;
    color: rgba(200, 225, 255, 0.8);
    background: transparent;
    border: none;
    transition: background 0.14s, color 0.14s;
  }
  .aero-dropdown-item:hover { background: rgba(255, 255, 255, 0.08); color: white; }
  .aero-dropdown-danger { color: rgba(255, 140, 140, 0.8); }
  .aero-dropdown-danger:hover { color: rgba(255, 160, 160, 1); }

  /* ── Footer ── */
  .aero-footer {
    margin-top: auto;
    background: rgba(4, 10, 30, 0.65);
    backdrop-filter: blur(14px);
    -webkit-backdrop-filter: blur(14px);
    border-top: 1px solid rgba(255, 255, 255, 0.055);
  }
  .aero-footer-link {
    color: rgba(255,255,255,0.3);
    text-decoration: none;
    transition: color 0.2s;
  }
  .aero-footer-link:hover { color: rgba(255,255,255,0.72); }

  /* ════════════════════════════════════════════════
     LIGHT MODE — page-local overrides
  ════════════════════════════════════════════════ */
  .aero-page,
  .aero-nav,
  .aero-card,
  .aero-preview,
  .aero-footer,
  .aero-dropdown,
  .aero-badge {
    transition:
      background     0.45s ease,
      background-color 0.45s ease,
      border-color   0.35s ease,
      box-shadow     0.35s ease,
      color          0.35s ease;
  }

  /* Page text */
  .aero-page.light { color: #0a1628; }

  /* Nav */
  .aero-page.light .aero-nav {
    background: rgba(255, 255, 255, 0.58);
    border-bottom-color: rgba(100, 160, 220, 0.22);
    box-shadow: inset 0 -1px 0 rgba(100, 160, 220, 0.08), 0 4px 32px rgba(0, 40, 100, 0.1);
  }
  .aero-page.light .aero-wordmark {
    background: linear-gradient(180deg, #081428 0%, #1a4888 100%);
    -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
  }
  .aero-page.light .aero-logo-icon {
    background: linear-gradient(145deg, rgba(55, 135, 255, 0.88), rgba(20, 82, 220, 0.92));
    border-color: rgba(90, 155, 255, 0.4);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.5), 0 0 14px rgba(55,135,255,0.3);
  }
  .aero-page.light .aero-btn-ghost { color: rgba(10, 30, 90, 0.68); }
  .aero-page.light .aero-btn-ghost:hover {
    background: rgba(50, 100, 200, 0.08); border-color: rgba(50, 100, 200, 0.16); color: #0a1628;
  }
  .aero-page.light .aero-btn-nav {
    color: #081428;
    background: linear-gradient(180deg, rgba(255,255,255,0.88) 0%, rgba(210,232,255,0.72) 100%);
    border-top: 1px solid rgba(255,255,255,0.92);
    border-left: 1px solid rgba(200,222,255,0.7);
    border-right: 1px solid rgba(155,192,242,0.5);
    border-bottom: 1px solid rgba(80,125,205,0.38);
    box-shadow: 0 3px 0 rgba(75,130,218,0.38), 0 5px 0 rgba(45,88,182,0.22), 0 8px 18px rgba(0,38,120,0.16);
  }
  .aero-page.light .aero-btn-nav:hover {
    background: linear-gradient(180deg, rgba(255,255,255,0.98) 0%, rgba(222,240,255,0.82) 100%);
  }

  /* Badge */
  .aero-page.light .aero-badge {
    background: rgba(50,120,255,0.07); border-color: rgba(50,120,255,0.26); color: rgba(18,68,200,0.88);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.5);
  }

  /* Hero */
  .aero-page.light .aero-hero-title {
    background: linear-gradient(180deg, #081428 0%, #1a4888 100%);
    -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
  }
  .aero-page.light .aero-hero-sub { color: rgba(12, 42, 108, 0.58); }

  /* CTA buttons */
  .aero-page.light .aero-btn-cta {
    color: #081428;
    background: linear-gradient(180deg, rgba(255,255,255,0.9) 0%, rgba(208,230,255,0.74) 100%);
    border-top: 1px solid rgba(255,255,255,0.95);
    border-left: 1px solid rgba(200,224,255,0.75);
    border-right: 1px solid rgba(152,192,246,0.56);
    border-bottom: 1px solid rgba(68,122,212,0.4);
    box-shadow:
      0 3px 0 rgba(68,122,212,0.42), 0 5px 0 rgba(38,80,180,0.26),
      0 8px 22px rgba(0,28,100,0.18), 0 0 28px rgba(60,140,255,0.1);
  }
  .aero-page.light .aero-btn-cta:hover {
    background: linear-gradient(180deg, white 0%, rgba(218,238,255,0.88) 100%);
    box-shadow: 0 4px 0 rgba(68,122,212,0.48), 0 7px 0 rgba(38,80,180,0.3), 0 10px 28px rgba(0,28,100,0.22);
  }
  .aero-page.light .aero-btn-secondary {
    color: rgba(10,30,90,0.75);
    background: linear-gradient(180deg, rgba(255,255,255,0.72) 0%, rgba(222,238,255,0.52) 100%);
    border-top: 1px solid rgba(255,255,255,0.88);
    border-left: 1px solid rgba(200,222,255,0.58);
    border-right: 1px solid rgba(158,192,242,0.4);
    border-bottom: 1px solid rgba(78,132,212,0.3);
    box-shadow: 0 3px 0 rgba(50,102,192,0.3), 0 5px 0 rgba(28,66,162,0.18), 0 7px 18px rgba(0,28,100,0.12);
  }
  .aero-page.light .aero-btn-secondary:hover {
    color: #0a1628;
    background: linear-gradient(180deg, rgba(255,255,255,0.9) 0%, rgba(230,244,255,0.7) 100%);
  }

  /* Dropdown */
  .aero-page.light .aero-dropdown {
    background: rgba(245,251,255,0.94); border-color: rgba(100,158,218,0.28);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.9), 0 18px 44px rgba(0,40,120,0.14);
  }
  .aero-page.light .aero-dropdown-item { color: rgba(10,30,90,0.8); }
  .aero-page.light .aero-dropdown-item:hover { background: rgba(50,100,200,0.07); color: #0a1628; }
  .aero-page.light .aero-dropdown-danger { color: rgba(195,45,45,0.85); }
  .aero-page.light .aero-dropdown-danger:hover { color: rgba(195,45,45,1); }

  /* Preview panel */
  .aero-page.light .aero-preview {
    background: rgba(255,255,255,0.18);
    backdrop-filter: blur(32px) saturate(180%) brightness(1.06);
    -webkit-backdrop-filter: blur(32px) saturate(180%) brightness(1.06);
    border-color: rgba(255,255,255,0.55);
    box-shadow:
      inset 0 1px 0 rgba(255,255,255,0.7),
      inset 0 0 0 1px rgba(255,255,255,0.25),
      0 8px 40px rgba(0,38,120,0.1);
  }
  .aero-page.light .aero-stat-card {
    background: rgba(255,255,255,0.28);
    backdrop-filter: blur(12px);
    -webkit-backdrop-filter: blur(12px);
    border-color: rgba(255,255,255,0.5);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.65);
    color: #0a1628;
  }
  .aero-page.light .stat-value { color: #0a1628; }
  .aero-page.light .stat-label { color: rgba(10, 42, 108, 0.52); }
  .aero-page.light .aero-row {
    background: rgba(255,255,255,0.2); border-color: rgba(255,255,255,0.42); color: rgba(10,48,130,0.48);
  }

  /* Features */
  .aero-page.light .aero-section-title {
    background: linear-gradient(180deg, #081428 0%, #1a4888 100%);
    -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
  }
  .aero-page.light .aero-feature-sub { color: rgba(12,42,108,0.52); }

  /* Feature cards */
  .aero-page.light .aero-card {
    background: rgba(255,255,255,0.14);
    backdrop-filter: blur(28px) saturate(180%) brightness(1.04);
    -webkit-backdrop-filter: blur(28px) saturate(180%) brightness(1.04);
    border: 1px solid rgba(255,255,255,0.55);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.72), 0 4px 24px rgba(0,38,120,0.07);
  }
  .aero-page.light .aero-card::before {
    background: linear-gradient(90deg, transparent, rgba(140,200,255,0.45), rgba(200,220,255,0.3), transparent);
  }
  .aero-page.light .aero-card:hover {
    background: rgba(255,255,255,0.22); border-color: rgba(255,255,255,0.72); transform: translateY(-6px);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.82), 0 20px 56px rgba(0,38,120,0.12), 0 8px 24px rgba(0,38,120,0.07);
  }
  .aero-page.light .aero-card h3 { color: #0a1628; }
  .aero-page.light .aero-card-desc { color: rgba(12,42,108,0.56); }
  .aero-page.light .aero-card-icon {
    background: rgba(255,255,255,0.32); border-color: rgba(255,255,255,0.55);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.8);
  }

  /* Footer */
  .aero-page.light .aero-footer {
    background: rgba(210,234,255,0.55); border-top-color: rgba(100,158,220,0.2);
  }
  .aero-page.light .aero-footer-link { color: rgba(10,38,100,0.45); }
  .aero-page.light .aero-footer-link:hover { color: rgba(10,38,100,0.85); }
  .aero-page.light .aero-footer span { color: rgba(10,38,100,0.55); }
  .aero-page.light .aero-footer p { color: rgba(10,38,100,0.3); }
</style>
