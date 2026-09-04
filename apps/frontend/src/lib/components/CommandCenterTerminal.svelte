<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { normalizeCommandCenterTerminalOutput } from '$lib/commandCenterTerminal';
  import '@xterm/xterm/css/xterm.css';
  import type { IDisposable, ITheme, Terminal as XtermTerminal } from '@xterm/xterm';
  import type { FitAddon as XtermFitAddon } from '@xterm/addon-fit';

  export let jobId: string | null = null;
  export let output = '';
  export let placeholder = '';
  export let status = '';

  const CLEAR_TERMINAL = '\x1b[2J\x1b[3J\x1b[H';

  const darkTheme: ITheme = {
    background: '#02060c',
    foreground: '#e2f2ff',
    cursor: '#02060c',
    selectionBackground: '#264f78',
    black: '#02060c',
    red: '#f87171',
    green: '#7dd3a8',
    yellow: '#f5d76e',
    blue: '#60a5fa',
    magenta: '#c084fc',
    cyan: '#67e8f9',
    white: '#e2f2ff',
    brightBlack: '#64748b',
    brightRed: '#fca5a5',
    brightGreen: '#a7f3d0',
    brightYellow: '#fde68a',
    brightBlue: '#93c5fd',
    brightMagenta: '#d8b4fe',
    brightCyan: '#a5f3fc',
    brightWhite: '#f8fafc'
  };

  const lightTheme: ITheme = {
    background: '#f8fbff',
    foreground: '#081c58',
    cursor: '#f8fbff',
    selectionBackground: '#bfdbfe',
    black: '#081c58',
    red: '#b91c1c',
    green: '#047857',
    yellow: '#92400e',
    blue: '#1d4ed8',
    magenta: '#7e22ce',
    cyan: '#0e7490',
    white: '#f8fbff',
    brightBlack: '#64748b',
    brightRed: '#dc2626',
    brightGreen: '#059669',
    brightYellow: '#b45309',
    brightBlue: '#2563eb',
    brightMagenta: '#9333ea',
    brightCyan: '#0891b2',
    brightWhite: '#ffffff'
  };

  let terminalEl: HTMLDivElement | null = null;
  let terminal: XtermTerminal | null = null;
  let fitAddon: XtermFitAddon | null = null;
  let resizeObserver: ResizeObserver | null = null;
  let themeObserver: MutationObserver | null = null;
  let scrollDisposable: IDisposable | null = null;
  let mounted = false;
  let terminalFollowing = true;
  let renderedJobId: string | null = null;
  let renderedOutput = '';
  let renderedPlaceholder = '';
  let renderedHasOutput = false;

  $: normalizedOutput = normalizeCommandCenterTerminalOutput(output);
  $: {
    const nextJobId = jobId;
    const nextOutput = normalizedOutput;
    const nextPlaceholder = placeholder;
    if (terminal) {
      renderTerminal(nextJobId, nextOutput, nextPlaceholder);
    }
  }

  onMount(() => {
    mounted = true;
    void initTerminal();

    return () => {
      mounted = false;
      disposeTerminal();
    };
  });

  async function initTerminal() {
    await tick();
    if (!mounted || !terminalEl || terminal) return;

    const [{ Terminal }, { FitAddon }] = await Promise.all([import('@xterm/xterm'), import('@xterm/addon-fit')]);
    if (!mounted || !terminalEl || terminal) return;

    const term = new Terminal({
      allowTransparency: false,
      convertEol: true,
      cursorBlink: false,
      disableStdin: true,
      fontFamily: "'Cascadia Code', 'Consolas', 'Courier New', monospace",
      fontSize: 12,
      lineHeight: 1.32,
      scrollback: 4_000,
      theme: currentTheme()
    });
    const nextFitAddon = new FitAddon();

    term.loadAddon(nextFitAddon);
    term.open(terminalEl);

    terminal = term;
    fitAddon = nextFitAddon;
    scrollDisposable = term.onScroll(() => {
      terminalFollowing = isTerminalAtBottom();
    });
    resizeObserver = new ResizeObserver(() => {
      fitTerminal();
      if (terminalFollowing) terminal?.scrollToBottom();
    });
    resizeObserver.observe(terminalEl);
    themeObserver = new MutationObserver(syncTerminalTheme);
    themeObserver.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });

    fitTerminal();
    syncTerminalTheme();
    renderTerminal(jobId, normalizedOutput, placeholder, true);
  }

  function disposeTerminal() {
    resizeObserver?.disconnect();
    resizeObserver = null;
    themeObserver?.disconnect();
    themeObserver = null;
    scrollDisposable?.dispose();
    scrollDisposable = null;
    terminal?.dispose();
    terminal = null;
    fitAddon = null;
  }

  function currentTheme() {
    return document.documentElement.classList.contains('light') ? lightTheme : darkTheme;
  }

  function syncTerminalTheme() {
    if (!terminal) return;
    terminal.options.theme = currentTheme();
  }

  function fitTerminal() {
    if (!fitAddon || !terminalEl) return;
    try {
      fitAddon.fit();
    } catch {
      // xterm can throw while the container is transitioning through a zero-size layout.
    }
  }

  function isTerminalAtBottom() {
    if (!terminal) return true;
    const buffer = terminal.buffer.active;
    return buffer.viewportY >= buffer.baseY - 1;
  }

  function writeTerminal(text: string, follow: boolean) {
    if (!terminal) return;
    terminal.write(text, () => {
      if (follow) {
        terminal?.scrollToBottom();
        terminalFollowing = true;
      } else {
        terminalFollowing = isTerminalAtBottom();
      }
    });
  }

  function renderTerminal(nextJobId: string | null, nextOutput: string, nextPlaceholder: string, force = false) {
    if (!terminal) return;

    const hasOutput = nextOutput.length > 0;
    const displayText = hasOutput ? nextOutput : nextPlaceholder;
    const jobChanged = nextJobId !== renderedJobId;
    const sourceChanged = hasOutput !== renderedHasOutput;
    const outputWasTruncated = hasOutput && !nextOutput.startsWith(renderedOutput);
    const placeholderChanged = !hasOutput && nextPlaceholder !== renderedPlaceholder;
    const shouldReset = force || jobChanged || sourceChanged || outputWasTruncated || placeholderChanged;
    const shouldFollow = jobChanged || terminalFollowing || isTerminalAtBottom();

    if (shouldReset) {
      renderedJobId = nextJobId;
      renderedOutput = hasOutput ? nextOutput : '';
      renderedPlaceholder = hasOutput ? '' : nextPlaceholder;
      renderedHasOutput = hasOutput;
      terminalFollowing = true;
      writeTerminal(`${CLEAR_TERMINAL}${displayText}`, true);
      return;
    }

    if (!hasOutput) return;

    const suffix = nextOutput.slice(renderedOutput.length);
    if (!suffix) return;

    renderedOutput = nextOutput;
    writeTerminal(suffix, shouldFollow);
  }
