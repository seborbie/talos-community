<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/stores';
  import { goto } from '$app/navigation';
  import { browser } from '$app/environment';
  import { authUtils, orgsApi } from '$lib/api';
  import Button from '$lib/ui/Button.svelte';
  import { topbarConfig } from '$lib/topbar';
  import {
    Server, Building, User, Settings, Shield, Layers, FileText, Disc3, Bot,
    Download, BellRing, LogOut, Menu, X, ChevronDown, Sun, Moon, MousePointer2,
    ClipboardList, ShieldCheck, RefreshCw
  } from 'lucide-svelte';
  import { isLightMode, toggleTheme } from '$lib/theme';
  import { cursorEffectsEnabled, toggleCursorEffects } from '$lib/cursor-effects';

  type NavItem = { name: string; href: string; icon: typeof Server; children?: NavItem[]; };

  const navigationItems: NavItem[] = [
    { name: 'Command Center',    href: '/dashboard',                  icon: Bot },
    { name: 'Devices',           href: '/dashboard/devices',          icon: Server },
    { name: 'Alerts',            href: '/dashboard/rmm/alerts',       icon: BellRing },
    { name: 'Customers',         href: '/dashboard/rmm/customers',    icon: Building },
    { name: 'Baselines',         href: '/dashboard/rmm/baselines',    icon: Layers },
    { name: 'Reports',           href: '/dashboard/rmm/reports',      icon: FileText },
    {
      name: 'Patches', href: '/dashboard/rmm/patches', icon: ShieldCheck,
      children: [
        { name: 'Feature Upgrade Center', href: '/dashboard/rmm/patches/feature-upgrades', icon: Disc3 }
      ]
    },
    { name: 'Installers',        href: '/dashboard/rmm/installers',   icon: Download },
    { name: 'Audit Log',         href: '/dashboard/audit',            icon: ClipboardList },
    { name: 'Command Reference', href: '/dashboard/rmm/policies',     icon: Shield },
    { name: 'Profile',           href: '/dashboard/profile',          icon: User },
    {
      name: 'Organization Config', href: '/dashboard/org/users', icon: Settings,
      children: [
        { name: 'Members',          href: '/dashboard/org/users',    icon: User },
        { name: 'Command Policies', href: '/dashboard/org/policies', icon: Shield }
      ]
    }
  ];

  let sidebarOpen      = false;
  let expandedSections = new Set<string>();
  let currentPath      = '';

  onMount(async () => {
    if (!authUtils.isAuthenticated()) { goto('/login'); return; }
    try {
      const current = await orgsApi.getCurrent();
      if ('needsOnboarding' in current && current.needsOnboarding) goto('/dashboard/onboarding');
    } catch {}
  });

  $: if (browser) {
    document.body.style.overflow = sidebarOpen ? 'hidden' : '';
  }

  const handleLogout = () => { authUtils.removeToken(); goto('/'); };

  const normalizePath = (path: string) =>
    path.length > 1 && path.endsWith('/') ? path.slice(0, -1) : path;

  $: currentPath = normalizePath($page.url.pathname);

  const isActiveRoute = (href: string, path: string, opts: { exact?: boolean } = {}) => {
    const h = normalizePath(href), p = normalizePath(path);
    return opts.exact || href === '/dashboard' ? p === h : p === h || p.startsWith(`${h}/`);
  };

  const isActiveSection = (item: NavItem, path: string) =>
    isActiveRoute(item.href, path) || item.children?.some((c) => isActiveRoute(c.href, path)) || false;

  const toggleSection = (name: string) => {
    const next = new Set(expandedSections);
    next.has(name) ? next.delete(name) : next.add(name);
    expandedSections = next;
  };

  $: if (browser) {
    const next = new Set(expandedSections);
    for (const item of navigationItems) {
      if (item.children && isActiveSection(item, currentPath)) next.add(item.name);
    }
    expandedSections = next;
  }
</script>

