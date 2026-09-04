const PLACEHOLDER_PREFIX = '\u0000MD';
const PLACEHOLDER_SUFFIX = '\u0000';

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}

function safeHref(value: string): string | null {
  const decoded = value.replace(/&amp;/g, '&').trim();
  if (/^\/SN\/[a-z0-9]{8}$/i.test(decoded)) {
    return escapeHtml(decoded);
  }
  if (!/^(https?:|mailto:)/i.test(decoded)) {
    return null;
  }
  try {
    const url = new URL(decoded);
    if (url.protocol === 'http:' || url.protocol === 'https:' || url.protocol === 'mailto:') {
      return escapeHtml(url.href);
    }
  } catch {
    return null;
  }
  return null;
}

function currentOrigin(): string {
  if (typeof window !== 'undefined' && window.location?.origin) {
    return window.location.origin;
  }
  return '';
}

function splitTrailingPunctuation(value: string): { core: string; trailing: string } {
  let core = value;
  let trailing = '';
  while (/[),.;:!?]$/.test(core)) {
    trailing = `${core.slice(-1)}${trailing}`;
    core = core.slice(0, -1);
  }
  return { core, trailing };
}

function anchorHtml(href: string, label: string): string {
  return `<a href="${href}" target="_blank" rel="noreferrer">${label}</a>`;
}

function secureNoteAnchorHtml(code: string): string {
  const path = `/SN/${code.toLowerCase()}`;
  const origin = currentOrigin();
  const label = origin ? `${origin}${path}` : path;
  return anchorHtml(escapeHtml(path), escapeHtml(label));
}

function restorePlaceholders(value: string, placeholders: string[]): string {
  return value.replace(/\u0000MD(\d+)\u0000/g, (_, index) => placeholders[Number(index)] ?? '');
}

function renderInline(value: string): string {
  const placeholders: string[] = [];
  const withCode = value.replace(/`([^`]+)`/g, (_, code: string) => {
    const secureNote = code.trim().match(/^\/?SN\/([a-z0-9]{8})$/i);
    const replacement = secureNote
      ? secureNoteAnchorHtml(secureNote[1])
      : `<code>${escapeHtml(code)}</code>`;
    const index = placeholders.push(replacement) - 1;
    return `${PLACEHOLDER_PREFIX}${index}${PLACEHOLDER_SUFFIX}`;
  });

  let html = escapeHtml(withCode);
  html = html.replace(/\[([^\]]+)\]\(([^)\s]+)\)/g, (_, label: string, href: string) => {
    const safe = safeHref(href);
    if (!safe) return label;
    const index = placeholders.push(anchorHtml(safe, label)) - 1;
    return `${PLACEHOLDER_PREFIX}${index}${PLACEHOLDER_SUFFIX}`;
  });
  html = html.replace(/\bhttps?:\/\/[^\s<]+/gi, (match: string) => {
    const { core, trailing } = splitTrailingPunctuation(match);
    const safe = safeHref(core);
    if (!safe) return match;
    return `${anchorHtml(safe, escapeHtml(core))}${escapeHtml(trailing)}`;
  });
  html = html.replace(/\*\*\/SN\/([a-z0-9]{8})\*\*/gi, (_, code: string) => {
    return `<strong>${secureNoteAnchorHtml(code)}</strong>`;
  });
  html = html.replace(/(^|[\s(])\/SN\/([a-z0-9]{8})\b/gi, (_, prefix: string, code: string) => {
    return `${prefix}${secureNoteAnchorHtml(code)}`;
  });
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  html = html.replace(/(^|[^*])\*([^*\n]+)\*/g, '$1<em>$2</em>');
  return restorePlaceholders(html, placeholders);
}

export function renderMarkdown(value: string): string {
  const lines = value.replace(/\r\n/g, '\n').split('\n');
  const html: string[] = [];
  const paragraph: string[] = [];
  let listType: 'ul' | 'ol' | null = null;
  let inFence = false;
  let fenceLanguage = '';
  let codeLines: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    html.push(`<p>${renderInline(paragraph.join(' '))}</p>`);
    paragraph.length = 0;
  };

  const closeList = () => {
    if (!listType) return;
    html.push(`</${listType}>`);
    listType = null;
  };

  const openList = (type: 'ul' | 'ol') => {
    if (listType === type) return;
    closeList();
    html.push(`<${type}>`);
    listType = type;
  };

  const closeFence = () => {
    const languageClass = fenceLanguage ? ` class="language-${escapeHtml(fenceLanguage)}"` : '';
    html.push(`<pre><code${languageClass}>${escapeHtml(codeLines.join('\n'))}</code></pre>`);
    inFence = false;
    fenceLanguage = '';
    codeLines = [];
  };

  for (const rawLine of lines) {
    const line = rawLine.trimEnd();
    const fenceMatch = line.match(/^```([A-Za-z0-9_-]*)\s*$/);
    if (fenceMatch) {
      if (inFence) {
        closeFence();
      } else {
        flushParagraph();
        closeList();
        inFence = true;
        fenceLanguage = fenceMatch[1] ?? '';
        codeLines = [];
      }
      continue;
    }

    if (inFence) {
      codeLines.push(rawLine);
      continue;
    }

    if (!line.trim()) {
      flushParagraph();
      closeList();
      continue;
    }

    const heading = line.match(/^(#{1,3})\s+(.+)$/);
    if (heading) {
      flushParagraph();
      closeList();
      const level = heading[1].length;
      html.push(`<h${level}>${renderInline(heading[2].trim())}</h${level}>`);
      continue;
    }

    const unordered = line.match(/^\s*[-*]\s+(.+)$/);
    if (unordered) {
      flushParagraph();
      openList('ul');
      html.push(`<li>${renderInline(unordered[1].trim())}</li>`);
      continue;
    }

    const ordered = line.match(/^\s*\d+\.\s+(.+)$/);
    if (ordered) {
      flushParagraph();
      openList('ol');
      html.push(`<li>${renderInline(ordered[1].trim())}</li>`);
      continue;
    }

    paragraph.push(line.trim());
  }

  if (inFence) {
    closeFence();
  }
  flushParagraph();
  closeList();

  return html.join('');
}
