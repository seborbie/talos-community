const POWERSHELL_PROMPT_PATTERN = /^\s*PS\s+[^>]*>\s*/i;
const CONTINUATION_PROMPT_PATTERN = /^\s*>{2,}\s*(.*)$/;
const TALOS_MARKER_PATTERN = /__TALOS_CMD_(?:START|END)_[A-Za-z0-9_]+__/i;

const stripPowerShellPrompt = (line: string) => line.replace(POWERSHELL_PROMPT_PATTERN, '');

const isPromptEcho = (line: string) => POWERSHELL_PROMPT_PATTERN.test(line) || CONTINUATION_PROMPT_PATTERN.test(line);

const isWrapperNoiseLine = (line: string, promptEcho: boolean) => {
  const trimmed = line.trim();
  const withoutPrompt = stripPowerShellPrompt(line).trim();
  const candidates = [trimmed, withoutPrompt];

  return candidates.some((candidate) => {
    if (!candidate) return false;
    if (TALOS_MARKER_PATTERN.test(candidate)) return true;
    if (/\$__talosExit\b/i.test(candidate)) return true;
    if (/^\$global:LASTEXITCODE\s*=\s*\$null$/i.test(candidate)) return true;
    if (promptEcho && /^Write-Error\s+\$_$/i.test(candidate)) return true;
    if (promptEcho && /^(?:try\s*\{|}\s*catch\s*\{|})$/i.test(candidate)) return true;
    return false;
  });
};

export function normalizeCommandCenterTerminalOutput(value: string): string {
  if (!value) return '';

  const normalized = value.replace(/\r\n/g, '\n');
  const lines = normalized.split('\n');
  const output: string[] = [];

  for (const line of lines) {
    const continuationMatch = line.match(CONTINUATION_PROMPT_PATTERN);
    const promptEcho = isPromptEcho(line);
    const lineWithoutContinuation = continuationMatch ? continuationMatch[1] ?? '' : line;

    if (continuationMatch && !lineWithoutContinuation.trim()) {
      continue;
    }
    if (isWrapperNoiseLine(lineWithoutContinuation, promptEcho)) {
      continue;
    }

    output.push(lineWithoutContinuation);
  }

  return output.join('\n');
}