<div class="dash-shell flex h-screen">
  <!-- ── Desktop sidebar ── -->
  <aside class="dash-sidebar hidden lg:flex flex-col w-64 flex-shrink-0">
    <div class="dash-sidebar-header">
      <div class="dash-wordmark-row">
        <div class="dash-logo-icon"><Server class="h-5 w-5 text-white" /></div>
        <span class="dash-wordmark">Talos</span>
      </div>
    </div>

    <nav class="flex-1 overflow-y-auto py-4 px-3 space-y-1 relative">
      {#each navigationItems as item}
        {#if item.children}
          <div class="space-y-0.5">
            {#if item.name === 'Patches'}
              <div class="nav-parent-row">
                <a href={item.href} class="nav-item nav-parent-link" class:nav-active={isActiveRoute(item.href, currentPath, { exact: true })}>
                  <svelte:component this={item.icon} class="mr-3 h-4 w-4 flex-shrink-0" />
                  <span class="flex-1 text-left">{item.name}</span>
                </a>
                <button type="button" on:click={() => toggleSection(item.name)} class="nav-item nav-expander" aria-label="Toggle Patches submenu">
                  <ChevronDown class="h-3.5 w-3.5 opacity-60 transition-transform {expandedSections.has(item.name) ? 'rotate-180' : ''}" />
                </button>
              </div>
            {:else}
              <button type="button" on:click={() => toggleSection(item.name)}
                class="nav-item w-full" class:nav-active={isActiveSection(item, currentPath)}>
                <svelte:component this={item.icon} class="mr-3 h-4 w-4 flex-shrink-0" />
                <span class="flex-1 text-left">{item.name}</span>
                <ChevronDown class="h-3.5 w-3.5 opacity-60 transition-transform {expandedSections.has(item.name) ? 'rotate-180' : ''}" />
              </button>
            {/if}
            {#if expandedSections.has(item.name)}
              <div class="ml-4 pl-3 border-l border-white/10 space-y-0.5 mt-0.5">
                {#each item.children as child}
                  <a href={child.href} class="nav-item nav-child" class:nav-active={isActiveRoute(child.href, currentPath, { exact: true })}>
                    <svelte:component this={child.icon} class="mr-3 h-3.5 w-3.5 flex-shrink-0" />
                    {child.name}
                  </a>
                {/each}
              </div>
            {/if}
          </div>
        {:else}
          <a href={item.href} class="nav-item" class:nav-active={isActiveRoute(item.href, currentPath)}>
            <svelte:component this={item.icon} class="mr-3 h-4 w-4 flex-shrink-0" />
            {item.name}
          </a>
        {/if}
      {/each}
    </nav>

    <div class="dash-sidebar-footer">
      <button type="button" on:click={toggleCursorEffects} class="nav-item w-full theme-toggle-nav" aria-label="Toggle cursor effects">
        <MousePointer2 class="mr-3 h-4 w-4 flex-shrink-0" />
        <span class="flex-1 text-left">Cursor Effects</span>
        <div class="aero-toggle-track">
          <div class="aero-toggle-thumb" class:is-active={$cursorEffectsEnabled}></div>
        </div>
      </button>
      <button type="button" on:click={toggleTheme} class="nav-item w-full theme-toggle-nav" aria-label="Toggle light/dark mode">
        <Moon class="mr-3 h-4 w-4 flex-shrink-0" />
        <span class="flex-1 text-left">Theme</span>
        <div class="aero-toggle-track">
          <div class="aero-toggle-thumb" class:is-light={$isLightMode}></div>
        </div>
        <Sun class="ml-2 h-3.5 w-3.5 flex-shrink-0 opacity-60" />
      </button>
      <button type="button" on:click={handleLogout} class="nav-item w-full logout-btn">
        <LogOut class="mr-3 h-4 w-4 flex-shrink-0" />
        Sign Out
      </button>
    </div>
  </aside>

  <!-- ── Main content ── -->
  <div class="flex-1 flex flex-col overflow-hidden min-w-0">
    <!-- Top bar -->
    <header class="dash-topbar">
      <div class="flex items-center justify-between px-5 h-full">
        <div class="flex items-center gap-3">
          <button type="button" on:click={() => (sidebarOpen = true)} class="lg:hidden p-1.5 rounded-lg hover:bg-white/8 transition-colors" aria-label="Open menu">
            <Menu class="h-5 w-5 text-white/70" />
          </button>
          {#if $topbarConfig}
            <h1 class="topbar-title">{$topbarConfig.title}</h1>
          {/if}
        </div>
        <div class="flex items-center gap-3">
          {#if $topbarConfig?.action}
            <Button
              variant="secondary"
              size="sm"
              className="h-8 gap-1.5"
              disabled={$topbarConfig.action.disabled}
              on:click={() => $topbarConfig?.action?.run()}
            >
              <RefreshCw class="h-3.5 w-3.5 {$topbarConfig.action.disabled ? 'animate-spin' : ''}" />
              {$topbarConfig.action.label}
            </Button>
          {/if}
          <div class="flex items-center gap-2.5">
            <div class="dash-avatar w-7 h-7 rounded-full flex items-center justify-center">
              <User class="h-3.5 w-3.5" />
            </div>
            <div class="hidden sm:block">
              <p class="text-xs font-medium text-white/80 leading-tight">Account</p>
              <p class="text-xs text-white/40 leading-tight">Active</p>
            </div>
          </div>
        </div>
      </div>
    </header>

    <!-- Scrollable page area -->
    <main class="flex-1 overflow-auto w-full">
      <div class="p-6">
        <slot></slot>
      </div>
    </main>
  </div>

  <!-- ── Mobile sidebar overlay ── -->
  {#if sidebarOpen}
    <div class="fixed inset-0 z-50 lg:hidden">
      <!-- Backdrop -->
      <button type="button" class="absolute inset-0 bg-black/50 backdrop-blur-sm" aria-label="Close menu" on:click={() => (sidebarOpen = false)}></button>
      <!-- Drawer -->
      <aside class="absolute inset-y-0 left-0 w-72 dash-sidebar flex flex-col">
        <div class="dash-sidebar-header">
          <div class="dash-wordmark-row">
            <div class="dash-logo-icon"><Server class="h-5 w-5 text-white" /></div>
            <span class="dash-wordmark">Talos</span>
          </div>
          <button type="button" on:click={() => (sidebarOpen = false)} class="p-1.5 rounded-lg hover:bg-white/8" aria-label="Close">
            <X class="h-5 w-5 text-white/60" />
          </button>
        </div>
        <nav class="flex-1 overflow-y-auto py-4 px-3 space-y-1 pb-20">
          {#each navigationItems as item}
            {#if item.children}
              <div class="space-y-0.5">
                {#if item.name === 'Patches'}
                  <div class="nav-parent-row">
                    <a href={item.href} class="nav-item nav-parent-link" class:nav-active={isActiveRoute(item.href, currentPath, { exact: true })} on:click={() => (sidebarOpen = false)}>
                      <svelte:component this={item.icon} class="mr-3 h-4 w-4 flex-shrink-0" />
                      <span class="flex-1 text-left">{item.name}</span>
                    </a>
                    <button type="button" on:click={() => toggleSection(item.name)} class="nav-item nav-expander" aria-label="Toggle Patches submenu">
                      <ChevronDown class="h-3.5 w-3.5 opacity-60 transition-transform {expandedSections.has(item.name) ? 'rotate-180' : ''}" />
                    </button>
                  </div>
                {:else}
                  <button type="button" on:click={() => toggleSection(item.name)}
                    class="nav-item w-full" class:nav-active={isActiveSection(item, currentPath)}>
                    <svelte:component this={item.icon} class="mr-3 h-4 w-4 flex-shrink-0" />
                    <span class="flex-1 text-left">{item.name}</span>
                    <ChevronDown class="h-3.5 w-3.5 opacity-60 transition-transform {expandedSections.has(item.name) ? 'rotate-180' : ''}" />
                  </button>
                {/if}
                {#if expandedSections.has(item.name)}
                  <div class="ml-4 pl-3 border-l border-white/10 space-y-0.5 mt-0.5">
                    {#each item.children as child}
                      <a href={child.href} class="nav-item nav-child" class:nav-active={isActiveRoute(child.href, currentPath, { exact: true })} on:click={() => (sidebarOpen = false)}>
                        <svelte:component this={child.icon} class="mr-3 h-3.5 w-3.5 flex-shrink-0" />
                        {child.name}
                      </a>
                    {/each}
                  </div>
                {/if}
              </div>
            {:else}
              <a href={item.href} class="nav-item" class:nav-active={isActiveRoute(item.href, currentPath)} on:click={() => (sidebarOpen = false)}>
                <svelte:component this={item.icon} class="mr-3 h-4 w-4 flex-shrink-0" />
                {item.name}
              </a>
            {/if}
          {/each}
        </nav>
        <div class="dash-sidebar-footer">
          <button type="button" on:click={toggleCursorEffects} class="nav-item w-full theme-toggle-nav" aria-label="Toggle cursor effects">
            <MousePointer2 class="mr-3 h-4 w-4 flex-shrink-0" />
            <span class="flex-1 text-left">Cursor Effects</span>
            <div class="aero-toggle-track">
              <div class="aero-toggle-thumb" class:is-active={$cursorEffectsEnabled}></div>
            </div>
          </button>
          <button type="button" on:click={toggleTheme} class="nav-item w-full theme-toggle-nav" aria-label="Toggle light/dark mode">
            <Moon class="mr-3 h-4 w-4 flex-shrink-0" />
            <span class="flex-1 text-left">Theme</span>
            <div class="aero-toggle-track">
              <div class="aero-toggle-thumb" class:is-light={$isLightMode}></div>
            </div>
            <Sun class="ml-2 h-3.5 w-3.5 flex-shrink-0 opacity-60" />
          </button>
          <button type="button" on:click={handleLogout} class="nav-item w-full logout-btn">
            <LogOut class="mr-3 h-4 w-4 flex-shrink-0" />Sign Out
          </button>
        </div>
      </aside>
    </div>
  {/if}
</div>

<style>
  /* ── Shell ── */
  .dash-shell {
    position: relative;
    z-index: 1;
    background: transparent;
  }

  /* ── Sidebar ── */
  .dash-sidebar {
    background: rgba(6, 14, 40, 0.75);
    backdrop-filter: blur(28px) saturate(160%);
    -webkit-backdrop-filter: blur(28px) saturate(160%);
    border-right: 1px solid rgba(255, 255, 255, 0.07);
    box-shadow: inset -1px 0 0 rgba(255,255,255,0.04), 4px 0 32px rgba(0,0,0,0.35);
  }

  .dash-sidebar-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    height: 56px;
    padding: 0 16px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.07);
    flex-shrink: 0;
  }
  .dash-wordmark-row { display: flex; align-items: center; gap: 10px; }
  .dash-logo-icon {
    width: 32px; height: 32px; border-radius: 9px;
    background: linear-gradient(145deg, rgba(70,150,255,0.85), rgba(20,80,210,0.9));
    border: 1px solid rgba(120,190,255,0.3);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.35), 0 0 14px rgba(50,130,255,0.35);
    display: flex; align-items: center; justify-content: center;
  }
  .dash-wordmark {
    font-size: 1.1rem; font-weight: 700; letter-spacing: -0.02em;
    background: linear-gradient(180deg, #ffffff 0%, rgba(160,210,255,0.8) 100%);
    -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
  }

  .dash-sidebar-footer {
    padding: 12px;
    border-top: 1px solid rgba(255, 255, 255, 0.07);
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  /* ── Top bar ── */
  .dash-topbar {
    height: 56px;
    background: rgba(6, 14, 40, 0.55);
    backdrop-filter: blur(28px) saturate(180%);
    -webkit-backdrop-filter: blur(28px) saturate(180%);
    border-bottom: 1px solid rgba(255, 255, 255, 0.07);
    box-shadow: inset 0 -1px 0 rgba(255,255,255,0.03), 0 4px 24px rgba(0,0,0,0.35);
    flex-shrink: 0;
  }

  .topbar-title {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 700;
    color: rgba(232, 237, 247, 0.95);
    letter-spacing: 0;
  }

  /* ── Nav items ── */
  .nav-item {
    display: flex;
    align-items: center;
    padding: 8px 12px;
    border-radius: 8px;
    font-size: 0.85rem;
    font-weight: 500;
    color: rgba(190, 220, 255, 0.62);
    text-decoration: none;
    transition: background 0.15s, color 0.15s, box-shadow 0.15s;
    border: 1px solid transparent;
  }
  .nav-item:hover {
    background: rgba(255, 255, 255, 0.07);
    color: rgba(220, 240, 255, 0.9);
  }
  .nav-item.nav-active {
    background: rgba(55, 130, 255, 0.14);
    border-color: rgba(80, 160, 255, 0.2);
    color: rgba(220, 240, 255, 0.95);
    box-shadow: inset 0 1px 0 rgba(255,255,255,0.08);
  }
  .nav-parent-row {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }
  .nav-parent-link {
    flex: 1;
    min-width: 0;
  }
  .nav-expander {
    width: 2.25rem;
    justify-content: center;
    padding: 8px;
  }
  .nav-child { padding: 6px 12px; font-size: 0.82rem; }

  .logout-btn { color: rgba(255, 160, 160, 0.6); }
  .logout-btn:hover { color: rgba(255, 180, 180, 0.9); background: rgba(255, 80, 80, 0.08); }

  /* ── Light mode ── */
  :global(html.light) .dash-sidebar {
    background: rgba(255, 255, 255, 0.6);
    border-right-color: rgba(100, 158, 220, 0.22);
    box-shadow: 4px 0 24px rgba(0, 38, 120, 0.08);
  }
  :global(html.light) .dash-sidebar-header { border-bottom-color: rgba(100,158,220,0.15); }
  :global(html.light) .dash-sidebar-footer { border-top-color: rgba(100,158,220,0.15); }
  :global(html.light) .dash-wordmark {
    background: linear-gradient(180deg, #081428 0%, #1a4888 100%);
    -webkit-background-clip: text; -webkit-text-fill-color: transparent; background-clip: text;
  }
  :global(html.light) .nav-item { color: rgba(10, 40, 120, 0.62); }
  :global(html.light) .nav-item:hover { background: rgba(50,100,200,0.08); color: #0a1628; }
  :global(html.light) .nav-item.nav-active {
    background: rgba(50,120,255,0.1); border-color: rgba(50,120,255,0.22); color: #081428;
  }
  :global(html.light) .logout-btn { color: rgba(180, 30, 30, 0.62); }
  :global(html.light) .logout-btn:hover { background: rgba(220,30,30,0.08); color: rgba(180,20,20,0.9); }
  :global(html.light) .dash-topbar {
    background: rgba(255,255,255,0.62);
    border-bottom-color: rgba(100,158,220,0.18);
    box-shadow: 0 4px 24px rgba(0,38,120,0.08);
  }
  :global(html.light) .dash-topbar [class*="text-white/"]:not(.aero-btn) { color: rgba(10, 42, 108, 0.52); }
  :global(html.light) .dash-topbar [class*="text-white"]:not([class*="text-white/"]):not(.aero-btn) { color: rgba(10, 30, 95, 0.8); }

  /* Topbar avatar — visible in both dark and light mode */
  .dash-avatar {
    background: rgba(255, 255, 255, 0.12);
    border: 1px solid rgba(255, 255, 255, 0.18);
  }
  .dash-avatar :global(svg) { color: rgba(255, 255, 255, 0.8); }
  :global(html.light) .dash-avatar {
    background: rgba(48, 118, 255, 0.18);
    border-color: rgba(48, 118, 255, 0.35);
  }
  :global(html.light) .dash-avatar :global(svg) { color: rgba(18, 68, 200, 0.85); }
</style>
