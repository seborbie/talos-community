<script lang="ts">
  import { onMount, tick } from 'svelte';

  type InvokeTauriFn = <T = unknown>(
    command: string,
    args?: Record<string, unknown>
  ) => Promise<T>;

  export let invokeTauri: InvokeTauriFn;
  export let connected: boolean;
  export let status: string = '';
  export let error: string | null = null;
  export let connectInFlight: boolean = false;
  export let onConnect: () => void = () => {};

  // v1 scope: only expose HKLM + HKU in the UI.
  type RegistryHive = 'HKLM' | 'HKU';

  type RegistryValueData =
    | { type: 'sz'; data: string }
    | { type: 'dword'; data: number }
    | { type: 'qword'; data: string }
    | { type: 'multi_sz'; data: string[] }
    | { type: 'binary'; dataB64: string }
    | { type: 'unknown'; rawType: number; dataB64: string };

  type RegistryValueEntry = {
    name: string;
    valueType: string;
    rawType: number;
    data: RegistryValueData;
  };

  const HIVE_LABELS: Record<RegistryHive, string> = {
    HKLM: 'HKEY_LOCAL_MACHINE',
    HKU: 'HKEY_USERS',
  };

  const HIVE_ORDER: RegistryHive[] = ['HKLM', 'HKU'];

  const BASIC_VALUE_TYPES = [
    'REG_SZ',
    'REG_DWORD',
    'REG_QWORD',
    'REG_MULTI_SZ',
    'REG_BINARY'
  ] as const;
  type RegistryBasicValueType = (typeof BASIC_VALUE_TYPES)[number];

  type ChildrenStatus = 'loading' | 'loaded' | 'error';
  type ChildrenState = { status: ChildrenStatus; children: string[]; error?: string };

  type TreeRow = {
    id: string;
    hive: RegistryHive;
    path: string;
    depth: number;
    label: string;
    fullPath: string;
    isExpanded: boolean;
    isSelected: boolean;
    canToggle: boolean;
    status: ChildrenStatus | 'idle';
    statusError?: string;
  };

  type ValueRow = {
    entry: RegistryValueEntry;
    nameLabel: string;
    preview: string;
  };

  const keyId = (hive: RegistryHive, path: string): string => `${hive}|${path}`;

  const joinKeyPath = (parent: string, child: string): string =>
    parent ? `${parent}\\${child}` : child;

  const normalizeKeyPath = (path: string): string =>
    path.trim().replace(/\//g, '\\').replace(/^\\+/, '').replace(/\\+$/, '');

  const formatKeyFullPath = (hive: RegistryHive, path: string): string => {
    // Match Windows Registry Editor / PowerShell provider style for copy/paste.
    // Example: Computer\HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft
    const hiveLabel = HIVE_LABELS[hive] ?? hive;
    return path ? `Computer\\${hiveLabel}\\${path}` : `Computer\\${hiveLabel}`;
  };

  const displayValueName = (name: string): string =>
    name.trim().length ? name : '(Default)';

  const parseParentPath = (path: string): { parent: string; leaf: string } => {
    const cleaned = normalizeKeyPath(path);
    if (!cleaned) return { parent: '', leaf: '' };
    const idx = cleaned.lastIndexOf('\\');
    if (idx < 0) return { parent: '', leaf: cleaned };
    return { parent: cleaned.slice(0, idx), leaf: cleaned.slice(idx + 1) };
  };

  const truncate = (text: string, maxLen: number): string => {
    if (text.length <= maxLen) return text;
    if (maxLen <= 3) return text.slice(0, maxLen);
    return `${text.slice(0, maxLen - 3)}...`;
  };

  const base64DecodedLength = (b64: string): number => {
    const s = b64.trim().replace(/\s+/g, '');
    if (!s) return 0;
    const pad = s.endsWith('==') ? 2 : s.endsWith('=') ? 1 : 0;
    return Math.max(0, Math.floor((s.length * 3) / 4) - pad);
  };

  const toHex8 = (n: number): string => n.toString(16).toUpperCase().padStart(8, '0');

  const tryParseU64 = (input: string): bigint | null => {
    const s = input.trim();
    if (!s) return null;
    try {
      const v = BigInt(s);
      if (v < 0n || v > 18446744073709551615n) return null;
      return v;
    } catch {
      return null;
    }
  };

  const parseDwordInput = (input: string): number | null => {
    const s = input.trim();
    if (!s) return null;
    const isHex = s.startsWith('0x') || s.startsWith('0X');
    const digits = isHex ? s.slice(2) : s;
    const radix = isHex ? 16 : 10;
    if (!digits.trim()) return null;
    const v = Number.parseInt(digits, radix);
    if (!Number.isFinite(v) || Number.isNaN(v)) return null;
    if (v < 0 || v > 0xffffffff) return null;
    return v >>> 0;
  };

  const parseQwordInputToDecimalString = (input: string): string | null => {
    const v = tryParseU64(input);
    if (v === null) return null;
    return v.toString(10);
  };

  const hexToBytes = (input: string): Uint8Array | null => {
    const cleaned = input
      .trim()
      .replace(/\s+/g, '')
      .replace(/,/g, '')
      .replace(/^0x/i, '');
    if (!cleaned) return new Uint8Array(0);
    if (cleaned.length % 2 !== 0) return null;
    const out = new Uint8Array(cleaned.length / 2);
    for (let i = 0; i < out.length; i++) {
      const byteHex = cleaned.slice(i * 2, i * 2 + 2);
      const v = Number.parseInt(byteHex, 16);
      if (!Number.isFinite(v) || Number.isNaN(v)) return null;
      out[i] = v;
    }
    return out;
  };

  const bytesToHex = (bytes: Uint8Array, maxBytes?: number): string => {
    const len = Math.min(bytes.length, typeof maxBytes === 'number' ? maxBytes : bytes.length);
    const parts: string[] = [];
    for (let i = 0; i < len; i++) {
      parts.push(bytes[i].toString(16).toUpperCase().padStart(2, '0'));
    }
    return parts.join(' ');
  };

  const bytesToBase64 = (bytes: Uint8Array): string => {
    const chunkSize = 0x2000;
    const parts: string[] = [];
    for (let i = 0; i < bytes.length; i += chunkSize) {
      parts.push(String.fromCharCode(...bytes.subarray(i, i + chunkSize)));
    }
    return btoa(parts.join(''));
  };

  const base64ToBytes = (b64: string): Uint8Array | null => {
    try {
      const bin = atob(b64.trim().replace(/\s+/g, ''));
      const out = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) {
        out[i] = bin.charCodeAt(i);
      }
      return out;
    } catch {
      return null;
    }
  };

  const isEditableEntry = (entry: RegistryValueEntry): RegistryBasicValueType | null => {
    const vt = entry.valueType.trim().toUpperCase();
    if (!BASIC_VALUE_TYPES.includes(vt as RegistryBasicValueType)) return null;
    // Ensure the structured variant matches, so we don't accidentally coerce types
    // (e.g. REG_EXPAND_SZ currently deserializes as `sz`, but we don't support writing it).
    const dt = entry.data.type;
    if (vt === 'REG_SZ' && dt === 'sz') return 'REG_SZ';
    if (vt === 'REG_DWORD' && dt === 'dword') return 'REG_DWORD';
    if (vt === 'REG_QWORD' && dt === 'qword') return 'REG_QWORD';
    if (vt === 'REG_MULTI_SZ' && dt === 'multi_sz') return 'REG_MULTI_SZ';
    if (vt === 'REG_BINARY' && dt === 'binary') return 'REG_BINARY';
    return null;
  };

  const formatValuePreview = (entry: RegistryValueEntry): string => {
    const d = entry.data;
    switch (d.type) {
      case 'sz':
        return truncate(d.data, 120);
      case 'dword':
        return `0x${toHex8(d.data)} (${d.data})`;
      case 'qword': {
        const v = tryParseU64(d.data);
        if (v === null) return truncate(d.data, 120);
        return `0x${v.toString(16).toUpperCase()} (${v.toString(10)})`;
      }
      case 'multi_sz':
        return truncate(d.data.join(' | '), 140);
      case 'binary': {
        const bytes = base64DecodedLength(d.dataB64);
        return `${bytes} bytes (binary)`;
      }
      case 'unknown': {
        const bytes = base64DecodedLength(d.dataB64);
        return `${bytes} bytes (${entry.valueType})`;
      }
    }
  };

  const parseRegistryPathInput = (
    input: string
  ): { hive: RegistryHive; path: string } | null => {
    let s = input.trim().replace(/\//g, '\\');
    if (!s) return null;
    if (s.toLowerCase().startsWith('computer\\')) {
      s = s.slice('computer\\'.length);
    }
    const parts = s.split('\\').filter((p) => p.length > 0);
    if (parts.length === 0) return null;

    const hiveRaw = parts[0].trim().toUpperCase();
    const hiveMap: Record<string, RegistryHive> = {
      HKLM: 'HKLM',
      HKEY_LOCAL_MACHINE: 'HKLM',
      HKU: 'HKU',
      HKEY_USERS: 'HKU',
    };
    const hive = hiveMap[hiveRaw];
    if (!hive) return null;
    const path = normalizeKeyPath(parts.slice(1).join('\\'));
    return { hive, path };
  };

  let childrenByKey: Record<string, ChildrenState> = {};
  let expanded = new Set<string>();

  let selectedHive: RegistryHive = 'HKLM';
  let selectedPath = '';
  let pathInput = formatKeyFullPath(selectedHive, selectedPath);
  let pathInputError: string | null = null;

  let values: RegistryValueEntry[] = [];
  let valuesLoading = false;
  let valuesError: string | null = null;
  let valueFilter = '';

  let actionError: string | null = null;
  let actionMessage: string | null = null;
  let opInFlight = false;

  let copyStatus: string | null = null;
  let copyStatusTimer: number | null = null;

  let valuesRequestSeq = 0;
  let didAutoInit = false;

  // Value editor state
  let editorOpen = false;
  let editorMode: 'create' | 'edit' = 'create';
  let editorOriginalName = '';
  let editorName = '';
  let editorType: RegistryBasicValueType = 'REG_SZ';
  let editorSz = '';
  let editorDword = '0';
  let editorQword = '0';
  let editorMultiSz = '';
  let editorBinaryFormat: 'hex' | 'base64' = 'hex';
  let editorBinary = '';
  let editorSaving = false;
  let editorError: string | null = null;

  // Lightweight dialogs (replace window.prompt/confirm).
  type RegistryDialogMode = 'create_subkey' | 'delete_key' | 'delete_tree' | 'delete_value';
  let dialogOpen = false;
  let dialogMode: RegistryDialogMode = 'create_subkey';
  let dialogBusy = false;
  let dialogError: string | null = null;
  let dialogSubkeyName = '';
  let dialogRecursive = false;
  let dialogValueEntry: RegistryValueEntry | null = null;

  const closeDialog = () => {
    if (dialogBusy) return;
    dialogOpen = false;
    dialogError = null;
    dialogSubkeyName = '';
    dialogRecursive = false;
    dialogValueEntry = null;
  };

  const buildTreeRows = (
    childrenMap: Record<string, ChildrenState>,
    expandedSet: Set<string>,
    selHive: RegistryHive,
    selPath: string
  ): TreeRow[] => {
    const rows: TreeRow[] = [];

    const pushNode = (hive: RegistryHive, path: string, label: string, depth: number) => {
      const id = keyId(hive, path);
      const fullPath = formatKeyFullPath(hive, path);
      const state = childrenMap[id];
      const isExpanded = expandedSet.has(id);
      const isSelected = hive === selHive && path === selPath;
      const status = state?.status ?? 'idle';
      const canToggle = status !== 'loaded' || (state?.children?.length ?? 0) > 0;

      rows.push({
        id,
        hive,
        path,
        depth,
        label,
        fullPath,
        isExpanded,
        isSelected,
        canToggle,
        status,
        statusError: state?.status === 'error' ? state.error : undefined
      });

      if (!isExpanded) return;
      const childNames = state?.children ?? [];
      for (const child of childNames) {
        pushNode(hive, joinKeyPath(path, child), child, depth + 1);
      }
    };

    for (const hive of HIVE_ORDER) {
      pushNode(hive, '', HIVE_LABELS[hive], 0);
    }
    return rows;
  };

  const invalidateChildren = (hive: RegistryHive, path: string) => {
    const id = keyId(hive, path);
    const next = { ...childrenByKey };
    delete next[id];
    childrenByKey = next;
  };

  const ensureChildrenLoaded = async (hive: RegistryHive, path: string) => {
    const id = keyId(hive, path);
    const current = childrenByKey[id];
    if (current?.status === 'loading' || current?.status === 'loaded') {
      return;
    }
    childrenByKey = {
      ...childrenByKey,
      [id]: { status: 'loading', children: [] }
    };
    try {
      const children = await invokeTauri<string[]>('registry_list_keys', {
        hive,
        path,
        timeoutMs: 8000
      });
      childrenByKey = {
        ...childrenByKey,
        [id]: { status: 'loaded', children }
      };

      // Auto-collapse leaves once we know they're empty (keeps the tree UI tidy).
      if (children.length === 0 && expanded.has(id)) {
        const next = new Set(expanded);
        next.delete(id);
        expanded = next;
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      childrenByKey = {
        ...childrenByKey,
        [id]: { status: 'error', children: [], error: message }
      };
    }
  };

  const primeInitialTree = async () => {
    if (!connected) return;

    // Always land on HKLM first so the user gets an expanded tree as soon as
    // the registry session is live.
    selectedHive = 'HKLM';
    selectedPath = '';
    pathInput = formatKeyFullPath(selectedHive, selectedPath);
    pathInputError = null;
    actionError = null;
    actionMessage = null;

    const rootId = keyId(selectedHive, '');
    expanded = new Set([...expanded, rootId]);
    invalidateChildren(selectedHive, '');

    // Wait for the DOM/reactivity cycle, then fetch the root children.
    await tick();
    await ensureChildrenLoaded(selectedHive, '');
    await refreshValues(selectedHive, selectedPath);

    // The registry transport can still be settling right after connect. If the
    // first root load did not complete, retry once shortly after.
    if (childrenByKey[rootId]?.status !== 'loaded') {
      window.setTimeout(() => {
        void ensureChildrenLoaded(selectedHive, '');
      }, 250);
    }
  };

  const toggleExpand = (hive: RegistryHive, path: string) => {
    const id = keyId(hive, path);
    const next = new Set(expanded);
    if (next.has(id)) {
      next.delete(id);
      expanded = next;
      return;
    }
    next.add(id);
    expanded = next;
    void ensureChildrenLoaded(hive, path);
  };

  const expandNode = (hive: RegistryHive, path: string) => {
    const id = keyId(hive, path);
    if (expanded.has(id)) return;
    expanded = new Set([...expanded, id]);
    void ensureChildrenLoaded(hive, path);
  };

  const collapseNode = (hive: RegistryHive, path: string) => {
    const id = keyId(hive, path);
    if (!expanded.has(id)) return;
    const next = new Set(expanded);
    next.delete(id);
    expanded = next;
  };

  const refreshValues = async (hive: RegistryHive, path: string) => {
    const seq = (valuesRequestSeq += 1);
    valuesLoading = true;
    valuesError = null;
    try {
      const result = await invokeTauri<RegistryValueEntry[]>('registry_list_values', {
        hive,
        path,
        timeoutMs: 8000
      });
      if (seq !== valuesRequestSeq) return;
      values = result;
    } catch (err) {
      if (seq !== valuesRequestSeq) return;
      const message = err instanceof Error ? err.message : String(err);
      values = [];
      valuesError = message;
    } finally {
      if (seq === valuesRequestSeq) {
        valuesLoading = false;
      }
    }
  };

  const selectKey = (hive: RegistryHive, path: string) => {
    selectedHive = hive;
    selectedPath = normalizeKeyPath(path);
    pathInput = formatKeyFullPath(selectedHive, selectedPath);
    pathInputError = null;
    actionError = null;
    actionMessage = null;

    // Auto-expand the hive root so navigation doesn't feel "empty".
    const rootId = keyId(hive, '');
    if (!expanded.has(rootId)) {
      expanded = new Set([...expanded, rootId]);
      void ensureChildrenLoaded(hive, '');
    }
    void refreshValues(hive, selectedPath);
  };

  const refreshSelected = async (opts?: { refreshChildren?: boolean }) => {
    actionError = null;
    actionMessage = null;
    if (opts?.refreshChildren) {
      invalidateChildren(selectedHive, selectedPath);
      void ensureChildrenLoaded(selectedHive, selectedPath);
    }
    await refreshValues(selectedHive, selectedPath);
  };

  const goToPathInput = () => {
    pathInputError = null;
    const parsed = parseRegistryPathInput(pathInput);
    if (!parsed) {
      pathInputError = 'Invalid registry path (example: Computer\\HKEY_LOCAL_MACHINE\\SOFTWARE)';
      return;
    }
    selectKey(parsed.hive, parsed.path);
  };

  const navigateUp = () => {
    if (!selectedPath) return;
    const { parent } = parseParentPath(selectedPath);
    selectKey(selectedHive, parent);
  };

  const copyCurrentPath = async () => {
    const text = formatKeyFullPath(selectedHive, selectedPath);
    try {
      await navigator.clipboard.writeText(text);
      copyStatus = 'Copied';
    } catch {
      copyStatus = 'Copy failed';
    }
    if (copyStatusTimer) {
      window.clearTimeout(copyStatusTimer);
    }
    copyStatusTimer = window.setTimeout(() => {
      copyStatus = null;
      copyStatusTimer = null;
    }, 1200);
  };

  const createSubKey = async () => {
    if (opInFlight) return;
    actionError = null;
    actionMessage = null;
    if (!connected) {
      actionError = 'Not connected to a remote session';
      return;
    }
    dialogMode = 'create_subkey';
    dialogSubkeyName = '';
    dialogError = null;
    dialogBusy = false;
    dialogOpen = true;
  };

  const deleteSelectedKey = async (recursive: boolean) => {
    if (opInFlight) return;
    actionError = null;
    actionMessage = null;

    if (!connected) {
      actionError = 'Not connected to a remote session';
      return;
    }
    if (!selectedPath) {
      actionError = 'Refusing to delete hive root';
      return;
    }
    dialogMode = recursive ? 'delete_tree' : 'delete_key';
    dialogRecursive = recursive;
    dialogError = null;
    dialogBusy = false;
    dialogOpen = true;
  };

  const deleteValue = async (entry: RegistryValueEntry) => {
    if (opInFlight) return;
    actionError = null;
    actionMessage = null;
    if (!connected) {
      actionError = 'Not connected to a remote session';
      return;
    }
    dialogMode = 'delete_value';
    dialogValueEntry = entry;
    dialogError = null;
    dialogBusy = false;
    dialogOpen = true;
  };

  const submitDialog = async () => {
    if (dialogBusy || opInFlight) return;
    dialogError = null;

    if (!connected) {
      dialogError = 'Not connected to a remote session';
      return;
    }

    dialogBusy = true;
    opInFlight = true;
    actionError = null;
    actionMessage = null;

    try {
      if (dialogMode === 'create_subkey') {
        const subkey = normalizeKeyPath(dialogSubkeyName ?? '');
        if (!subkey) {
          dialogError = 'Subkey name cannot be empty.';
          return;
        }
        const fullPath = selectedPath ? joinKeyPath(selectedPath, subkey) : subkey;
        await invokeTauri('registry_create_key', {
          hive: selectedHive,
          path: fullPath,
          timeoutMs: 15000
        });

        // Refresh the parent node so the new key appears.
        invalidateChildren(selectedHive, selectedPath);
        expanded = new Set([...expanded, keyId(selectedHive, selectedPath)]);
        await ensureChildrenLoaded(selectedHive, selectedPath);
        actionMessage = 'Key created';
        closeDialog();
        return;
      }

      if (dialogMode === 'delete_key' || dialogMode === 'delete_tree') {
        if (!selectedPath) {
          dialogError = 'Refusing to delete hive root';
          return;
        }
        await invokeTauri('registry_delete_key', {
          hive: selectedHive,
          path: selectedPath,
          recursive: dialogRecursive,
          timeoutMs: 20000
        });

        const { parent } = parseParentPath(selectedPath);
        invalidateChildren(selectedHive, parent);
        const nextExpanded = new Set(expanded);
        nextExpanded.delete(keyId(selectedHive, selectedPath));
        expanded = nextExpanded;
        selectKey(selectedHive, parent);
        actionMessage = 'Key deleted';
        closeDialog();
        return;
      }

      // delete_value
      const entry = dialogValueEntry;
      if (!entry) {
        dialogError = 'No registry value selected';
        return;
      }
      await invokeTauri('registry_delete_value', {
        hive: selectedHive,
        path: selectedPath,
        name: entry.name,
        timeoutMs: 15000
      });
      await refreshValues(selectedHive, selectedPath);
      actionMessage = 'Value deleted';
      closeDialog();
    } catch (err) {
      dialogError = err instanceof Error ? err.message : String(err);
    } finally {
      opInFlight = false;
      dialogBusy = false;
    }
  };

  const openCreateValue = () => {
    editorError = null;
    editorMode = 'create';
    editorOriginalName = '';
    editorName = '';
    editorType = 'REG_SZ';
    editorSz = '';
    editorDword = '0';
    editorQword = '0';
    editorMultiSz = '';
    editorBinaryFormat = 'hex';
    editorBinary = '';
    editorOpen = true;
  };

  const openEditValue = async (entry: RegistryValueEntry) => {
    editorError = null;
    actionError = null;
    actionMessage = null;

    if (!connected) {
      actionError = 'Not connected to a remote session';
      return;
    }
    const editableType = isEditableEntry(entry);
    if (!editableType) {
      actionError = `Editing is only supported for: ${BASIC_VALUE_TYPES.join(', ')}`;
      return;
    }

    opInFlight = true;
    try {
      const full = await invokeTauri<RegistryValueEntry | null>('registry_get_value', {
        hive: selectedHive,
        path: selectedPath,
        name: entry.name,
        timeoutMs: 12000
      });
      if (!full) {
        actionError = 'Value not found';
        return;
      }

      editorMode = 'edit';
      editorOriginalName = full.name;
      editorName = full.name;
      editorType = editableType;
      editorSz = '';
      editorDword = '0';
      editorQword = '0';
      editorMultiSz = '';
      editorBinaryFormat = 'hex';
      editorBinary = '';

      const d = full.data;
      if (editableType === 'REG_SZ' && d.type === 'sz') {
        editorSz = d.data;
      } else if (editableType === 'REG_DWORD' && d.type === 'dword') {
        editorDword = String(d.data);
      } else if (editableType === 'REG_QWORD' && d.type === 'qword') {
        editorQword = d.data;
      } else if (editableType === 'REG_MULTI_SZ' && d.type === 'multi_sz') {
        editorMultiSz = d.data.join('\n');
      } else if (editableType === 'REG_BINARY' && d.type === 'binary') {
        const bytes = base64ToBytes(d.dataB64);
        if (bytes && bytes.length <= 8192) {
          editorBinaryFormat = 'hex';
          editorBinary = bytesToHex(bytes);
        } else {
          editorBinaryFormat = 'base64';
          editorBinary = d.dataB64;
        }
      }

      editorOpen = true;
    } catch (err) {
      actionError = err instanceof Error ? err.message : String(err);
    } finally {
      opInFlight = false;
    }
  };

  const closeEditor = () => {
    if (editorSaving) return;
    editorOpen = false;
    editorError = null;
  };

  const buildEditorValueData = (): RegistryValueData | null => {
    if (editorType === 'REG_SZ') {
      return { type: 'sz', data: editorSz };
    }
    if (editorType === 'REG_DWORD') {
      const v = parseDwordInput(editorDword);
      if (v === null) {
        editorError = 'Invalid DWORD (use decimal like 42 or hex like 0x2A)';
        return null;
      }
      return { type: 'dword', data: v };
    }
    if (editorType === 'REG_QWORD') {
      const v = parseQwordInputToDecimalString(editorQword);
      if (v === null) {
        editorError = 'Invalid QWORD (use decimal like 123 or hex like 0x7B)';
        return null;
      }
      return { type: 'qword', data: v };
    }
    if (editorType === 'REG_MULTI_SZ') {
      const raw = editorMultiSz.replace(/\r\n/g, '\n');
      const lines = raw.split('\n');
      // Trim trailing empty lines (common when editing in a textarea).
      while (lines.length > 0 && lines[lines.length - 1] === '') {
        lines.pop();
      }
      return { type: 'multi_sz', data: lines };
    }
    if (editorType === 'REG_BINARY') {
      if (editorBinaryFormat === 'base64') {
        const bytes = base64ToBytes(editorBinary);
        if (bytes === null) {
          editorError = 'Invalid base64';
          return null;
        }
        return { type: 'binary', dataB64: editorBinary.trim().replace(/\s+/g, '') };
      }
      const bytes = hexToBytes(editorBinary);
      if (bytes === null) {
        editorError = 'Invalid hex (use pairs like: DE AD BE EF)';
        return null;
      }
      return { type: 'binary', dataB64: bytesToBase64(bytes) };
    }
    editorError = 'Unsupported type';
    return null;
  };

  const saveEditor = async () => {
    if (editorSaving || opInFlight) return;
    editorError = null;
    actionError = null;
    actionMessage = null;

    if (!connected) {
      editorError = 'Not connected to a remote session';
      return;
    }

    const name = editorMode === 'edit' ? editorOriginalName : editorName;
    const data = buildEditorValueData();
    if (!data) {
      return;
    }

    editorSaving = true;
    try {
      await invokeTauri('registry_set_value', {
        hive: selectedHive,
        path: selectedPath,
        name,
        data,
        timeoutMs: 15000
      });
      editorOpen = false;
      await refreshValues(selectedHive, selectedPath);
      actionMessage = 'Value saved';
    } catch (err) {
      editorError = err instanceof Error ? err.message : String(err);
    } finally {
      editorSaving = false;
    }
  };

  let treeRows: TreeRow[] = [];
  // Explicit dependencies (Svelte is compile-time reactive; function internals don't count).
  $: treeRows = buildTreeRows(childrenByKey, expanded, selectedHive, selectedPath);

  let valueRows: ValueRow[] = [];
  $: valueRows = values.map((entry) => ({
    entry,
    nameLabel: displayValueName(entry.name),
    preview: formatValuePreview(entry)
  }));

  let filteredValueRows: ValueRow[] = [];
  $: {
    const needle = valueFilter.trim().toLowerCase();
    filteredValueRows = !needle
      ? valueRows
      : valueRows.filter((row) => {
          if (row.nameLabel.toLowerCase().includes(needle)) return true;
          if (row.entry.valueType.toLowerCase().includes(needle)) return true;
          if (row.preview.toLowerCase().includes(needle)) return true;
          return false;
        });
  }

  onMount(() => {
    // Best-effort initial load; App.svelte is responsible for bringing up the session.
    if (connected) {
      didAutoInit = true;
      void primeInitialTree();
    }
  });

  $: if (connected && !didAutoInit) {
    didAutoInit = true;
    void primeInitialTree();
  }

  $: if (!connected) {
    didAutoInit = false;
  }
</script>

<div class="registry-layout" aria-label="Remote Registry">
  <div class="registry-toolbar registry-toolbar--path">
    <div class="registry-toolbar-left registry-toolbar-left--path">
      <button class="registry-button" type="button" on:click={navigateUp} disabled={!selectedPath || opInFlight}>
        Up
      </button>
      <button
        class="registry-button"
        type="button"
        on:click={() => void refreshSelected({ refreshChildren: true })}
        disabled={opInFlight}
      >
        Refresh
      </button>
      <button class="registry-button" type="button" on:click={() => void copyCurrentPath()} disabled={opInFlight}>
        Copy Path
      </button>
      {#if copyStatus}
        <span class="registry-toolbar-hint">{copyStatus}</span>
      {/if}
      <input
        class="registry-input registry-path-input"
        bind:value={pathInput}
        placeholder="HKLM\\SOFTWARE\\Microsoft"
        on:keydown={(e) => {
          if (e.key === 'Enter') goToPathInput();
        }}
        disabled={opInFlight}
      />
      <button class="registry-button registry-button--primary" type="button" on:click={goToPathInput} disabled={opInFlight}>
        Go
      </button>
      {#if pathInputError}
        <span class="registry-toolbar-hint registry-toolbar-hint--error">{pathInputError}</span>
      {/if}
    </div>
    <div class="registry-toolbar-right registry-toolbar-right--path">
      {#if error}
        <span class="registry-toolbar-hint registry-toolbar-hint--error">{error}</span>
      {:else if status && !connected}
        <span class="registry-toolbar-hint">{status}</span>
      {/if}
      <button class="registry-button" type="button" on:click={() => void createSubKey()} disabled={!connected || opInFlight}>
        New Key
      </button>
      <button
        class="registry-button"
        type="button"
        on:click={() => void deleteSelectedKey(false)}
        disabled={!connected || !selectedPath || opInFlight}
        title="Delete selected key (non-recursive)"
      >
        Delete Key
      </button>
      <button
        class="registry-button registry-button--danger"
        type="button"
        on:click={() => void deleteSelectedKey(true)}
        disabled={!connected || !selectedPath || opInFlight}
        title="Delete selected key and all subkeys"
      >
        Delete Tree
      </button>
      {#if !connected}
        <button
          class="registry-button registry-button--primary"
          type="button"
          on:click={onConnect}
          disabled={connectInFlight || opInFlight}
        >
          {connectInFlight ? 'Connecting...' : 'Connect'}
        </button>
      {/if}
    </div>
  </div>

  {#if actionError}
    <div class="registry-alert registry-alert--error" role="alert">{actionError}</div>
  {/if}
  {#if actionMessage}
    <div class="registry-alert registry-alert--ok" role="status">{actionMessage}</div>
  {/if}

  <div class="registry-split">
    <section class="registry-pane registry-tree" aria-label="Registry keys tree">
      <div class="registry-pane-header">
        <div class="registry-pane-title">Keys</div>
      </div>
      <div class="registry-pane-body registry-pane-body--tree file-transfer-scrollable">
        {#each treeRows as row (row.id)}
          <div
            class="registry-tree-row"
            class:selected={row.isSelected}
            title={row.fullPath + (row.statusError ? `\n\n${row.statusError}` : '')}
            style={`padding-left: ${row.depth * 14 + 8}px`}
            on:click={() => selectKey(row.hive, row.path)}
            on:dblclick={() => {
              if (row.canToggle) toggleExpand(row.hive, row.path);
            }}
            role="button"
            tabindex="0"
            on:keydown={(e) => {
              if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                selectKey(row.hive, row.path);
              }
              if (e.key === 'ArrowRight') {
                e.preventDefault();
                if (row.canToggle) expandNode(row.hive, row.path);
              }
              if (e.key === 'ArrowLeft') {
                e.preventDefault();
                if (row.canToggle) collapseNode(row.hive, row.path);
              }
            }}
          >
            <button
              class="registry-tree-toggle"
              type="button"
              on:click={(e) => {
                e.stopPropagation();
                toggleExpand(row.hive, row.path);
              }}
              on:keydown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  e.stopPropagation();
                  toggleExpand(row.hive, row.path);
                }
              }}
              disabled={!row.canToggle}
              aria-label={row.isExpanded ? 'Collapse' : 'Expand'}
            >
              {row.canToggle ? (row.isExpanded ? '−' : '+') : ''}
            </button>
            <span class="registry-tree-label">{row.label}</span>
            {#if row.status === 'loading'}
              <span class="registry-tree-status">...</span>
            {:else if row.status === 'error'}
              <span class="registry-tree-status registry-tree-status--error">!</span>
            {/if}
          </div>
        {/each}
      </div>
    </section>

    <section class="registry-pane registry-values" aria-label="Registry values editor">
      <div class="registry-pane-header">
        <div class="registry-pane-title">Values</div>
        <div class="registry-pane-tools">
          <input
            class="registry-input registry-filter-input"
            bind:value={valueFilter}
            placeholder="Filter values..."
            disabled={opInFlight}
          />
          <button class="registry-button" type="button" on:click={openCreateValue} disabled={!connected || opInFlight}>
            New Value
          </button>
        </div>
      </div>
      <div class="registry-pane-subheader">
        <div class="registry-pane-subheader-right">
          {#if valuesLoading}
            <span class="registry-toolbar-hint">Loading...</span>
          {:else if valuesError}
            <span class="registry-toolbar-hint registry-toolbar-hint--error">{valuesError}</span>
          {:else}
            <span class="registry-toolbar-hint">{filteredValueRows.length} value(s)</span>
          {/if}
        </div>
      </div>
      <div class="registry-pane-body registry-pane-body--values file-transfer-scrollable">
        <table class="registry-table" aria-label="Registry values table">
          <thead>
            <tr>
              <th class="registry-col-name">Name</th>
              <th class="registry-col-type">Type</th>
              <th class="registry-col-data">Data</th>
              <th class="registry-col-actions"></th>
            </tr>
          </thead>
          <tbody>
            {#if !connected}
              <tr>
                <td colspan="4" class="registry-table-empty">
                  Connect a registry session to browse the registry.
                </td>
              </tr>
            {:else if !valuesLoading && filteredValueRows.length === 0}
              <tr>
                <td colspan="4" class="registry-table-empty">
                  No values found.
                </td>
              </tr>
            {:else}
              {#each filteredValueRows as row (row.entry.name + ':' + row.entry.valueType)}
                <tr
                  class="registry-table-row"
                  on:dblclick={() => void openEditValue(row.entry)}
                  title="Double-click to edit (basic types only)"
                >
                  <td class="registry-cell-mono">{row.nameLabel}</td>
                  <td>{row.entry.valueType}</td>
                  <td class="registry-cell-data">{row.preview}</td>
                  <td class="registry-cell-actions">
                    <button
                      class="registry-button registry-button--ghost"
                      type="button"
                      on:click={() => void openEditValue(row.entry)}
                      disabled={!isEditableEntry(row.entry) || opInFlight}
                      title={isEditableEntry(row.entry) ? 'Edit' : 'Unsupported type for editing'}
                    >
                      Edit
                    </button>
                    <button
                      class="registry-button registry-button--ghost registry-button--danger"
                      type="button"
                      on:click={() => void deleteValue(row.entry)}
                      disabled={opInFlight}
                      title="Delete"
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              {/each}
            {/if}
          </tbody>
        </table>
      </div>
    </section>
  </div>
</div>

{#if editorOpen}
  <div class="registry-modal-backdrop" role="presentation" on:click|self={closeEditor}>
    <div class="registry-modal" role="dialog" aria-modal="true" aria-label="Registry value editor">
      <div class="registry-modal-header">
        <div class="registry-modal-title">
          {editorMode === 'create' ? 'New Value' : 'Edit Value'}
        </div>
        <button class="registry-button registry-button--ghost" type="button" on:click={closeEditor} disabled={editorSaving}>
          Close
        </button>
      </div>

      <div class="registry-modal-body">
        <div class="registry-form-row">
          <div class="registry-form-label">Key</div>
          <div class="registry-form-static">{formatKeyFullPath(selectedHive, selectedPath)}</div>
        </div>

        <div class="registry-form-row">
          <label class="registry-form-label" for="registry-editor-name">Name</label>
          <input
            class="registry-input"
            id="registry-editor-name"
            bind:value={editorName}
            disabled={editorMode === 'edit'}
            placeholder="(Default when empty)"
          />
        </div>

        <div class="registry-form-row">
          <label class="registry-form-label" for="registry-editor-type">Type</label>
          <select class="registry-select" id="registry-editor-type" bind:value={editorType} disabled={editorMode === 'edit'}>
            {#each BASIC_VALUE_TYPES as t}
              <option value={t}>{t}</option>
            {/each}
          </select>
        </div>

        {#if editorType === 'REG_SZ'}
          <div class="registry-form-row">
            <label class="registry-form-label" for="registry-editor-data">Data</label>
            <input class="registry-input" id="registry-editor-data" bind:value={editorSz} />
          </div>
        {:else if editorType === 'REG_DWORD'}
          <div class="registry-form-row">
            <label class="registry-form-label" for="registry-editor-data">Data</label>
            <input class="registry-input" id="registry-editor-data" bind:value={editorDword} placeholder="42 or 0x2A" />
          </div>
        {:else if editorType === 'REG_QWORD'}
          <div class="registry-form-row">
            <label class="registry-form-label" for="registry-editor-data">Data</label>
            <input class="registry-input" id="registry-editor-data" bind:value={editorQword} placeholder="123 or 0x7B" />
          </div>
        {:else if editorType === 'REG_MULTI_SZ'}
          <div class="registry-form-row registry-form-row--textarea">
            <label class="registry-form-label" for="registry-editor-data">Strings (one per line)</label>
            <textarea class="registry-textarea" id="registry-editor-data" bind:value={editorMultiSz} rows="6"></textarea>
          </div>
        {:else if editorType === 'REG_BINARY'}
          <div class="registry-form-row">
            <label class="registry-form-label" for="registry-editor-binary-format">Format</label>
            <select class="registry-select" id="registry-editor-binary-format" bind:value={editorBinaryFormat}>
              <option value="hex">Hex</option>
              <option value="base64">Base64</option>
            </select>
          </div>
          <div class="registry-form-row registry-form-row--textarea">
            <label class="registry-form-label" for="registry-editor-data">Data</label>
            <textarea
              class="registry-textarea"
              id="registry-editor-data"
              bind:value={editorBinary}
              rows="6"
              placeholder={editorBinaryFormat === 'hex'
                ? 'DE AD BE EF'
                : 'Base64...'}
            ></textarea>
          </div>
        {/if}

        <div class="registry-alert registry-alert--warning">
          Writes are applied immediately to the remote machine.
        </div>

        {#if editorError}
          <div class="registry-alert registry-alert--error" role="alert">{editorError}</div>
        {/if}
      </div>

      <div class="registry-modal-footer">
        <button class="registry-button" type="button" on:click={closeEditor} disabled={editorSaving}>
          Cancel
        </button>
        <button class="registry-button registry-button--primary" type="button" on:click={() => void saveEditor()} disabled={editorSaving}>
          {editorSaving ? 'Saving...' : 'Save'}
        </button>
      </div>
    </div>
  </div>
{/if}

{#if dialogOpen}
  <div class="registry-modal-backdrop" role="presentation" on:click|self={closeDialog}>
    <div class="registry-modal" role="dialog" aria-modal="true" aria-label="Registry confirmation dialog">
      <div class="registry-modal-header">
        <div class="registry-modal-title">
          {#if dialogMode === 'create_subkey'}
            Create Subkey
          {:else if dialogMode === 'delete_value'}
            Delete Value
          {:else}
            {dialogMode === 'delete_tree' ? 'Delete Tree' : 'Delete Key'}
          {/if}
        </div>
        <button
          class="registry-button registry-button--ghost"
          type="button"
          on:click={closeDialog}
          disabled={dialogBusy}
        >
          Close
        </button>
      </div>

      <div class="registry-modal-body">
        {#if dialogMode === 'create_subkey'}
          <div class="registry-form-row">
            <div class="registry-form-label">Parent</div>
            <div class="registry-form-static">{formatKeyFullPath(selectedHive, selectedPath)}</div>
          </div>
          <div class="registry-form-row">
            <label class="registry-form-label" for="registry-subkey-name">Name</label>
            <input
              class="registry-input"
              id="registry-subkey-name"
              bind:value={dialogSubkeyName}
              placeholder="New subkey name"
              disabled={dialogBusy}
              on:keydown={(e) => e.key === 'Enter' && void submitDialog()}
            />
          </div>
          <div class="registry-alert registry-alert--warning">This will modify the remote registry.</div>
        {:else if dialogMode === 'delete_value'}
          {@const entry = dialogValueEntry}
          <div class="registry-form-row">
            <div class="registry-form-label">Key</div>
            <div class="registry-form-static">{formatKeyFullPath(selectedHive, selectedPath)}</div>
          </div>
          <div class="registry-form-row">
            <div class="registry-form-label">Value</div>
            <div class="registry-form-static">{entry ? displayValueName(entry.name) : '—'}</div>
          </div>
          <div class="registry-alert registry-alert--warning">This will modify the remote registry.</div>
        {:else}
          <div class="registry-form-row">
            <div class="registry-form-label">Key</div>
            <div class="registry-form-static">{formatKeyFullPath(selectedHive, selectedPath)}</div>
          </div>
          <div class="registry-alert registry-alert--warning">
            This will permanently remove the key{dialogMode === 'delete_tree' ? ' and all subkeys' : ''}.
          </div>
        {/if}

        {#if dialogError}
          <div class="registry-alert registry-alert--error" role="alert">{dialogError}</div>
        {/if}
      </div>

      <div class="registry-modal-footer">
        <button class="registry-button" type="button" on:click={closeDialog} disabled={dialogBusy}>
          Cancel
        </button>
        <button
          class="registry-button registry-button--primary"
          type="button"
          on:click={() => void submitDialog()}
          disabled={dialogBusy}
        >
          {#if dialogMode === 'create_subkey'}
            {dialogBusy ? 'Creating...' : 'Create'}
          {:else}
            {dialogBusy ? 'Deleting...' : 'Delete'}
          {/if}
        </button>
      </div>
    </div>
  </div>
{/if}
