<script lang="ts">
  import { onDestroy } from 'svelte';
  import { MousePointer2 } from 'lucide-svelte';
  import { commandCenterApi } from '$lib/api';
  import type { CommandCenterMessageAttachment } from '$lib/types';

  export let attachment: CommandCenterMessageAttachment;

  let displayedUrl = '';
  let displayedAttachment: CommandCenterMessageAttachment | null = null;
  let previousUrl = '';
  let previousAttachment: CommandCenterMessageAttachment | null = null;
  let requestedArtifactId = '';
  let loadSerial = 0;
  let error = '';
  let loading = true;
  let releasePreviousTimer: ReturnType<typeof setTimeout> | null = null;

  $: visibleAttachment = displayedAttachment ?? attachment;
  $: isLiveFrame = visibleAttachment.presentation === 'live_frame';
  $: frameStyle =
    visibleAttachment.width && visibleAttachment.height
      ? `aspect-ratio: ${visibleAttachment.width} / ${visibleAttachment.height};`
      : '';
  $: cursor = visibleAttachment.cursor;
  $: showCursor =
    isLiveFrame &&
    cursor?.visible === true &&
    typeof cursor.x === 'number' &&
    typeof cursor.y === 'number' &&
    cursor.width > 0 &&
    cursor.height > 0;
  $: cursorStyle = showCursor
    ? `left: ${Math.max(0, Math.min(100, (cursor!.x! / cursor!.width) * 100))}%; top: ${Math.max(0, Math.min(100, (cursor!.y! / cursor!.height) * 100))}%;`
    : '';

  const revokeUrl = (url: string) => {
    if (url) URL.revokeObjectURL(url);
  };

  const clearPreviousFrame = () => {
    if (releasePreviousTimer) {
      clearTimeout(releasePreviousTimer);
      releasePreviousTimer = null;
    }
    revokeUrl(previousUrl);
    previousUrl = '';
    previousAttachment = null;
  };

  const releasePreviousFrameSoon = (url: string) => {
    if (releasePreviousTimer) clearTimeout(releasePreviousTimer);
    releasePreviousTimer = setTimeout(() => {
      if (previousUrl === url) {
        revokeUrl(previousUrl);
        previousUrl = '';
        previousAttachment = null;
      }
      releasePreviousTimer = null;
    }, 180);
  };

  const decodeImage = async (url: string) => {
    const image = new Image();
    image.decoding = 'async';
    image.src = url;
    if (typeof image.decode === 'function') {
      await image.decode();
      return;
    }
    await new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error('Image failed to decode'));
    });
  };

  const loadArtifact = async (nextAttachment: CommandCenterMessageAttachment) => {
    const serial = ++loadSerial;
    requestedArtifactId = nextAttachment.artifactId;
    loading = true;
    error = '';
    let nextUrl = '';

    try {
      const blob = await commandCenterApi.getArtifactContent(nextAttachment.artifactId);
      if (serial !== loadSerial) return;
      nextUrl = URL.createObjectURL(blob);
      await decodeImage(nextUrl);
      if (serial !== loadSerial) {
        revokeUrl(nextUrl);
        return;
      }

      const oldUrl = displayedUrl;
      const oldAttachment = displayedAttachment;
      if (isLiveFrame && oldUrl && oldUrl !== nextUrl) {
        clearPreviousFrame();
        previousUrl = oldUrl;
        previousAttachment = oldAttachment;
      } else {
        clearPreviousFrame();
      }
      displayedUrl = nextUrl;
      displayedAttachment = nextAttachment;
      loading = false;
      if (isLiveFrame && oldUrl) {
        releasePreviousFrameSoon(oldUrl);
      } else {
        revokeUrl(oldUrl);
      }
    } catch (err) {
      if (nextUrl) revokeUrl(nextUrl);
      if (serial === loadSerial) {
        error = err instanceof Error ? err.message : 'Image failed to load';
        loading = false;
      }
    }
  };

  $: if (attachment.artifactId && attachment.artifactId !== requestedArtifactId) {
    void loadArtifact(attachment);
  }

  onDestroy(() => {
    loadSerial += 1;
    clearPreviousFrame();
    revokeUrl(displayedUrl);
  });
