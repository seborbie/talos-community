import assert from 'node:assert/strict';
import { test } from 'node:test';
import { normalizeCommandCenterTerminalOutput } from './commandCenterTerminal';

test('normalizeCommandCenterTerminalOutput removes prompt-only continuation lines', () => {
  assert.equal(
    normalizeCommandCenterTerminalOutput('>>\r\n>>\r\nDownloading package...\r\n>>\r\nInstall complete'),
    'Downloading package...\nInstall complete'
  );
});

test('normalizeCommandCenterTerminalOutput removes Talos Windows wrapper echoes', () => {
  const input = [
    'PS C:\\Windows\\system32> $global:LASTEXITCODE = $null',
    '>> try {',
    '>> choco list --local-only --exact treesizefree',
    'treesizefree 4.7.3',
    'PS C:\\Windows\\system32> if (-not $?) { $__talosExit = 1 }',
    'PS C:\\Windows\\system32> Write-Output "__TALOS_CMD_END_test__:$__talosExit"',
    'PS C:\\Windows\\system32> Write-Error $_',
    'PS C:\\Windows\\system32> }',
    '1 packages installed.'
  ].join('\r\n');

  assert.equal(
    normalizeCommandCenterTerminalOutput(input),
    'choco list --local-only --exact treesizefree\ntreesizefree 4.7.3\n1 packages installed.'
  );
});

test('normalizeCommandCenterTerminalOutput preserves real PowerShell paths and errors', () => {
  const input = [
    'PS C:\\Windows\\system32> Get-Item C:\\Windows\\System32\\drivers\\etc\\hosts',
    'C:\\Windows\\System32\\drivers\\etc\\hosts exists',
    'Write-Error $_ failed for C:\\Temp\\example.txt'
  ].join('\n');

  assert.equal(normalizeCommandCenterTerminalOutput(input), input);
});

test('normalizeCommandCenterTerminalOutput preserves wrapper-looking content outside prompt echoes', () => {
  const input = ['try {', 'Write-Error $_', '} catch {', '}'].join('\n');

  assert.equal(normalizeCommandCenterTerminalOutput(input), input);
});

test('normalizeCommandCenterTerminalOutput preserves carriage-return progress updates', () => {
  const input = 'Downloading 10%\rDownloading 40%\rDownloading 100%';

  assert.equal(normalizeCommandCenterTerminalOutput(input), input);
});

test('normalizeCommandCenterTerminalOutput preserves ANSI color sequences', () => {
  const input = '\x1b[32mInstalled\x1b[0m package';

  assert.equal(normalizeCommandCenterTerminalOutput(input), input);
});

test('normalizeCommandCenterTerminalOutput handles truncated rerender input', () => {
  const input = [
    '__TALOS_CMD_START_previous__',
    'middle of retained buffer',
    '>>',
    'PS C:\\Windows\\system32> $global:LASTEXITCODE = $null',
    'final line'
  ].join('\r\n');

  assert.equal(normalizeCommandCenterTerminalOutput(input), 'middle of retained buffer\nfinal line');
});
