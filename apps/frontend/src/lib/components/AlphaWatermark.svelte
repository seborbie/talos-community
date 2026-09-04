<div class="alpha-watermark" aria-label="Alpha build. Things may change.">
  <span class="alpha-watermark-title">Alpha build</span>
  <span class="alpha-watermark-copy">Things may change.</span>
</div>

<style>
  .alpha-watermark {
    position: fixed;
    right: calc(env(safe-area-inset-right, 0px) + 0.75rem);
    bottom: calc(env(safe-area-inset-bottom, 0px) + 0.75rem);
    z-index: 40;
    pointer-events: none;
    display: flex;
    max-width: min(18rem, calc(100vw - 1.5rem));
    align-items: center;
    gap: 0.55rem;
    border-radius: 999px;
    border: 1px solid rgba(120, 190, 255, 0.22);
    background: rgba(6, 14, 40, 0.62);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.11),
      0 8px 28px rgba(0, 0, 0, 0.32);
    color: rgba(220, 240, 255, 0.78);
    padding: 0.4rem 0.7rem;
    font-size: 0.72rem;
    line-height: 1.15;
    backdrop-filter: blur(22px) saturate(165%);
    -webkit-backdrop-filter: blur(22px) saturate(165%);
  }

  .alpha-watermark-title {
    flex-shrink: 0;
    font-weight: 700;
    color: rgba(240, 248, 255, 0.92);
  }

  .alpha-watermark-copy {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: rgba(190, 220, 255, 0.6);
  }

  :global(html.light) .alpha-watermark {
    border-color: rgba(78, 148, 220, 0.26);
    background: rgba(255, 255, 255, 0.72);
    box-shadow:
      inset 0 1px 0 rgba(255, 255, 255, 0.88),
      0 8px 26px rgba(0, 38, 120, 0.12);
    color: rgba(10, 42, 108, 0.72);
  }

  :global(html.light) .alpha-watermark-title {
    color: rgba(8, 20, 40, 0.88);
  }

  :global(html.light) .alpha-watermark-copy {
    color: rgba(10, 42, 108, 0.58);
  }

</style>