</script>

{#if displayedUrl || !isLiveFrame || !loading}
  <figure class="attachment-image" class:live-frame={isLiveFrame}>
    <div class="attachment-frame" style={frameStyle}>
      {#if displayedUrl}
        {#if isLiveFrame}
          <div class="desktop-frame-shell">
            {#if previousUrl && previousAttachment}
              <img
                class="desktop-frame desktop-frame-layer previous"
                src={previousUrl}
                alt=""
                width={previousAttachment.width}
                height={previousAttachment.height}
                aria-hidden="true"
                loading="eager"
              />
            {/if}
            <img
              class="desktop-frame desktop-frame-layer current"
              src={displayedUrl}
              alt="Live desktop frame"
              width={visibleAttachment.width}
              height={visibleAttachment.height}
              loading="eager"
            />
            {#if showCursor}
              <MousePointer2 class="talos-cursor talos-cursor-mask" aria-hidden="true" style={cursorStyle} />
              <MousePointer2 class="talos-cursor" aria-hidden="true" style={cursorStyle} />
            {/if}
          </div>
        {:else}
          <img
            class="desktop-frame"
            src={displayedUrl}
            alt={visibleAttachment.name}
            width={visibleAttachment.width}
            height={visibleAttachment.height}
            loading="lazy"
          />
        {/if}
      {:else if loading}
        <div class="attachment-placeholder">Loading image...</div>
      {:else}
        <div class="attachment-placeholder error">{error || 'Image unavailable'}</div>
      {/if}
    </div>
    {#if !isLiveFrame}
      <figcaption>{visibleAttachment.name}</figcaption>
    {/if}
  </figure>
{/if}

<style>
  .attachment-image {
    display: grid;
    gap: 8px;
    margin: 10px 0 0;
  }

  .attachment-frame {
    position: relative;
    width: min(100%, 720px);
    overflow: hidden;
  }

  .attachment-image.live-frame .attachment-frame {
    border-radius: 8px;
    background: rgba(3, 9, 25, 0.58);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.08), 0 14px 34px rgba(0, 0, 0, 0.2);
  }

  .desktop-frame,
  .attachment-placeholder {
    width: 100%;
    max-height: 520px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.045);
    object-fit: contain;
  }

  .desktop-frame {
    display: block;
    height: 100%;
  }

  .desktop-frame-shell {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 180px;
    overflow: hidden;
    border: 1px solid rgba(125, 180, 255, 0.22);
    border-radius: 8px;
    background: rgb(5, 14, 32);
  }

  .attachment-image.live-frame .desktop-frame {
    position: absolute;
    inset: 0;
    border: 0;
    border-radius: 0;
    max-height: none;
    height: 100%;
    background: rgb(5, 14, 32);
    object-fit: contain;
  }

  .desktop-frame-layer.previous {
    z-index: 0;
  }

  .desktop-frame-layer.current {
    z-index: 1;
  }

  :global(.talos-cursor) {
    position: absolute;
    z-index: 3;
    width: clamp(26px, 4.4%, 40px);
    height: clamp(26px, 4.4%, 40px);
    color: rgb(17, 24, 39);
    fill: rgb(248, 250, 252);
    stroke-width: 2;
    filter:
      drop-shadow(0 1px 0 rgba(255, 255, 255, 0.72))
      drop-shadow(0 4px 8px rgba(0, 0, 0, 0.38));
    pointer-events: none;
    transform: translate(-3px, -3px);
  }

  :global(.talos-cursor-mask) {
    z-index: 2;
    width: clamp(32px, 5.2%, 48px);
    height: clamp(32px, 5.2%, 48px);
    color: rgba(255, 255, 255, 0.8);
    fill: rgba(255, 255, 255, 0.8);
    filter: none;
    transform: translate(-5px, -5px);
  }

  .attachment-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 180px;
    color: rgba(170, 205, 255, 0.62);
    font-size: 12px;
  }

  .attachment-placeholder.error {
    color: rgba(252, 165, 165, 0.88);
  }

  figcaption {
    color: rgba(170, 205, 255, 0.54);
    font-size: 11px;
  }
</style>