</script>

<div class="runner-console-xterm" data-status={status}>
  <div
    class="runner-console-xterm-host"
    bind:this={terminalEl}
    role="log"
    aria-live="polite"
    aria-atomic="false"
    aria-label={status ? `Terminal ${status}` : 'Terminal output'}
  ></div>
</div>

<style>
  .runner-console-xterm {
    height: clamp(190px, 28vh, 360px);
    max-height: 360px;
    min-height: 190px;
    overflow: hidden;
    padding: 10px;
    background: rgba(2, 6, 12, 0.78);
    color: rgba(226, 242, 255, 0.94);
    user-select: text;
    -webkit-user-select: text;
  }

  .runner-console-xterm-host {
    width: 100%;
    height: 100%;
  }

  .runner-console-xterm :global(.xterm) {
    height: 100%;
  }

  .runner-console-xterm :global(.xterm-viewport) {
    background: transparent !important;
  }

  .runner-console-xterm :global(.xterm-cursor-layer) {
    display: none;
    pointer-events: none;
  }

  :global(html.light) .runner-console-xterm {
    background: rgba(255, 255, 255, 0.66);
    color: rgba(8, 28, 88, 0.9);
  }

  @media (max-width: 900px) {
    .runner-console-xterm {
      height: clamp(190px, 30vh, 300px);
      max-height: 300px;
    }
  }
</style>
