<script lang="ts">
  import { onDestroy, onMount, tick } from 'svelte';
  import {
    AlertTriangle,
    Bot,
	    CheckCircle2,
	    ChevronLeft,
	    ChevronRight,
	    CircleStop,
    Download,
	    MessageSquarePlus,
    Monitor,
    Pause,
    Play,
    Search,
    Send,
    SkipBack,
    SkipForward,
    Terminal,
    Trash2,
    XCircle,
    UserRound
  } from 'lucide-svelte';
  import { commandCenterApi, installerApi, rmmApi } from '$lib/api';
  import {
    commandCenterMessageAiRunnerEvidence,
    commandCenterMessageAttachments,
    commandCenterMessageCommandApproval
  } from '$lib/commandCenterAttachments';
  import CommandCenterAttachmentImage from '$lib/components/CommandCenterAttachmentImage.svelte';
  import CommandCenterTerminal from '$lib/components/CommandCenterTerminal.svelte';
  import { renderMarkdown } from '$lib/markdown/render';
  import { topbarConfig } from '$lib/topbar';
  import type {
    CommandCenterAiRunnerEvidence,
    CommandCenterAiRunnerJob,
    CommandCenterAiRunnerOutputDelta,
    CommandCenterAiRunnerReplayFrame,
    CommandCenterAiRunnerReplayManifest,
    CommandCenterAiRunnerStreamSnapshot,
    CommandCenterCommandApproval,
    CommandCenterMessageAttachment,
    CommandCenterStoredMessage,
    RmmConnectResponse
  } from '$lib/types';
  import Dialog from '$lib/ui/Dialog.svelte';
  import { detectViewerInstallerPlatform, isDesktopViewerLaunchSupported, launchViewerDeepLink } from '$lib/viewer-launcher';
  import { waitForViewerSessionConnected } from '$lib/viewer-session-status';

  type ChatRole = 'assistant' | 'user';

  type ChatMessage = {
    id: string;
    role: ChatRole;
    content: string;
    createdAt: string;
    attachments?: CommandCenterMessageAttachment[];
    commandApproval?: CommandCenterCommandApproval | null;
    aiRunnerEvidence?: CommandCenterAiRunnerEvidence | null;
    streamedContent?: string;
  };

  type Conversation = {
    id: string;
    title: string;
    createdAt: string;
    updatedAt: string;
    messages: ChatMessage[];
  };

  type TranscriptScrollSnapshot = {
    top: number;
    nearBottom: boolean;
  };

  type ConversationLoadScrollMode = 'bottom' | 'follow';

  type RunnerTerminalMeta = {
    jobId: string;
    turnIndex: number | null;
    terminal: boolean;
    updatedAt: string;
  };

  type RunnerConsoleWaitState = {
    mode: 'checking' | 'waiting';
    badge: string;
    title: string;
    detail: string;
  };

  type TakeOverMode = 'remote_desktop' | 'shell';

  const suggestions = [
    'Find devices with critical alerts and summarize likely causes.',
    'Which endpoints are missing the latest patch baseline?',
    'Draft a safe remediation plan for a user who cannot open Outlook.',
    'Show me stale devices that have not checked in this week.'
  ];
  const WELCOME_MESSAGE_VARIATIONS = [
    'Command Center is ready. Bring me a device, customer, user, alert, patch question, or remediation goal and I will turn it into a clear operator workflow.',
    'Command Center is online. Point me at a device, customer, user, alert, patch concern, or remediation target and I will shape it into an operator-ready workflow.',
    'Ready in Command Center. Give me a device, customer, user, alert, patch question, or remediation goal and I will map the next practical steps.',
    'Talos AI is ready. Tell me what device, customer, user, alert, patch issue, or remediation outcome you want and I will build the workflow.',
    'Command Center is standing by. Share a device, customer, user, alert, patch question, or remediation objective and I will turn it into a clear plan.',
    'I am ready to help. Start with a device, customer, user, alert, patch question, or remediation goal and I will organize the operator path.',
    'Command Center is ready for the next job. Bring me device context, a customer, a user issue, an alert, a patch question, or a remediation goal.',
    'Talos AI is listening. Give me a device, customer, user, alert, patch concern, or remediation target and I will translate it into action.',
    'Ready when you are. Send a device, customer, user, alert, patch question, or remediation goal and I will build a clear workflow.',
    'Command Center is live. Bring an endpoint, customer, user, alert, patch decision, or remediation goal and I will help turn it into an operator workflow.',
    'I am ready for command context. Share a device, customer, user, alert, patch question, or remediation goal and I will lay out the next moves.',
    'Talos AI is warmed up. Point me toward a device, customer, user, alert, patch issue, or remediation goal and I will structure the workflow.',
    'Command Center is prepared. Give me the device, customer, user, alert, patch question, or remediation outcome and I will make the path clear.',
    'Ready to triage. Bring me an endpoint, customer, user issue, alert, patch question, or remediation goal and I will organize the response.',
    'Command Center is ready to work. Start with a device, customer, user, alert, patch question, or remediation goal and I will assemble the workflow.',
    'Talos AI is on deck. Send a device, customer, user, alert, patch concern, or remediation goal and I will turn it into practical steps.',
    'I am ready to coordinate the next workflow. Bring me a device, customer, user, alert, patch question, or remediation target.',
    'Command Center is set. Give me an endpoint, customer, user, alert, patch question, or remediation goal and I will shape the operator plan.',
    'Ready for the next investigation. Share a device, customer, user, alert, patch issue, or remediation goal and I will build the workflow.',
    'Talos AI is ready for direction. Bring me a device, customer, user, alert, patch question, or remediation objective and I will map the response.',
    'Command Center is open. Tell me about a device, customer, user, alert, patch concern, or remediation goal and I will turn it into a usable workflow.'
  ];
  const WELCOME_STREAM_INITIAL_DELAY_MS = 320;
  const WELCOME_STREAM_INTERVAL_MS = 18;
  const MAX_MESSAGE_CHARS = 8000;
  const AI_RUNNER_POLL_INTERVAL_MS = 1000;
  const AI_RUNNER_TERMINAL_BUFFER_CHARS = 256 * 1024;
  const takeOverRunnerStatuses = new Set(['queued', 'approval_pending', 'approval_granted', 'running']);

  const createId = () => `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  const timestamp = (offsetMinutes = 0) => new Date(Date.now() - offsetMinutes * 60_000).toISOString();
  const randomWelcomeMessageContent = () =>
    WELCOME_MESSAGE_VARIATIONS[Math.floor(Math.random() * WELCOME_MESSAGE_VARIATIONS.length)];

  const welcomeMessage = (): ChatMessage => ({
    id: createId(),
    role: 'assistant',
    createdAt: timestamp(),
    content: randomWelcomeMessageContent()
  });

  let activeConversationId: string | null = null;
  let draft = '';
  let sending = false;
  let loadingConversations = true;
  let conversationLoadError = '';
  let activityStatus = '';
  let streamingMessageId: string | null = null;
  let transcriptEl: HTMLDivElement | null = null;
  let composerEl: HTMLTextAreaElement | null = null;
  let isConversationRailCollapsed = false;
  let conversations: Conversation[] = [];
  let draftTitle = 'New command session';
  let draftUpdatedAt = timestamp();
  let draftMessages: ChatMessage[] = [welcomeMessage()];
  let pendingDeleteConversation: Conversation | null = null;
  let deletingConversation = false;
  let deleteConversationError = '';
  let activeAiRunnerJobs: CommandCenterAiRunnerJob[] = [];
  let aiRunnerPollTimer: number | null = null;
  let lastAiRunnerPollStartedAt = 0;
	  let aiRunnerPollInFlight = false;
	  let lastAiRunnerTerminalSignature = '';
	  let commandApprovalActionIds = new Set<string>();
	  let streamAbortController: AbortController | null = null;
	  let streamStopConversationId: string | null = null;
	  let stopRequestedForStream = false;
	  let stopControlBusy = false;
	  let stopControlError = '';
  let takeOverJobIds = new Set<string>();
  let takeOverError: { jobId: string; message: string } | null = null;
  let viewerInstallerDownloading = false;
  let viewerLaunchOverlayOpen = false;
  let viewerLaunchOverlayLabel = 'Viewer';
  let viewerLaunchJobId: string | null = null;
  let viewerLaunchTimedOut = false;
  let cancelViewerLaunchWait: (() => void) | null = null;
	  let animatedWelcomeMessageId: string | null = null;
	  let welcomeStreamTimer: number | null = null;
  let transcriptDownloadJobIds = new Set<string>();
  let runnerEvidenceError: { jobId: string; message: string } | null = null;
  let replayDialogOpen = false;
  let replayLoading = false;
  let replayError = '';
  let replayManifest: CommandCenterAiRunnerReplayManifest | null = null;
  let replayFrameIndex = 0;
  let replayPlaying = false;
  let replayTimer: number | null = null;
  let replayRequestSerial = 0;
  let aiRunnerStreamAbortController: AbortController | null = null;
  let aiRunnerStreamConversationId: string | null = null;
  let aiRunnerStreamJobs: CommandCenterAiRunnerJob[] = [];
  let aiRunnerTerminalByApproval: Record<string, string> = {};
  let aiRunnerTerminalMetaByApproval: Record<string, RunnerTerminalMeta> = {};
  let aiRunnerTerminalTerminalByApproval: Record<string, boolean> = {};
  let aiRunnerTerminalSeenEventIds = new Set<string>();

  $: activeConversation = activeConversationId
    ? conversations.find((conversation) => conversation.id === activeConversationId) ?? null
    : {
        id: '',
        title: draftTitle,
        createdAt: draftUpdatedAt,
        updatedAt: draftUpdatedAt,
        messages: draftMessages
      };
  $: messages = activeConversation?.messages ?? [];
  $: draftTooLong = draft.length > MAX_MESSAGE_CHARS;
  $: canSend = Boolean(draft.trim()) && !draftTooLong && !sending;
  $: conversationCountLabel = conversations.length.toString();
  $: activeStreamingMessageContent = streamingMessageId
    ? messages.find((message) => message.id === streamingMessageId)?.content ?? ''
    : '';
  $: replayCurrentFrame = replayManifest?.frames[replayFrameIndex] ?? null;
  $: replayFrameAttachment = replayAttachmentForFrame(replayCurrentFrame, replayManifest);
  $: runnerConsoleJob = latestRunnerConsoleJob(aiRunnerStreamJobs, activeAiRunnerJobs, aiRunnerTerminalMetaByApproval);
  $: runnerConsoleApproval = runnerConsoleJob ? latestCommandApprovalForJob(runnerConsoleJob, messages) : null;
  $: runnerConsoleWaitState = runnerConsoleJob
    ? runnerConsoleWaitStateForJob(runnerConsoleJob, runnerConsoleApproval)
    : null;
  $: runnerConsoleOutput = runnerConsoleJob
    ? terminalOutputForJob(runnerConsoleJob.id, aiRunnerTerminalMetaByApproval, aiRunnerTerminalByApproval)
    : '';
  $: runnerConsoleTerminalStatus = runnerConsoleTerminalStateLabel(
    runnerConsoleApproval,
    runnerConsoleOutput,
    runnerConsoleWaitState
  );
  $: runnerConsoleTurnStatus = runnerConsoleWaitState?.badge ?? (runnerConsoleApproval ? commandApprovalStatusLabel(runnerConsoleApproval) : 'planning');
  $: runnerConsoleTerminalPlaceholder = runnerConsoleTerminalEmptyText(runnerConsoleApproval);
  $: runnerStatusJobs = activeAiRunnerJobs.filter((job) => job.id !== runnerConsoleJob?.id);
  $: hasActiveShellRunner = knownRunnerJobs(aiRunnerStreamJobs, activeAiRunnerJobs).some(
    (job) => job.jobType === 'shell_goal' && activeRunnerStatuses.has(job.status)
  );
  $: footerShellTranscriptEvidence = hasActiveShellRunner ? null : latestCompletedShellTranscriptEvidence(messages);
  $: hiddenCommandApprovalJobId = runnerConsoleJob?.id ?? footerShellTranscriptEvidence?.jobId ?? null;

  onMount(() => {
    topbarConfig.set({ title: 'Command Center' });
    void loadConversations();
    aiRunnerPollTimer = window.setInterval(() => {
      void loadActiveAiRunnerJobs();
    }, AI_RUNNER_POLL_INTERVAL_MS);
    void scrollTranscript('auto');
  });

  onDestroy(() => {
    topbarConfig.set(null);
    if (aiRunnerPollTimer) {
      window.clearInterval(aiRunnerPollTimer);
      aiRunnerPollTimer = null;
	    }
    clearWelcomeStream();
    closeReplayDialog();
    cancelViewerLaunchWait?.();
	    streamAbortController?.abort();
    aiRunnerStreamAbortController?.abort();
	  });

  const formatTime = (value: string) =>
    new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' }).format(new Date(value));

  const formatDate = (value: string) =>
    new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' }).format(
      new Date(value)
    );

  const compactTitle = (content: string) => {
    const clean = content.replace(/\s+/g, ' ').trim();
    if (!clean) return 'New command session';
    return clean.length > 48 ? `${clean.slice(0, 45).trim()}...` : clean;
  };

  const messageParagraphs = (content: string) =>
    content
      .split(/\n{2,}/)
      .map((block) => block.trim())
      .filter(Boolean);

  const messageDisplayContent = (message: ChatMessage) => message.streamedContent ?? message.content;

  function replayAttachmentForFrame(
    frame: CommandCenterAiRunnerReplayFrame | null,
    manifest: CommandCenterAiRunnerReplayManifest | null
  ): CommandCenterMessageAttachment | null {
    if (!frame || !manifest) return null;
    return {
      id: frame.artifactId,
      type: 'image',
      artifactId: frame.artifactId,
      mimeType: 'image/png',
      name: `desktop-replay-frame-${frame.frameSeq ?? replayFrameIndex + 1}.png`,
      width: frame.width ?? undefined,
      height: frame.height ?? undefined,
      presentation: 'live_frame',
      jobId: manifest.jobId,
      frameSeq: frame.frameSeq ?? undefined,
      cursor: frame.cursor ?? undefined
    };
  }

  const clearReplayTimer = () => {
    if (replayTimer) {
      window.clearTimeout(replayTimer);
      replayTimer = null;
    }
  };

  function scheduleReplayTimer() {
    clearReplayTimer();
    const frameCount = replayManifest?.frames.length ?? 0;
    if (!replayDialogOpen || !replayPlaying || frameCount <= 1) return;
    replayTimer = window.setTimeout(() => {
      advanceReplayFrame(1, true);
    }, replayManifest?.defaultDelayMs ?? 1000);
  }

  function setReplayFrameIndex(index: number) {
    const frameCount = replayManifest?.frames.length ?? 0;
    if (frameCount === 0) return;
    replayFrameIndex = Math.max(0, Math.min(frameCount - 1, index));
    scheduleReplayTimer();
  }

  function advanceReplayFrame(delta: number, fromTimer = false) {
    const frameCount = replayManifest?.frames.length ?? 0;
    if (frameCount === 0) return;
    const nextIndex = replayFrameIndex + delta;
    if (nextIndex >= frameCount) {
      replayFrameIndex = frameCount - 1;
      replayPlaying = false;
      clearReplayTimer();
      return;
    }
    replayFrameIndex = Math.max(0, nextIndex);
    if (fromTimer || replayPlaying) {
      scheduleReplayTimer();
    }
  }

  function toggleReplayPlayback() {
    const frameCount = replayManifest?.frames.length ?? 0;
    if (frameCount === 0) return;
    if (!replayPlaying && replayFrameIndex >= frameCount - 1) {
      replayFrameIndex = 0;
    }
    replayPlaying = !replayPlaying;
    scheduleReplayTimer();
  }

  const updateDraftMessageStreamedContent = (messageId: string, streamedContent: string | null) => {
    draftMessages = draftMessages.map((message) => {
      if (message.id !== messageId) return message;
      if (streamedContent !== null) return { ...message, streamedContent };
      const { streamedContent: _streamedContent, ...rest } = message;
      return rest;
    });
  };

  const clearWelcomeStream = () => {
    if (welcomeStreamTimer) {
      window.clearTimeout(welcomeStreamTimer);
      welcomeStreamTimer = null;
    }
  };

  const finishWelcomeStream = () => {
    clearWelcomeStream();
    if (!animatedWelcomeMessageId) return;
    updateDraftMessageStreamedContent(animatedWelcomeMessageId, null);
    animatedWelcomeMessageId = null;
  };

  async function streamWelcomeMessage() {
    clearWelcomeStream();
    if (activeConversationId || draftMessages.length !== 1 || draftMessages[0]?.role !== 'assistant') {
      animatedWelcomeMessageId = null;
      return;
    }

    const welcomeId = draftMessages[0].id;
    const welcomeContent = draftMessages[0].content;
    animatedWelcomeMessageId = welcomeId;
    updateDraftMessageStreamedContent(welcomeId, '');
    let cursor = 0;

    const typeNextChunk = () => {
      if (activeConversationId || animatedWelcomeMessageId !== welcomeId) {
        clearWelcomeStream();
        return;
      }

      const previousCharacter = welcomeContent[cursor - 1] ?? '';
      const chunkSize = previousCharacter === '.' || previousCharacter === ',' ? 1 : 2;
      cursor = Math.min(welcomeContent.length, cursor + chunkSize);
      updateDraftMessageStreamedContent(welcomeId, welcomeContent.slice(0, cursor));
      void scrollTranscriptIfFollowing(undefined, 'auto');

      if (cursor >= welcomeContent.length) {
        welcomeStreamTimer = window.setTimeout(() => {
          updateDraftMessageStreamedContent(welcomeId, null);
          animatedWelcomeMessageId = null;
          welcomeStreamTimer = null;
        }, 420);
        return;
      }

      const currentCharacter = welcomeContent[cursor - 1] ?? '';
      const delay = currentCharacter === '.' ? 220 : currentCharacter === ',' ? 90 : WELCOME_STREAM_INTERVAL_MS;
      welcomeStreamTimer = window.setTimeout(typeNextChunk, delay);
    };

    await tick();
    welcomeStreamTimer = window.setTimeout(typeNextChunk, WELCOME_STREAM_INITIAL_DELAY_MS);
  }

  const fallbackActivityMessages = [
    'Recalculating splines',
    'Cross-checking inventory signals',
    'Sorting endpoint breadcrumbs',
    'Aligning device context',
    'Tracing software records',
    'Opening a secure desktop view',
    'Waiting for desktop frames',
    'Observing desktop state',
    'Checking relay state',
    'Packaging visual context'
  ];

  const randomActivityMessage = () =>
    fallbackActivityMessages[Math.floor(Math.random() * fallbackActivityMessages.length)];

  const resetDraftConversation = () => {
    draftTitle = 'New command session';
    draftUpdatedAt = timestamp();
    draftMessages = [welcomeMessage()];
  };

  const mapStoredMessage = (message: CommandCenterStoredMessage): ChatMessage => ({
    id: message.id,
    role: message.role,
    content: message.content,
    createdAt: message.createdAt,
    attachments: commandCenterMessageAttachments(message.metadata),
    commandApproval: commandCenterMessageCommandApproval(message.metadata),
    aiRunnerEvidence: commandCenterMessageAiRunnerEvidence(message.metadata)
  });

  const conversationMessagesById = (conversationId: string | null) =>
    conversationId
      ? conversations.find((conversation) => conversation.id === conversationId)?.messages ?? []
      : draftMessages;

  const setConversationMessages = (conversationId: string, nextMessages: ChatMessage[]) => {
    conversations = conversations.map((conversation) =>
      conversation.id === conversationId ? { ...conversation, messages: nextMessages } : conversation
    );
  };

  const updateConversationMessages = (conversationId: string | null, nextMessages: ChatMessage[], title?: string) => {
    if (!conversationId) {
      draftTitle = title ?? draftTitle;
      draftUpdatedAt = timestamp();
      draftMessages = nextMessages;
      return;
    }

    conversations = conversations.map((conversation) =>
      conversation.id === conversationId
        ? {
            ...conversation,
            title: title ?? conversation.title,
            updatedAt: timestamp(),
            messages: nextMessages
          }
        : conversation
    );
  };

  const updateConversationMessageContent = (
    conversationId: string | null,
    messageId: string,
    content: string,
    attachments?: CommandCenterMessageAttachment[]
  ) => {
    if (!conversationId) {
      draftUpdatedAt = timestamp();
      draftMessages = draftMessages.map((message) =>
        message.id === messageId
          ? { ...message, content, ...(attachments !== undefined ? { attachments } : {}) }
          : message
      );
      return;
    }

    conversations = conversations.map((conversation) =>
      conversation.id === conversationId
        ? {
            ...conversation,
            updatedAt: timestamp(),
            messages: conversation.messages.map((message) =>
              message.id === messageId
                ? { ...message, content, ...(attachments !== undefined ? { attachments } : {}) }
                : message
            )
          }
        : conversation
    );
  };

  const appendConversationMessageContent = (conversationId: string | null, messageId: string, delta: string) => {
    if (!conversationId) {
      draftUpdatedAt = timestamp();
      draftMessages = draftMessages.map((message) =>
        message.id === messageId ? { ...message, content: `${message.content}${delta}` } : message
      );
      return;
    }

    conversations = conversations.map((conversation) =>
      conversation.id === conversationId
        ? {
            ...conversation,
            updatedAt: timestamp(),
            messages: conversation.messages.map((message) =>
              message.id === messageId ? { ...message, content: `${message.content}${delta}` } : message
            )
          }
        : conversation
    );
  };

  const toCommandCenterMessages = (items: ChatMessage[]) =>
    items
      .filter((message) => message.role === 'user' || message.role === 'assistant')
      .map((message) => ({ role: message.role, content: message.content }));

  const attachmentRenderKey = (message: ChatMessage, attachment: CommandCenterMessageAttachment) =>
    attachment.presentation === 'live_frame'
      ? `live-frame:${message.id}:${attachment.jobId ?? 'frame'}`
      : attachment.id;

  const takeOverModeForJob = (job: CommandCenterAiRunnerJob | null | undefined): TakeOverMode | null => {
    if (!job) return null;
    if (job.jobType === 'desktop_goal') return 'remote_desktop';
    if (job.jobType === 'shell_goal') return 'shell';
    return null;
  };

  const takeOverLabelForMode = (mode: TakeOverMode) => (mode === 'shell' ? 'System Shell' : 'Remote Desktop');

  const canTakeOverJob = (job: CommandCenterAiRunnerJob | null | undefined) =>
    Boolean(job && takeOverModeForJob(job) && takeOverRunnerStatuses.has(job.status));

  const jobForCommandApproval = (approval: CommandCenterCommandApproval | null | undefined) =>
    approval
      ? knownRunnerJobs(aiRunnerStreamJobs, activeAiRunnerJobs).find((job) => job.id === approval.jobId) ?? null
      : null;

  const commandApprovalControlsDisabled = (approval: CommandCenterCommandApproval) => {
    const job = jobForCommandApproval(approval);
    return commandApprovalActionIds.has(approval.id) || Boolean(job && takeOverJobIds.has(job.id));
  };

  const takeOverControlsDisabled = (
    job: CommandCenterAiRunnerJob,
    approval?: CommandCenterCommandApproval | null
  ) => takeOverJobIds.has(job.id) || Boolean(approval && commandApprovalActionIds.has(approval.id));

  const takeOverErrorForJob = (job: CommandCenterAiRunnerJob | null | undefined) =>
    job && takeOverError?.jobId === job.id ? takeOverError.message : '';

  const saveBlobFile = (filename: string, blob: Blob) => {
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = filename;
    anchor.click();
    URL.revokeObjectURL(url);
  };

  const waitForOverlayPaint = async () => {
    await tick();
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
    await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
  };

  const resolveConnectSessionId = (response: RmmConnectResponse): string | null => {
    if (typeof response.sessionId === 'string' && response.sessionId.trim()) {
      return response.sessionId.trim();
    }
    try {
      const parsed = new URL(response.url);
      const fromUrl = parsed.searchParams.get('session');
      return fromUrl?.trim() ? fromUrl.trim() : null;
    } catch {
      return null;
    }
  };

  const downloadViewerInstaller = async () => {
    try {
      viewerInstallerDownloading = true;
      const result = await installerApi.downloadViewerInstaller(detectViewerInstallerPlatform());
      saveBlobFile(result.filename, result.blob);
    } catch (error) {
      takeOverError = {
        jobId: viewerLaunchJobId ?? takeOverError?.jobId ?? '',
        message: error instanceof Error ? error.message : 'Failed to download Talos Viewer.'
      };
    } finally {
      viewerInstallerDownloading = false;
    }
  };

  const mergeUpdatedRunnerJob = (job: CommandCenterAiRunnerJob) => {
    activeAiRunnerJobs = visibleAiRunnerJobs([
      ...activeAiRunnerJobs.filter((candidate) => candidate.id !== job.id),
      job
    ]);
    aiRunnerStreamJobs = [
      ...aiRunnerStreamJobs.filter((candidate) => candidate.id !== job.id),
      job
    ];
  };

  const markTakeOverJobStopping = (jobId: string) => {
    activeAiRunnerJobs = activeAiRunnerJobs.map((job) =>
      job.id === jobId && activeRunnerStatuses.has(job.status) ? { ...job, status: 'stopping' } : job
    );
    aiRunnerStreamJobs = aiRunnerStreamJobs.map((job) =>
      job.id === jobId && activeRunnerStatuses.has(job.status) ? { ...job, status: 'stopping' } : job
    );
  };

  const runTakeOverJob = async (job: CommandCenterAiRunnerJob) => {
    const mode = takeOverModeForJob(job);
    if (!mode || takeOverJobIds.has(job.id) || !takeOverRunnerStatuses.has(job.status)) return;
    if (!isDesktopViewerLaunchSupported()) {
      takeOverError = {
        jobId: job.id,
        message: 'Talos Viewer launch is supported on Windows and macOS browsers.'
      };
      return;
    }

    takeOverJobIds = new Set([...takeOverJobIds, job.id]);
    takeOverError = null;
    try {
      cancelViewerLaunchWait?.();
      const connectResponse =
        mode === 'shell'
          ? await rmmApi.connectShell(job.agentId)
          : await rmmApi.connectDevice(job.agentId);
      viewerLaunchOverlayLabel = takeOverLabelForMode(mode);
      viewerLaunchJobId = job.id;
      viewerLaunchOverlayOpen = true;
      viewerLaunchTimedOut = false;
      let cancelled = false;
      cancelViewerLaunchWait = () => {
        cancelled = true;
        viewerLaunchOverlayOpen = false;
        viewerLaunchJobId = null;
        viewerLaunchTimedOut = false;
      };
      await waitForOverlayPaint();
      const launchResult = await launchViewerDeepLink(connectResponse.url);
      if (launchResult.status === 'unsupported_platform') {
        throw new Error('Talos Viewer launch is supported on Windows and macOS browsers.');
      }
      const sessionId = resolveConnectSessionId(connectResponse);
      if (!sessionId) {
        throw new Error('Connect response missing session id');
      }
      const status = await waitForViewerSessionConnected(sessionId, {
        agentId: job.agentId,
        onTimeout: () => {
          viewerLaunchTimedOut = true;
        },
        shouldCancel: () => cancelled
      });
      cancelViewerLaunchWait = null;
      if (cancelled) {
        return;
      }
      viewerLaunchOverlayOpen = false;
      viewerLaunchJobId = null;
      viewerLaunchTimedOut = false;
      if (!status?.connected) {
        return;
      }

      markTakeOverJobStopping(job.id);
      let stoppedJob: CommandCenterAiRunnerJob;
      try {
        stoppedJob = await commandCenterApi.stopAiRunnerJob(job.id);
      } catch (error) {
        mergeUpdatedRunnerJob(job);
        takeOverError = {
          jobId: job.id,
          message: error instanceof Error ? error.message : 'Viewer opened, but Command Center could not stop the AI runner.'
        };
        return;
      }

      mergeUpdatedRunnerJob(stoppedJob);
      try {
        await loadActiveAiRunnerJobs(true, true);
        if (activeConversationId) {
          await loadConversationMessages(activeConversationId);
        }
      } catch {
        takeOverError = {
          jobId: job.id,
          message: 'AI runner was stopped, but Command Center could not refresh the latest status.'
        };
      }
    } catch (error) {
      cancelViewerLaunchWait = null;
      viewerLaunchOverlayOpen = false;
      viewerLaunchJobId = null;
      viewerLaunchTimedOut = false;
      takeOverError = {
        jobId: job.id,
        message: error instanceof Error ? error.message : `Could not open ${takeOverLabelForMode(mode)}.`
      };
    } finally {
      const next = new Set(takeOverJobIds);
      next.delete(job.id);
      takeOverJobIds = next;
    }
  };

  async function loadConversationMessages(conversationId: string) {
    const storedMessages = await commandCenterApi.getConversationMessages(conversationId);
    const loadedMessages = storedMessages.map(mapStoredMessage);
    setConversationMessages(conversationId, loadedMessages.length ? loadedMessages : [welcomeMessage()]);
  }

  async function loadConversations(
    preferredConversationId: string | null = activeConversationId,
    messageOverrides: Map<string, ChatMessage[]> = new Map(),
    scrollMode: ConversationLoadScrollMode = 'bottom'
  ) {
    const scrollSnapshot = readTranscriptScroll();
    loadingConversations = true;
    conversationLoadError = '';
    try {
      const currentMessages = new Map(conversations.map((conversation) => [conversation.id, conversation.messages]));
      for (const [id, items] of messageOverrides) {
        currentMessages.set(id, items);
      }
      const summaries = await commandCenterApi.listConversations();
      conversations = summaries.map((summary) => ({
        ...summary,
        messages: currentMessages.get(summary.id) ?? []
      }));

      const nextActiveId =
        preferredConversationId && conversations.some((conversation) => conversation.id === preferredConversationId)
          ? preferredConversationId
          : conversations[0]?.id ?? null;
      activeConversationId = nextActiveId;

      if (nextActiveId) {
        startAiRunnerEventStream(nextActiveId);
        const active = conversations.find((conversation) => conversation.id === nextActiveId);
        if (active && active.messages.length === 0) {
          await loadConversationMessages(nextActiveId);
        }
        await loadActiveAiRunnerJobs(false);
      } else {
        startAiRunnerEventStream(null);
        resetDraftConversation();
        activeAiRunnerJobs = [];
        void streamWelcomeMessage();
      }
      if (scrollMode === 'follow') {
        await scrollTranscriptIfFollowing(scrollSnapshot, 'auto');
      } else {
        await scrollTranscript('auto');
      }
    } catch (error) {
      conversationLoadError = error instanceof Error ? error.message : 'Could not load conversations.';
    } finally {
      loadingConversations = false;
    }
  }

	  const activeRunnerStatuses = new Set(['queued', 'approval_pending', 'approval_granted', 'running', 'stopping']);
	  const visibleTerminalRunnerStatuses = new Set(['approval_denied', 'approval_expired', 'failed', 'stopped']);
  const completedShellRunnerStatuses = new Set(['succeeded', 'failed', 'stopped']);
	  $: hasStoppableControl =
	    Boolean(streamAbortController) || activeAiRunnerJobs.some((job) => activeRunnerStatuses.has(job.status));

	  const visibleAiRunnerJobs = (jobs: CommandCenterAiRunnerJob[]) => {
    const recentCutoff = Date.now() - 10 * 60_000;
    return jobs.filter((job) => {
      if (activeRunnerStatuses.has(job.status)) return true;
      if (visibleTerminalRunnerStatuses.has(job.status)) {
        return new Date(job.updatedAt).getTime() >= recentCutoff;
      }
      return false;
    });
  };

  const runnerJobStatusLabel = (job: CommandCenterAiRunnerJob) => {
    if (job.pendingCommandApproval?.status === 'pending') return 'Waiting for command approval';
    if (job.pendingCommandApproval?.status === 'approved') return 'Command approved; preparing execution';
    if (job.pendingCommandApproval?.status === 'desktop_control_requested') return 'Transferring to desktop control';
    if (job.pendingCommandApproval?.status === 'executing') return 'Executing approved command';
    if (job.pendingCommandApproval?.status === 'policy_blocked') return 'Command blocked by policy';
    if (job.status === 'approval_pending') return 'Waiting for endpoint approval';
    if (job.status === 'approval_granted') return 'Endpoint approved; opening secure session';
    if (job.status === 'running') return job.jobType === 'shell_goal' ? 'Working in system shell' : 'Working on the desktop goal';
    if (job.status === 'queued') return 'Preparing secure runner request';
    if (job.status === 'stopping') return 'Stopping request';
    if (job.status === 'approval_denied') return 'Endpoint denied screen access';
    if (job.status === 'approval_expired') return 'Endpoint approval expired';
    if (job.status === 'failed') return job.error || 'Screen capture failed';
    if (job.status === 'stopped') return 'Request stopped';
    return 'Screen capture complete';
  };

  const runnerJobDetailLabel = (job: CommandCenterAiRunnerJob) => {
    const device = job.deviceLabel || job.agentId;
    const approval = job.pendingCommandApproval ?? job.latestCommandApproval;
    if (approval) {
      return `${device} • turn ${approval.turnIndex + 1}`;
    }
    if (job.status === 'approval_pending' && job.approvalExpiresAt) {
      return `${device} • expires ${formatTime(job.approvalExpiresAt)}`;
    }
    return device;
  };

	  const commandApprovalStatusLabel = (approval: CommandCenterCommandApproval) => {
    if (approval.status === 'pending') return 'Review command';
    if (approval.status === 'approved') return 'Approved';
    if (approval.status === 'desktop_control_requested') return 'Desktop control requested';
    if (approval.status === 'executing') return 'Running';
    if (approval.status === 'executed') return approval.exitCode && approval.exitCode !== 0 ? 'Completed with exit code' : 'Completed';
    if (approval.status === 'denied') return 'Denied';
    if (approval.status === 'expired') return 'Expired';
    if (approval.status === 'policy_blocked') return 'Blocked by policy';
	    return 'Failed';
	  };

  function runnerRecord(value: unknown): Record<string, unknown> | null {
    return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
  }

  function runnerNumber(value: unknown): number | null {
    return typeof value === 'number' && Number.isFinite(value) ? value : null;
  }

  function runnerString(value: unknown): string | null {
    return typeof value === 'string' && value.trim() ? value.trim() : null;
  }

  function formatRunnerDuration(ms: number | null) {
    if (!ms || ms <= 0) return '';
    if (ms < 1_000) return `${Math.round(ms)}ms`;
    const seconds = Math.round(ms / 1_000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    const remainder = seconds % 60;
    return remainder ? `${minutes}m ${remainder}s` : `${minutes}m`;
  }

  function runnerCheckpointDetail(result: Record<string, unknown>) {
    const parts: string[] = [];
    const checkpointCount = runnerNumber(result.checkpointCount);
    const elapsed = formatRunnerDuration(runnerNumber(result.elapsedMs));
    const remaining = formatRunnerDuration(runnerNumber(result.remainingMs));
    if (checkpointCount && checkpointCount > 0) parts.push(`checkpoint ${checkpointCount}`);
    if (elapsed) parts.push(`elapsed ${elapsed}`);
    if (remaining) parts.push(`hard timeout left ${remaining}`);
    return parts.join(' · ');
  }

  function runnerConsoleWaitStateForJob(
    job: CommandCenterAiRunnerJob,
    approval: CommandCenterCommandApproval | null
  ): RunnerConsoleWaitState | null {
    if (approval?.status !== 'executing') return null;
    const result = runnerRecord(job.result);
    const phase = runnerString(result?.phase);
    const resultApprovalId = runnerString(result?.approvalId);
    if (resultApprovalId && resultApprovalId !== approval.id) return null;
    const detail = result ? runnerCheckpointDetail(result) : '';
    if (phase === 'checking_command') {
      return {
        mode: 'checking',
        badge: 'Checking output',
        title: 'AI is reviewing the live terminal output',
        detail: detail || 'Waiting for the model to decide whether to keep waiting or recover'
      };
    }
    const waitMs = runnerNumber(result?.waitMs);
    if (phase === 'executing_command' && waitMs && waitMs > 0) {
      const duration = formatRunnerDuration(waitMs) || 'a little longer';
      return {
        mode: 'waiting',
        badge: `Waiting ${duration}`,
        title: `AI chose to wait ${duration} before the next checkpoint`,
        detail: detail || 'The approved command is still running'
      };
    }
    return null;
  }

	  const isAbortError = (error: unknown) =>
	    error instanceof DOMException
	      ? error.name === 'AbortError'
	      : error instanceof Error && error.name === 'AbortError';

	  const markVisibleAiRunnerJobsStopping = () => {
	    activeAiRunnerJobs = activeAiRunnerJobs.map((job) =>
	      activeRunnerStatuses.has(job.status) ? { ...job, status: 'stopping' } : job
	    );
	  };

  const commandApprovalUsesShellMode = (approval: CommandCenterCommandApproval) =>
    knownRunnerJobs(aiRunnerStreamJobs, activeAiRunnerJobs).some(
      (job) => job.id === approval.jobId && job.jobType === 'shell_goal'
    );

  const isCompletedShellTranscriptEvidence = (evidence: CommandCenterAiRunnerEvidence | null | undefined) =>
    Boolean(
      evidence?.jobType === 'shell_goal' &&
        evidence.shellTranscriptAvailable &&
        completedShellRunnerStatuses.has(evidence.status)
    );

  const isFooterShellTranscriptEvidence = (evidence: CommandCenterAiRunnerEvidence | null | undefined) =>
    Boolean(footerShellTranscriptEvidence && evidence?.jobId === footerShellTranscriptEvidence.jobId);

  function latestCompletedShellTranscriptEvidence(currentMessages: ChatMessage[]) {
    for (const message of [...currentMessages].reverse()) {
      if (isCompletedShellTranscriptEvidence(message.aiRunnerEvidence)) {
        return message.aiRunnerEvidence;
      }
    }
    return null;
  }

  const knownRunnerJobs = (streamJobs: CommandCenterAiRunnerJob[], activeJobs: CommandCenterAiRunnerJob[]) => {
    const byId = new Map<string, CommandCenterAiRunnerJob>();
    for (const job of streamJobs) byId.set(job.id, job);
    for (const job of activeJobs) byId.set(job.id, { ...byId.get(job.id), ...job });
    return [...byId.values()];
  };

  function latestRunnerConsoleJob(
    streamJobs: CommandCenterAiRunnerJob[],
    activeJobs: CommandCenterAiRunnerJob[],
    terminalMetaByApproval: Record<string, RunnerTerminalMeta>
  ): CommandCenterAiRunnerJob | null {
    const outputJobIds = new Set(Object.values(terminalMetaByApproval).map((meta) => meta.jobId));
    const candidates = knownRunnerJobs(streamJobs, activeJobs)
      .filter((job) => job.jobType === 'shell_goal')
      .filter(
        (job) => {
          const hasShellConsoleState =
            outputJobIds.has(job.id) || Boolean(job.pendingCommandApproval || job.latestCommandApproval);
          if (job.status === 'approval_pending' && !hasShellConsoleState) return false;
          if (completedShellRunnerStatuses.has(job.status)) return false;
          return activeRunnerStatuses.has(job.status) || hasShellConsoleState;
        }
      )
      .sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime());
    return candidates[0] ?? null;
  }

  function latestCommandApprovalForJob(
    job: CommandCenterAiRunnerJob,
    currentMessages: ChatMessage[]
  ): CommandCenterCommandApproval | null {
    const approvals = [
      job.pendingCommandApproval,
      job.latestCommandApproval,
      ...currentMessages
        .map((message) => message.commandApproval)
        .filter((approval): approval is CommandCenterCommandApproval => approval?.jobId === job.id)
    ].filter((approval): approval is CommandCenterCommandApproval => Boolean(approval));
    return (
      approvals.sort((a, b) => {
        const turnDelta = b.turnIndex - a.turnIndex;
        if (turnDelta !== 0) return turnDelta;
        return new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime();
      })[0] ?? null
    );
  }

  function terminalOutputForJob(
    jobId: string,
    terminalMetaByApproval: Record<string, RunnerTerminalMeta>,
    terminalByApproval: Record<string, string>
  ) {
    return Object.entries(terminalMetaByApproval)
      .filter(([, meta]) => meta.jobId === jobId)
      .sort(([, a], [, b]) => {
        const turnDelta = (a.turnIndex ?? 0) - (b.turnIndex ?? 0);
        if (turnDelta !== 0) return turnDelta;
        return new Date(a.updatedAt).getTime() - new Date(b.updatedAt).getTime();
      })
      .map(([approvalId]) => (terminalByApproval[approvalId] ?? '').trimEnd())
      .filter(Boolean)
      .join('\n');
  }

  function runnerConsoleStatusLabel(job: CommandCenterAiRunnerJob, approval: CommandCenterCommandApproval | null) {
    const waitState = runnerConsoleWaitStateForJob(job, approval);
    if (waitState?.mode === 'checking') return 'Checking terminal output';
    if (waitState?.mode === 'waiting') return 'Waiting on running command';
    if (approval?.status === 'pending') return 'Awaiting approval';
    if (approval?.status === 'approved') return 'Preparing command';
    if (approval?.status === 'desktop_control_requested') return 'Transferring to desktop control';
    if (approval?.status === 'executing') return 'Streaming terminal';
    if (approval?.status === 'executed' && job.status === 'running') return 'Command complete; planning next step';
    if (approval?.status === 'failed') return 'Command failed';
    if (job.status === 'succeeded') return 'Completed';
    if (job.status === 'failed') return 'Failed';
    if (job.status === 'stopping') return 'Stopping';
    if (job.status === 'stopped') return 'Stopped';
    return 'Working';
  }

  function runnerConsoleTerminalStateLabel(
    approval: CommandCenterCommandApproval | null,
    output: string,
    waitState: RunnerConsoleWaitState | null
  ) {
    if (waitState?.mode === 'checking') return 'checkpoint review';
    if (waitState?.mode === 'waiting') return 'live output · waiting';
    if (output) return approval?.status === 'executed' || approval?.status === 'failed' ? 'recent output' : 'live output';
    if (approval?.status === 'pending') return 'awaiting approval';
    if (approval?.status === 'approved') return 'starting';
    if (approval?.status === 'desktop_control_requested') return 'desktop handoff';
    if (approval?.status === 'executed') return 'no output';
    if (approval?.status === 'failed') return 'error';
    return 'waiting';
  }

  function runnerConsoleTerminalEmptyText(approval: CommandCenterCommandApproval | null) {
    if (approval?.status === 'pending') return 'Waiting for approval before command execution...';
    if (approval?.status === 'approved') return 'Waiting for the remote shell to start the command...';
    if (approval?.status === 'desktop_control_requested') return 'Switching this runner to desktop control...';
    if (approval?.status === 'executed') return 'Command completed with no terminal output.';
    if (approval?.status === 'failed') return approval.error || 'Command failed before terminal output was captured.';
    return 'Waiting for terminal output...';
  }

  const shouldRenderChatMessage = (message: ChatMessage) =>
    !message.commandApproval || message.commandApproval.jobId !== hiddenCommandApprovalJobId;

  function applyAiRunnerOutputDelta(delta: CommandCenterAiRunnerOutputDelta) {
    if (aiRunnerTerminalSeenEventIds.has(delta.eventId)) return;
    aiRunnerTerminalSeenEventIds = new Set([...aiRunnerTerminalSeenEventIds, delta.eventId]);
    if (aiRunnerTerminalSeenEventIds.size > 2_000) {
      const oldest = aiRunnerTerminalSeenEventIds.values().next().value;
      if (oldest) {
        aiRunnerTerminalSeenEventIds.delete(oldest);
        aiRunnerTerminalSeenEventIds = new Set(aiRunnerTerminalSeenEventIds);
      }
    }
    const current = aiRunnerTerminalByApproval[delta.approvalId] ?? '';
    const next = `${current}${delta.text}`.slice(-AI_RUNNER_TERMINAL_BUFFER_CHARS);
    aiRunnerTerminalByApproval = {
      ...aiRunnerTerminalByApproval,
      [delta.approvalId]: next
    };
    aiRunnerTerminalMetaByApproval = {
      ...aiRunnerTerminalMetaByApproval,
      [delta.approvalId]: {
        jobId: delta.jobId,
        turnIndex: delta.turnIndex,
        terminal: delta.terminal || aiRunnerTerminalMetaByApproval[delta.approvalId]?.terminal === true,
        updatedAt: delta.createdAt
      }
    };
    if (delta.terminal) {
      aiRunnerTerminalTerminalByApproval = {
        ...aiRunnerTerminalTerminalByApproval,
        [delta.approvalId]: true
      };
    }
  }

  function applyAiRunnerStreamSnapshot(snapshot: CommandCenterAiRunnerStreamSnapshot) {
    aiRunnerStreamJobs = snapshot.jobs;
    activeAiRunnerJobs = visibleAiRunnerJobs(snapshot.jobs);
    const outputByApproval: Record<string, string> = {};
    const metaByApproval: Record<string, RunnerTerminalMeta> = {};
    const terminalByApproval: Record<string, boolean> = {};
    const seenEventIds = new Set<string>();
    for (const delta of snapshot.output) {
      seenEventIds.add(delta.eventId);
      outputByApproval[delta.approvalId] = `${outputByApproval[delta.approvalId] ?? ''}${delta.text}`.slice(
        -AI_RUNNER_TERMINAL_BUFFER_CHARS
      );
      metaByApproval[delta.approvalId] = {
        jobId: delta.jobId,
        turnIndex: delta.turnIndex,
        terminal: delta.terminal || metaByApproval[delta.approvalId]?.terminal === true,
        updatedAt: delta.createdAt
      };
      if (delta.terminal) {
        terminalByApproval[delta.approvalId] = true;
      }
    }
    aiRunnerTerminalByApproval = outputByApproval;
    aiRunnerTerminalMetaByApproval = metaByApproval;
    aiRunnerTerminalTerminalByApproval = terminalByApproval;
    aiRunnerTerminalSeenEventIds = seenEventIds;
  }

  function stopAiRunnerEventStream() {
    aiRunnerStreamAbortController?.abort();
    aiRunnerStreamAbortController = null;
    aiRunnerStreamConversationId = null;
  }

  function startAiRunnerEventStream(conversationId: string | null) {
    if (!conversationId) {
      stopAiRunnerEventStream();
      aiRunnerTerminalByApproval = {};
      aiRunnerTerminalMetaByApproval = {};
      aiRunnerTerminalTerminalByApproval = {};
      aiRunnerTerminalSeenEventIds = new Set();
      aiRunnerStreamJobs = [];
      return;
    }
    if (aiRunnerStreamConversationId === conversationId && aiRunnerStreamAbortController) return;
    stopAiRunnerEventStream();
    const controller = new AbortController();
    aiRunnerStreamAbortController = controller;
    aiRunnerStreamConversationId = conversationId;
    void commandCenterApi
      .streamAiRunnerConversation(conversationId, {
        onSnapshot: (snapshot) => {
          if (aiRunnerStreamConversationId !== conversationId) return;
          applyAiRunnerStreamSnapshot(snapshot);
        },
        onJobs: (event) => {
          if (aiRunnerStreamConversationId !== conversationId) return;
          aiRunnerStreamJobs = event.jobs;
          activeAiRunnerJobs = visibleAiRunnerJobs(event.jobs);
        },
        onOutput: (event) => {
          if (aiRunnerStreamConversationId !== conversationId) return;
          const scrollSnapshot = readTranscriptScroll();
          applyAiRunnerOutputDelta(event);
          void scrollTranscriptIfFollowing(scrollSnapshot, 'auto');
        },
        signal: controller.signal
      })
      .catch((error) => {
        if (isAbortError(error) || controller.signal.aborted) return;
      })
      .finally(() => {
        if (aiRunnerStreamAbortController === controller) {
          aiRunnerStreamAbortController = null;
        }
      });
  }

	  const runCommandApprovalAction = async (
    approval: CommandCenterCommandApproval,
    action: 'approve' | 'deny' | 'desktop_control'
  ) => {
    if (commandApprovalActionIds.has(approval.id) || approval.status !== 'pending') return;
    commandApprovalActionIds = new Set([...commandApprovalActionIds, approval.id]);
    try {
      if (action === 'approve') {
        await commandCenterApi.approveCommandApproval(approval.id);
      } else if (action === 'desktop_control') {
        await commandCenterApi.denyCommandApprovalAndUseDesktopControl(approval.id);
      } else {
        await commandCenterApi.denyCommandApproval(approval.id);
      }
      await loadActiveAiRunnerJobs(true, true);
      if (activeConversationId) {
        await loadConversationMessages(activeConversationId);
      }
    } finally {
      const next = new Set(commandApprovalActionIds);
      next.delete(approval.id);
      commandApprovalActionIds = next;
    }
  };

  const replayFrameCountLabel = (evidence: CommandCenterAiRunnerEvidence) =>
    evidence.replayFrameCount === 1 ? '1 frame' : `${evidence.replayFrameCount} frames`;

  async function downloadShellTranscript(evidence: CommandCenterAiRunnerEvidence) {
    if (transcriptDownloadJobIds.has(evidence.jobId)) return;
    transcriptDownloadJobIds = new Set([...transcriptDownloadJobIds, evidence.jobId]);
    runnerEvidenceError = null;
    let url = '';
    try {
      const blob = await commandCenterApi.downloadShellTranscript(evidence.jobId);
      url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = `shell-transcript-${evidence.jobId}.txt`;
      document.body.appendChild(link);
      link.click();
      link.remove();
    } catch (error) {
      runnerEvidenceError = {
        jobId: evidence.jobId,
        message: error instanceof Error ? error.message : 'Could not download the shell transcript.'
      };
    } finally {
      if (url) {
        window.setTimeout(() => URL.revokeObjectURL(url), 0);
      }
      const next = new Set(transcriptDownloadJobIds);
      next.delete(evidence.jobId);
      transcriptDownloadJobIds = next;
    }
  }

  async function openDesktopReplay(evidence: CommandCenterAiRunnerEvidence) {
    replayRequestSerial += 1;
    const serial = replayRequestSerial;
    replayDialogOpen = true;
    replayLoading = true;
    replayError = '';
    replayManifest = null;
    replayFrameIndex = 0;
    replayPlaying = false;
    runnerEvidenceError = null;
    clearReplayTimer();
    try {
      const manifest = await commandCenterApi.getAiRunnerReplay(evidence.jobId);
      if (serial !== replayRequestSerial) return;
      replayManifest = manifest;
      replayFrameIndex = 0;
      replayError = manifest.frames.length === 0 ? 'No desktop replay frames were captured for this job.' : '';
    } catch (error) {
      if (serial !== replayRequestSerial) return;
      replayError = error instanceof Error ? error.message : 'Could not load the desktop replay.';
    } finally {
      if (serial === replayRequestSerial) {
        replayLoading = false;
      }
    }
  }

  function closeReplayDialog() {
    replayRequestSerial += 1;
    replayDialogOpen = false;
    replayLoading = false;
    replayPlaying = false;
    replayManifest = null;
    replayFrameIndex = 0;
    replayError = '';
    clearReplayTimer();
  }

  const readTranscriptScroll = (): TranscriptScrollSnapshot | null => {
    if (!transcriptEl) return null;
    return {
      top: transcriptEl.scrollTop,
      nearBottom: transcriptEl.scrollHeight - transcriptEl.scrollTop - transcriptEl.clientHeight <= 96
    };
  };

  async function restoreTranscriptScroll(snapshot: TranscriptScrollSnapshot | null) {
    await tick();
    if (!snapshot || !transcriptEl) return;
    transcriptEl.scrollTo({ top: snapshot.top, behavior: 'auto' });
  }

  async function scrollTranscriptIfFollowing(
    snapshot: TranscriptScrollSnapshot | null = readTranscriptScroll(),
    behavior: ScrollBehavior = 'smooth'
  ) {
    if (!snapshot || snapshot.nearBottom) {
      await scrollTranscript(behavior);
      return;
    }
    await restoreTranscriptScroll(snapshot);
  }

  async function loadActiveAiRunnerJobs(reloadMessagesOnTerminal = true, force = false) {
    if (!activeConversationId) {
      activeAiRunnerJobs = [];
      lastAiRunnerTerminalSignature = '';
      return;
    }
    const conversationId = activeConversationId;
    const now = Date.now();
    if (aiRunnerPollInFlight || (!force && now - lastAiRunnerPollStartedAt < AI_RUNNER_POLL_INTERVAL_MS)) {
      return;
    }
    lastAiRunnerPollStartedAt = now;
    aiRunnerPollInFlight = true;
    try {
      const jobs = await commandCenterApi.listAiRunnerJobs({ conversationId });
      if (activeConversationId !== conversationId) return;
      activeAiRunnerJobs = visibleAiRunnerJobs(jobs);
      const runnerSignature = jobs
        .map(
	          (job) =>
	            `${job.id}:${job.status}:${job.resultMessageId ?? ''}:${job.liveFrameMessageId ?? ''}:${job.updatedAt}:${job.attachments
	              .map((attachment) => attachment.artifactId)
	              .join(',')}:${job.pendingCommandApproval
                ? `${job.pendingCommandApproval.id}:${job.pendingCommandApproval.status}:${job.pendingCommandApproval.updatedAt}`
                : ''}:${job.latestCommandApproval
                ? `${job.latestCommandApproval.id}:${job.latestCommandApproval.status}:${job.latestCommandApproval.updatedAt}`
                : ''}:${job.evidence?.shellTranscriptAvailable ? 'shell' : ''}:${job.evidence?.replayFrameCount ?? 0}`
	        )
        .join('|');
      const hasRunnerMessageChange = jobs.some(
        (job) =>
          Boolean(job.resultMessageId) ||
          Boolean(job.liveFrameMessageId) ||
          job.attachments.length > 0 ||
          Boolean(job.pendingCommandApproval)
      );
      if (
        reloadMessagesOnTerminal &&
        runnerSignature &&
        runnerSignature !== lastAiRunnerTerminalSignature &&
        hasRunnerMessageChange
      ) {
        const scrollSnapshot = readTranscriptScroll();
        const streamingPlaceholder =
          sending && streamingMessageId
            ? conversationMessagesById(conversationId).find((message) => message.id === streamingMessageId) ?? null
            : null;
        await loadConversationMessages(conversationId);
        if (
          streamingPlaceholder &&
          !conversationMessagesById(conversationId).some((message) => message.id === streamingPlaceholder.id)
        ) {
          setConversationMessages(conversationId, [...conversationMessagesById(conversationId), streamingPlaceholder]);
        }
        await scrollTranscriptIfFollowing(scrollSnapshot);
      }
      if (reloadMessagesOnTerminal || !hasRunnerMessageChange) {
        lastAiRunnerTerminalSignature = runnerSignature;
      }
    } catch {
      // Keep the previous visible status rather than flashing an error in the transcript.
    } finally {
      aiRunnerPollInFlight = false;
    }
  }

  async function scrollTranscript(behavior: ScrollBehavior = 'smooth') {
    await tick();
    transcriptEl?.scrollTo({ top: transcriptEl.scrollHeight, behavior });
  }

  function resizeComposer() {
    if (!composerEl) return;
    composerEl.style.height = 'auto';
    composerEl.style.height = `${Math.min(168, Math.max(56, composerEl.scrollHeight))}px`;
  }

  async function handleDraftInput() {
    await tick();
    resizeComposer();
  }

  async function useSuggestion(prompt: string) {
    draft = prompt;
    await tick();
    resizeComposer();
    composerEl?.focus();
  }

  async function selectConversation(id: string) {
    closeReplayDialog();
    activeConversationId = id;
    lastAiRunnerTerminalSignature = '';
    startAiRunnerEventStream(id);
    const conversation = conversations.find((item) => item.id === id);
    if (conversation && conversation.messages.length === 0) {
      await loadConversationMessages(id);
    }
    await loadActiveAiRunnerJobs(false);
    await scrollTranscript('auto');
  }

  function requestDeleteConversation(id: string) {
    if (sending && id === activeConversationId) return;
    pendingDeleteConversation = conversations.find((conversation) => conversation.id === id) ?? null;
    deleteConversationError = '';
  }

  function closeDeleteDialog() {
    if (deletingConversation) return;
    pendingDeleteConversation = null;
    deleteConversationError = '';
  }

  async function confirmDeleteConversation() {
    if (!pendingDeleteConversation || deletingConversation) return;

    const id = pendingDeleteConversation.id;
    if (sending && id === activeConversationId) return;

    const deletedIndex = conversations.findIndex((conversation) => conversation.id === id);
    if (deletedIndex < 0) return;

    const wasActive = id === activeConversationId;

    deletingConversation = true;
    try {
      await commandCenterApi.deleteConversation(id);
      const remaining = conversations.filter((conversation) => conversation.id !== id);
      conversations = remaining;
      if (wasActive) {
        const nextConversation = remaining[Math.min(deletedIndex, remaining.length - 1)];
        activeConversationId = nextConversation?.id ?? null;
        if (activeConversationId) {
          startAiRunnerEventStream(activeConversationId);
          const active = remaining.find((conversation) => conversation.id === activeConversationId);
          if (active && active.messages.length === 0) {
            await loadConversationMessages(activeConversationId);
          }
        } else {
          startAiRunnerEventStream(null);
          resetDraftConversation();
          void streamWelcomeMessage();
        }
      }

      if (wasActive) {
        draft = '';
        await tick();
        resizeComposer();
        await scrollTranscript('auto');
      }
      pendingDeleteConversation = null;
      deleteConversationError = '';
    } catch (error) {
      deleteConversationError = error instanceof Error ? error.message : 'Could not delete this conversation.';
    } finally {
      deletingConversation = false;
    }
  }

		  async function newConversation() {
    closeReplayDialog();
		    activeConversationId = null;
	    activeAiRunnerJobs = [];
	    lastAiRunnerTerminalSignature = '';
    startAiRunnerEventStream(null);
	    stopControlError = '';
	    resetDraftConversation();
	    void streamWelcomeMessage();
	    draft = '';
    await tick();
    resizeComposer();
	    await scrollTranscript('auto');
	  }

	  async function stopCurrentCommandCenterWork() {
	    if (stopControlBusy || !hasStoppableControl) return;
	    const conversationId = streamAbortController ? streamStopConversationId ?? activeConversationId : activeConversationId;
	    const stoppedPlaceholder =
	      conversationId && streamingMessageId
	        ? conversationMessagesById(conversationId).find((message) => message.id === streamingMessageId) ?? null
	        : null;
	    stopControlBusy = true;
	    stopControlError = '';
	    stopRequestedForStream = true;
	    activityStatus = 'Stopping';
	    streamAbortController?.abort();
	    markVisibleAiRunnerJobsStopping();

	    try {
	      if (conversationId) {
	        const stoppedJobs = await commandCenterApi.stopAiRunnerJobsForConversation(conversationId);
	        if (activeConversationId === conversationId) {
	          activeAiRunnerJobs = visibleAiRunnerJobs(stoppedJobs);
	          await loadActiveAiRunnerJobs(true, true);
	          await loadConversationMessages(conversationId);
	          if (
	            stoppedPlaceholder &&
	            !conversationMessagesById(conversationId).some((message) => message.id === stoppedPlaceholder.id)
	          ) {
	            setConversationMessages(conversationId, [
	              ...conversationMessagesById(conversationId),
	              {
	                ...stoppedPlaceholder,
	                content: stoppedPlaceholder.content.trim() ? stoppedPlaceholder.content : 'Stopped.'
	              }
	            ]);
	          }
	        }
	      }
	    } catch (error) {
	      stopControlError = error instanceof Error ? error.message : 'Could not stop the active runner work.';
	    } finally {
	      stopControlBusy = false;
	    }
	  }

	  async function sendMessage() {
    const content = draft.trim();
    if (!content || draftTooLong || sending || !activeConversation) return;
    finishWelcomeStream();

    let conversationId = activeConversationId;
    const userMessage: ChatMessage = {
      id: createId(),
      role: 'user',
      content,
      createdAt: timestamp()
    };

	    draft = '';
	    sending = true;
	    activityStatus = '';
	    stopControlError = '';
	    stopRequestedForStream = false;
	    const abortController = new AbortController();
	    streamAbortController = abortController;
	    streamStopConversationId = conversationId;
	    const assistantMessage: ChatMessage = {
	      id: createId(),
      role: 'assistant',
      content: '',
      createdAt: timestamp(),
      attachments: []
    };
    streamingMessageId = assistantMessage.id;
    const nextMessages = [...conversationMessagesById(conversationId), userMessage];
    updateConversationMessages(conversationId, [...nextMessages, assistantMessage], compactTitle(content));
    await tick();
    resizeComposer();
    await scrollTranscript();

    try {
      const response = await commandCenterApi.streamChat(
        {
          conversationId,
          messages: toCommandCenterMessages(nextMessages)
        },
        {
          onDelta: (event) => {
            if (!event.delta) return;
            const scrollSnapshot = readTranscriptScroll();
            activityStatus = '';
            appendConversationMessageContent(conversationId, assistantMessage.id, event.delta);
            void scrollTranscriptIfFollowing(scrollSnapshot);
          },
          onStatus: (event) => {
            const scrollSnapshot = readTranscriptScroll();
            activityStatus = event.message || randomActivityMessage();
            void loadActiveAiRunnerJobs(true, true);
            void scrollTranscriptIfFollowing(scrollSnapshot);
          },
	          onConversation: (event) => {
	            if (!event.conversationId) return;
	            streamStopConversationId = event.conversationId;
	            if (!conversationId) {
              const promotedMessages = conversationMessagesById(null);
              conversations = [
                {
                  id: event.conversationId,
                  title: compactTitle(content),
                  createdAt: timestamp(),
                  updatedAt: timestamp(),
                  messages: promotedMessages
                },
                ...conversations.filter((conversation) => conversation.id !== event.conversationId)
              ];
              resetDraftConversation();
              activeConversationId = event.conversationId;
              lastAiRunnerTerminalSignature = '';
              startAiRunnerEventStream(event.conversationId);
            }
	            conversationId = event.conversationId;
	            void loadActiveAiRunnerJobs(true, true);
	          },
	          signal: abortController.signal
	        }
	      );
      const currentContent =
        conversationMessagesById(conversationId).find((message) => message.id === assistantMessage.id)?.content ?? '';
      updateConversationMessageContent(
        conversationId,
        assistantMessage.id,
        response.content || currentContent,
        response.attachments ?? []
      );
      const settledMessages = conversationMessagesById(conversationId);
	      if (response.conversationId) {
	        streamStopConversationId = response.conversationId;
	        await loadConversations(response.conversationId, new Map([[response.conversationId, settledMessages]]), 'follow');
	      }
	      await loadActiveAiRunnerJobs(false);
	    } catch (error) {
	      if (stopRequestedForStream && (isAbortError(error) || abortController.signal.aborted)) {
	        const currentContent =
	          conversationMessagesById(conversationId).find((message) => message.id === assistantMessage.id)?.content ?? '';
	        updateConversationMessageContent(
	          conversationId,
	          assistantMessage.id,
	          currentContent.trim() ? currentContent : 'Stopped.'
	        );
	        return;
	      }
	      const fallbackMessage =
	        error instanceof Error
	          ? error.message
          : 'Command Center could not reach the language model. Please try again.';
      const currentContent =
        conversationMessagesById(conversationId).find((message) => message.id === assistantMessage.id)?.content ?? '';
      updateConversationMessageContent(
        conversationId,
        assistantMessage.id,
        currentContent.trim()
          ? `${currentContent}\n\nI could not complete the rest of that request.\n\n${fallbackMessage}`
          : `I could not complete that request.\n\n${fallbackMessage}`
      );
    } finally {
	      const scrollSnapshot = readTranscriptScroll();
	      sending = false;
	      activityStatus = '';
	      streamingMessageId = null;
	      if (streamAbortController === abortController) {
	        streamAbortController = null;
	        streamStopConversationId = null;
	      }
	      stopRequestedForStream = false;
	      void scrollTranscriptIfFollowing(scrollSnapshot);
	    }
  }

  function handleComposerKeydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      void sendMessage();
    }
  }
</script>

<div class="command-center-page" class:rail-collapsed={isConversationRailCollapsed} data-testid="command-center-page">
  <aside
    class="conversation-rail"
    class:collapsed={isConversationRailCollapsed}
    aria-label="Command Center conversations"
  >
    <div class="rail-header">
      <div class="rail-title">
        <span>Conversations</span>
        <small>{conversationCountLabel} saved</small>
      </div>
      <div class="rail-actions">
        {#if isConversationRailCollapsed}
          <button
            type="button"
            on:click={() => (isConversationRailCollapsed = false)}
            aria-label="Expand conversations"
            title="Expand conversations"
          >
            <ChevronLeft class="h-4 w-4" />
          </button>
        {:else}
          <button type="button" on:click={newConversation} aria-label="New conversation" title="New conversation">
            <MessageSquarePlus class="h-4 w-4" />
          </button>
          <button
            type="button"
            on:click={() => (isConversationRailCollapsed = true)}
            aria-label="Minimize conversations"
            title="Minimize conversations"
          >
            <ChevronRight class="h-4 w-4" />
          </button>
        {/if}
      </div>
    </div>

    {#if !isConversationRailCollapsed}
      <div class="conversation-list">
        {#if loadingConversations}
          <div class="rail-empty">Loading conversations...</div>
        {:else if conversationLoadError}
          <div class="rail-empty error">{conversationLoadError}</div>
        {:else if conversations.length === 0}
          <div class="rail-empty">No saved conversations</div>
        {:else}
          {#each conversations as conversation (conversation.id)}
            <div class="conversation-item" class:active={conversation.id === activeConversationId}>
              <button
                type="button"
                class="conversation-select"
                on:click={() => selectConversation(conversation.id)}
              >
                <span>{conversation.title}</span>
                <small>{formatDate(conversation.updatedAt)}</small>
              </button>
              <button
                type="button"
                class="conversation-delete"
                disabled={sending && conversation.id === activeConversationId}
                on:click={() => requestDeleteConversation(conversation.id)}
                aria-label={`Delete conversation ${conversation.title}`}
                title={sending && conversation.id === activeConversationId ? 'Waiting for response' : 'Delete conversation'}
              >
                <Trash2 class="h-3.5 w-3.5" />
              </button>
            </div>
          {/each}
        {/if}
      </div>
    {/if}
  </aside>

  <section class="chat-shell" aria-label="Talos Command Center chat">
    <div class="transcript" bind:this={transcriptEl} data-testid="command-center-transcript">
      {#each messages as message (message.id)}
        {#if (message.id !== streamingMessageId || message.content.trim()) && shouldRenderChatMessage(message)}
          <article class="message-row" class:user={message.role === 'user'}>
            <div class="message-avatar" aria-hidden="true">
              {#if message.role === 'user'}
                <UserRound class="h-4 w-4" />
              {:else}
                <Bot class="h-4 w-4" />
              {/if}
            </div>
            <div class="message-bubble">
              <div class="message-meta">
                <span>{message.role === 'user' ? 'You' : 'Talos AI'}</span>
                <small>{formatTime(message.createdAt)}</small>
              </div>
              {#if message.role === 'assistant'}
                {#if !message.commandApproval}
                  <div class="message-content markdown-content" class:streaming-welcome={message.id === animatedWelcomeMessageId}>
                    {@html renderMarkdown(messageDisplayContent(message))}
                  </div>
	                {/if}
	                {#if message.commandApproval}
                  {@const approvalJob = jobForCommandApproval(message.commandApproval)}
                  {@const approvalTakeOverError = takeOverErrorForJob(approvalJob)}
	                  <div class="command-approval-card" class:blocked={message.commandApproval.status === 'policy_blocked'}>
	                    <div class="command-approval-header">
	                      <div>
	                        <strong>{commandApprovalStatusLabel(message.commandApproval)}</strong>
	                        <small>Turn {message.commandApproval.turnIndex + 1}</small>
	                      </div>
	                      {#if message.commandApproval.status === 'pending'}
	                        <div class="command-action-cluster">
                            {#if approvalJob && canTakeOverJob(approvalJob)}
                              <div class="command-takeover-row">
                                <button
                                  type="button"
                                  class="command-takeover"
                                  disabled={takeOverControlsDisabled(approvalJob, message.commandApproval)}
                                  on:click={() => void runTakeOverJob(approvalJob)}
                                >
                                  {#if takeOverModeForJob(approvalJob) === 'shell'}
                                    <Terminal class="h-4 w-4" />
                                  {:else}
                                    <Monitor class="h-4 w-4" />
                                  {/if}
                                  {takeOverJobIds.has(approvalJob.id) ? 'Taking over' : 'Take over'}
                                </button>
                              </div>
                            {/if}
	                          <div class="command-approval-actions">
	                          <button
	                            type="button"
	                            class="command-deny"
	                            disabled={commandApprovalControlsDisabled(message.commandApproval)}
	                            on:click={() => void runCommandApprovalAction(message.commandApproval!, 'deny')}
	                          >
	                            <XCircle class="h-4 w-4" />
	                            Deny
	                          </button>
	                          {#if commandApprovalUsesShellMode(message.commandApproval)}
	                            <button
	                              type="button"
	                              class="command-desktop-control"
	                              disabled={commandApprovalControlsDisabled(message.commandApproval)}
	                              on:click={() => void runCommandApprovalAction(message.commandApproval!, 'desktop_control')}
	                            >
	                              <Monitor class="h-4 w-4" />
	                              Use desktop control
	                            </button>
	                          {/if}
	                          <button
	                            type="button"
	                            class="command-approve"
	                            disabled={commandApprovalControlsDisabled(message.commandApproval)}
	                            on:click={() => void runCommandApprovalAction(message.commandApproval!, 'approve')}
	                          >
	                            <CheckCircle2 class="h-4 w-4" />
	                            Approve
	                          </button>
                            </div>
                            {#if approvalTakeOverError}
                              <p class="takeover-error">{approvalTakeOverError}</p>
                            {/if}
	                        </div>
	                      {/if}
	                    </div>
                    <pre class="command-approval-command"><code>{message.commandApproval.command}</code></pre>
                    <div class="command-approval-grid">
                      <div>
                        <span>Reasoning</span>
                        <p>{message.commandApproval.explanation}</p>
                      </div>
                      <div>
                        <span>Risk</span>
                        <p>{message.commandApproval.risk}</p>
                      </div>
                    </div>
                    {#if message.commandApproval.policyReason}
                      <p class="command-policy-note">{message.commandApproval.policyReason}</p>
                    {/if}
                    {#if message.commandApproval.notes.length}
                      <ul class="command-notes">
                        {#each message.commandApproval.notes as note}
                          <li>{note}</li>
                        {/each}
                      </ul>
                    {/if}
                    {#if message.commandApproval.output || message.commandApproval.error}
                      <pre class="command-output"><code>{message.commandApproval.error ?? message.commandApproval.output}</code></pre>
                    {/if}
                  </div>
                {/if}
                {#if message.attachments?.length}
                  <div class="message-attachments">
                    {#each message.attachments as attachment (attachmentRenderKey(message, attachment))}
                      {#if attachment.type === 'image'}
                        <CommandCenterAttachmentImage {attachment} />
                      {/if}
                    {/each}
                  </div>
                {/if}
                {#if message.aiRunnerEvidence && (message.aiRunnerEvidence.desktopReplayAvailable || !isFooterShellTranscriptEvidence(message.aiRunnerEvidence))}
                  <div class="runner-evidence-actions">
                    {#if message.aiRunnerEvidence.shellTranscriptAvailable && !isFooterShellTranscriptEvidence(message.aiRunnerEvidence)}
                      <button
                        type="button"
                        class="runner-evidence-button"
                        disabled={transcriptDownloadJobIds.has(message.aiRunnerEvidence.jobId)}
                        on:click={() => downloadShellTranscript(message.aiRunnerEvidence!)}
                      >
                        <Download class="h-4 w-4" />
                        <span>
                          {transcriptDownloadJobIds.has(message.aiRunnerEvidence.jobId)
                            ? 'Downloading transcript'
                            : 'Download shell transcript'}
                        </span>
                      </button>
                    {/if}
                    {#if message.aiRunnerEvidence.desktopReplayAvailable}
                      <button
                        type="button"
                        class="runner-evidence-button"
                        disabled={message.aiRunnerEvidence.replayFrameCount <= 0}
                        on:click={() => openDesktopReplay(message.aiRunnerEvidence!)}
                      >
                        <Play class="h-4 w-4" />
                        <span>Replay desktop</span>
                        <small>{replayFrameCountLabel(message.aiRunnerEvidence)}</small>
                      </button>
                    {/if}
                  </div>
                  {#if runnerEvidenceError?.jobId === message.aiRunnerEvidence.jobId && !isFooterShellTranscriptEvidence(message.aiRunnerEvidence)}
                    <p class="runner-evidence-error">{runnerEvidenceError.message}</p>
                  {/if}
                {/if}
              {:else}
                <div class="message-content">
                  {#each messageParagraphs(message.content) as paragraph}
                    <p>{paragraph}</p>
                  {/each}
                </div>
              {/if}
            </div>
          </article>
        {/if}
      {/each}

      {#if runnerConsoleJob}
        <section class="runner-console" aria-live="polite" aria-label="Shell runner console">
          <div class="runner-console-header">
            <div>
              <strong>{runnerConsoleStatusLabel(runnerConsoleJob, runnerConsoleApproval)}</strong>
              <small>{runnerJobDetailLabel(runnerConsoleJob)}</small>
            </div>
            {#if runnerConsoleApproval}
              <span>Turn {runnerConsoleApproval.turnIndex + 1}</span>
            {/if}
          </div>
          <div class="runner-console-grid">
            <section class="runner-console-terminal" aria-label="Live terminal output">
              <div class="runner-console-panel-header">
                <span>Terminal</span>
                <small>{runnerConsoleTerminalStatus}</small>
              </div>
              <CommandCenterTerminal
                jobId={runnerConsoleJob.id}
                output={runnerConsoleOutput}
                placeholder={runnerConsoleTerminalPlaceholder}
                status={runnerConsoleTerminalStatus}
              />
            </section>
            <aside class="runner-console-turn" aria-label="Current AI command turn">
              <div class="runner-console-panel-header">
                <span>AI turn</span>
                <small>{runnerConsoleTurnStatus}</small>
              </div>
              {#if runnerConsoleApproval}
                {#if runnerConsoleWaitState}
                  <div
                    class="runner-console-wait-state"
                    class:checking={runnerConsoleWaitState.mode === 'checking'}
                    class:waiting={runnerConsoleWaitState.mode === 'waiting'}
                  >
                    <span aria-hidden="true"></span>
                    <div>
                      <small>{runnerConsoleWaitState.badge}</small>
                      <strong>{runnerConsoleWaitState.title}</strong>
                      <em>{runnerConsoleWaitState.detail}</em>
                    </div>
                  </div>
                {/if}
                <pre class="runner-console-command"><code>{runnerConsoleApproval.command}</code></pre>
                <div class="runner-console-copy">
                  <span>Reasoning</span>
                  <p>{runnerConsoleApproval.explanation}</p>
                </div>
                <div class="runner-console-copy">
                  <span>Risk</span>
                  <p>{runnerConsoleApproval.risk}</p>
	                </div>
	                {#if runnerConsoleApproval.status === 'pending'}
	                  <div class="command-action-cluster runner-console-action-cluster">
                      {#if canTakeOverJob(runnerConsoleJob)}
                        <div class="command-takeover-row">
                          <button
                            type="button"
                            class="command-takeover"
                            disabled={takeOverControlsDisabled(runnerConsoleJob, runnerConsoleApproval)}
                            on:click={() => void runTakeOverJob(runnerConsoleJob)}
                          >
                            {#if takeOverModeForJob(runnerConsoleJob) === 'shell'}
                              <Terminal class="h-4 w-4" />
                            {:else}
                              <Monitor class="h-4 w-4" />
                            {/if}
                            {takeOverJobIds.has(runnerConsoleJob.id) ? 'Taking over' : 'Take over'}
                          </button>
                        </div>
                      {/if}
	                    <div class="runner-console-actions">
	                    <button
	                      type="button"
	                      class="command-deny"
	                      disabled={commandApprovalControlsDisabled(runnerConsoleApproval)}
	                      on:click={() => void runCommandApprovalAction(runnerConsoleApproval!, 'deny')}
	                    >
	                      <XCircle class="h-4 w-4" />
	                      Deny
	                    </button>
	                    <button
	                      type="button"
	                      class="command-desktop-control"
	                      disabled={commandApprovalControlsDisabled(runnerConsoleApproval)}
	                      on:click={() => void runCommandApprovalAction(runnerConsoleApproval!, 'desktop_control')}
	                    >
	                      <Monitor class="h-4 w-4" />
	                      Use desktop control
	                    </button>
	                    <button
	                      type="button"
	                      class="command-approve"
	                      disabled={commandApprovalControlsDisabled(runnerConsoleApproval)}
	                      on:click={() => void runCommandApprovalAction(runnerConsoleApproval!, 'approve')}
	                    >
	                      <CheckCircle2 class="h-4 w-4" />
	                      Approve
	                    </button>
                      </div>
                      {#if takeOverErrorForJob(runnerConsoleJob)}
                        <p class="takeover-error">{takeOverErrorForJob(runnerConsoleJob)}</p>
                      {/if}
	                  </div>
	                {/if}
              {:else}
                <p class="runner-console-empty">Waiting for the next command proposal.</p>
              {/if}
            </aside>
          </div>
        </section>
      {/if}

	      {#if runnerStatusJobs.length}
	        <div class="runner-job-stack" aria-live="polite">
	          {#each runnerStatusJobs as job (job.id)}
	            <article class="runner-job-card" class:terminal={!activeRunnerStatuses.has(job.status)}>
	              <div class="runner-job-pulse" aria-hidden="true"><span></span></div>
	              <div>
	                <strong>{runnerJobStatusLabel(job)}</strong>
	                <small>{runnerJobDetailLabel(job)}</small>
	              </div>
                {#if canTakeOverJob(job)}
                  <div class="runner-job-actions">
                    <button
                      type="button"
                      class="command-takeover compact"
                      disabled={takeOverControlsDisabled(job)}
                      on:click={() => void runTakeOverJob(job)}
                    >
                      {#if takeOverModeForJob(job) === 'shell'}
                        <Terminal class="h-4 w-4" />
                      {:else}
                        <Monitor class="h-4 w-4" />
                      {/if}
                      {takeOverJobIds.has(job.id) ? 'Taking over' : 'Take over'}
                    </button>
                    {#if takeOverErrorForJob(job)}
                      <p class="takeover-error">{takeOverErrorForJob(job)}</p>
                    {/if}
                  </div>
                {/if}
	            </article>
	          {/each}
	        </div>
	      {/if}

      {#if footerShellTranscriptEvidence}
        <section class="runner-evidence-actions runner-transcript-footer" aria-label="Completed shell transcript">
          <button
            type="button"
            class="runner-evidence-button"
            disabled={transcriptDownloadJobIds.has(footerShellTranscriptEvidence.jobId)}
            on:click={() => downloadShellTranscript(footerShellTranscriptEvidence!)}
          >
            <Download class="h-4 w-4" />
            <span>
              {transcriptDownloadJobIds.has(footerShellTranscriptEvidence.jobId)
                ? 'Downloading transcript'
                : 'Download shell transcript'}
            </span>
          </button>
        </section>
        {#if runnerEvidenceError?.jobId === footerShellTranscriptEvidence.jobId}
          <p class="runner-evidence-error runner-transcript-footer-error">{runnerEvidenceError.message}</p>
        {/if}
      {/if}

      {#if sending && (activityStatus || !activeStreamingMessageContent)}
        <article class="message-row">
          <div class="message-avatar" aria-hidden="true"><Bot class="h-4 w-4" /></div>
          <div class="message-bubble typing" class:with-status={Boolean(activityStatus)}>
            {#if activityStatus}
              <div class="activity-spinner" aria-hidden="true"><span></span><span></span><span></span></div>
              <span class="activity-text">{activityStatus}</span>
            {:else}
              <div class="typing-dots" aria-label="Talos AI is thinking">
                <span></span><span></span><span></span>
              </div>
            {/if}
          </div>
        </article>
      {/if}
    </div>

    <div class="suggestion-row" aria-label="Prompt suggestions">
      {#each suggestions as suggestion}
        <button type="button" on:click={() => useSuggestion(suggestion)}>
          <Search class="h-3.5 w-3.5" />
          <span>{suggestion}</span>
        </button>
      {/each}
    </div>

	    <form class="composer" on:submit|preventDefault={sendMessage}>
	      <div class="composer-box" class:limit={draftTooLong} class:with-stop={hasStoppableControl}>
	        <textarea
	          bind:this={composerEl}
          bind:value={draft}
          rows="1"
          maxlength={MAX_MESSAGE_CHARS + 1}
          placeholder="Ask Talos about devices, alerts, patches, users, or a remediation goal..."
          on:keydown={handleComposerKeydown}
          on:input={handleDraftInput}
	          aria-label="Command Center message"
	          data-testid="command-center-composer"
	        ></textarea>
	        {#if hasStoppableControl}
	          <button
	            type="button"
	            class="stop-button"
	            disabled={stopControlBusy}
	            on:click={stopCurrentCommandCenterWork}
	            aria-label="Stop active AI runner work"
	            title="Stop active AI runner work"
	          >
	            <CircleStop class="h-4 w-4" />
	            <span>{stopControlBusy ? 'Stopping' : 'Stop'}</span>
	          </button>
	        {/if}
	        <button type="submit" class="send-button" disabled={!canSend} aria-label="Send message">
	          <Send class="h-4 w-4" />
	        </button>
	      </div>
	      {#if stopControlError}
	        <p class="stop-control-error">{stopControlError}</p>
	      {/if}
	    </form>
  </section>
</div>

{#if viewerLaunchOverlayOpen}
  <div class="viewer-launch-overlay">
    <div class="viewer-launch-panel">
      {#if viewerLaunchTimedOut}
        <button
          type="button"
          class="viewer-launch-close"
          aria-label="Close viewer launch overlay"
          on:click={() => cancelViewerLaunchWait?.()}
        >
          ×
        </button>
      {/if}
      <div class="viewer-launch-spinner"></div>
      <div class="viewer-launch-title">Opening {viewerLaunchOverlayLabel}...</div>
      <div class="viewer-launch-copy">
        {#if viewerLaunchTimedOut}
          Talos Viewer still has not confirmed the session. Viewer not installed? Download and install it here. This page will keep waiting and will close automatically if the viewer connects.
        {:else}
          Waiting for Talos Viewer to confirm the session.
        {/if}
      </div>
      {#if viewerLaunchJobId && takeOverError?.jobId === viewerLaunchJobId}
        <p class="viewer-launch-error">{takeOverError.message}</p>
      {/if}
      {#if viewerLaunchTimedOut}
        <div class="viewer-launch-timeout">
          <div class="viewer-launch-timeout-actions">
            <button type="button" class="dialog-button secondary" on:click={() => cancelViewerLaunchWait?.()}>
              Cancel
            </button>
            <button type="button" class="dialog-button" disabled={viewerInstallerDownloading} on:click={downloadViewerInstaller}>
              {viewerInstallerDownloading ? 'Downloading...' : 'Download Viewer'}
            </button>
          </div>
        </div>
      {/if}
    </div>
  </div>
{/if}

<Dialog open={replayDialogOpen} on:close={closeReplayDialog} className="max-w-5xl">
  <div class="replay-dialog-content">
    <div class="replay-dialog-header">
      <div>
        <h2>Desktop replay</h2>
        {#if replayManifest}
          <p>
            {replayManifest.deviceLabel ?? replayManifest.jobId}
            {#if replayManifest.goal}
              <span>•</span>
              {replayManifest.goal}
            {/if}
          </p>
        {:else}
          <p>Loading replay frames</p>
        {/if}
      </div>
      {#if replayManifest?.frames.length}
        <span class="replay-counter">{replayFrameIndex + 1} / {replayManifest.frames.length}</span>
      {/if}
    </div>

    {#if replayLoading}
      <div class="replay-loading">
        <div class="activity-spinner" aria-hidden="true"><span></span><span></span><span></span></div>
        <span>Loading replay</span>
      </div>
    {:else if replayError}
      <p class="replay-error">{replayError}</p>
    {:else if replayManifest && replayCurrentFrame && replayFrameAttachment}
      <div class="replay-frame-stage">
        <CommandCenterAttachmentImage attachment={replayFrameAttachment} />
      </div>
      <div class="replay-narration">
        <strong>
          {replayCurrentFrame.stepIndex !== null ? `Step ${replayCurrentFrame.stepIndex + 1}` : `Frame ${replayFrameIndex + 1}`}
        </strong>
        <p>{replayCurrentFrame.displayText}</p>
      </div>
      <div class="replay-controls">
        <button
          type="button"
          class="replay-control-button"
          disabled={replayFrameIndex <= 0}
          on:click={() => setReplayFrameIndex(replayFrameIndex - 1)}
          aria-label="Previous replay frame"
          title="Previous replay frame"
        >
          <SkipBack class="h-4 w-4" />
        </button>
        <button
          type="button"
          class="replay-control-button primary"
          on:click={toggleReplayPlayback}
          aria-label={replayPlaying ? 'Pause desktop replay' : 'Play desktop replay'}
          title={replayPlaying ? 'Pause desktop replay' : 'Play desktop replay'}
        >
          {#if replayPlaying}
            <Pause class="h-4 w-4" />
          {:else}
            <Play class="h-4 w-4" />
          {/if}
        </button>
        <button
          type="button"
          class="replay-control-button"
          disabled={replayFrameIndex >= replayManifest.frames.length - 1}
          on:click={() => setReplayFrameIndex(replayFrameIndex + 1)}
          aria-label="Next replay frame"
          title="Next replay frame"
        >
          <SkipForward class="h-4 w-4" />
        </button>
      </div>
    {/if}
  </div>
</Dialog>

<Dialog open={Boolean(pendingDeleteConversation)} on:close={closeDeleteDialog} className="delete-dialog">
  <div class="delete-dialog-content">
    <div class="delete-dialog-icon" aria-hidden="true">
      <AlertTriangle class="h-5 w-5" />
    </div>
    <div class="delete-dialog-copy">
      <h2>Delete chat?</h2>
      <p>
        This permanently deletes
        <strong>{pendingDeleteConversation?.title ?? 'this conversation'}</strong>
        and its message history. This cannot be undone.
      </p>
      {#if deleteConversationError}
        <p class="delete-dialog-error">{deleteConversationError}</p>
      {/if}
    </div>
    <div class="delete-dialog-actions">
      <button type="button" class="dialog-button secondary" disabled={deletingConversation} on:click={closeDeleteDialog}>
        Cancel
      </button>
      <button type="button" class="dialog-button danger" disabled={deletingConversation} on:click={confirmDeleteConversation}>
        {deletingConversation ? 'Deleting...' : 'Delete chat'}
      </button>
    </div>
  </div>
</Dialog>

<style>
  .command-center-page {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 260px;
    gap: 16px;
    height: calc(100vh - 104px);
    min-height: 620px;
  }

  .command-center-page.rail-collapsed {
    grid-template-columns: minmax(0, 1fr) 52px;
  }

  .conversation-rail {
    grid-column: 2;
    grid-row: 1;
  }

  .chat-shell {
    grid-column: 1;
    grid-row: 1;
  }

  .conversation-rail,
  .chat-shell {
    min-height: 0;
    border: 1px solid rgba(255, 255, 255, 0.09);
    border-radius: 8px;
    background: rgba(5, 14, 32, 0.72);
    box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.06), 0 16px 48px rgba(0, 0, 0, 0.28);
    backdrop-filter: blur(18px) saturate(150%);
    -webkit-backdrop-filter: blur(18px) saturate(150%);
  }

  .conversation-rail,
  .chat-shell {
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  .rail-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 14px;
    border-bottom: 1px solid rgba(255, 255, 255, 0.08);
  }

  .conversation-rail.collapsed .rail-header {
    justify-content: center;
    padding: 8px;
    border-bottom: 0;
  }

  .rail-title {
    min-width: 0;
  }

  .conversation-rail.collapsed .rail-title {
    display: none;
  }

  .rail-header span {
    display: block;
    color: rgba(238, 247, 255, 0.94);
    font-size: 13px;
    font-weight: 700;
  }

  .rail-header small {
    display: block;
    margin-top: 2px;
    color: rgba(170, 205, 255, 0.5);
    font-size: 11px;
  }

  .rail-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .conversation-rail.collapsed .rail-actions {
    gap: 0;
  }

  .rail-header button,
  .conversation-delete,
  .suggestion-row button,
  .send-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: 1px solid rgba(255, 255, 255, 0.1);
    color: rgba(235, 246, 255, 0.9);
    background: rgba(255, 255, 255, 0.055);
    transition: background 0.16s ease, border-color 0.16s ease, color 0.16s ease;
  }

  .rail-header button {
    width: 34px;
    height: 34px;
    flex: 0 0 auto;
    border-radius: 7px;
  }

  .rail-header button:hover,
  .conversation-delete:not(:disabled):hover,
  .suggestion-row button:hover,
  .send-button:not(:disabled):hover {
    border-color: rgba(125, 180, 255, 0.42);
    background: rgba(59, 130, 246, 0.16);
  }

  .rail-header button:focus-visible,
  .conversation-select:focus-visible,
  .conversation-delete:focus-visible,
  .suggestion-row button:focus-visible,
  .send-button:focus-visible,
  .composer-box textarea:focus-visible {
    outline: 2px solid rgba(125, 180, 255, 0.72);
    outline-offset: 2px;
  }

  .conversation-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 10px;
    overflow: auto;
  }

  .rail-empty {
    padding: 12px;
    color: rgba(170, 205, 255, 0.58);
    font-size: 12px;
    line-height: 1.4;
  }

  .rail-empty.error {
    color: rgba(252, 165, 165, 0.86);
  }

  .conversation-item {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 30px;
    align-items: center;
    gap: 6px;
    width: 100%;
    min-height: 58px;
    padding: 6px 8px 6px 10px;
    border: 1px solid transparent;
    border-radius: 7px;
    background: transparent;
    color: rgba(230, 242, 255, 0.82);
    text-align: left;
  }

  .conversation-item:hover {
    border-color: rgba(125, 180, 255, 0.22);
    background: rgba(125, 180, 255, 0.08);
  }

  .conversation-item.active {
    border-color: rgba(80, 155, 255, 0.38);
    background: rgba(70, 140, 255, 0.14);
  }

  .conversation-select {
    display: grid;
    gap: 4px;
    min-width: 0;
    padding: 4px 0;
    border: 0;
    background: transparent;
    color: inherit;
    text-align: left;
  }

  .conversation-delete {
    width: 30px;
    height: 30px;
    border-radius: 7px;
    opacity: 0.62;
  }

  .conversation-delete:disabled {
    cursor: not-allowed;
    opacity: 0.28;
  }

  .conversation-item:hover .conversation-delete:not(:disabled),
  .conversation-delete:focus-visible {
    opacity: 1;
  }

  .conversation-item span {
    overflow: hidden;
    color: rgba(238, 247, 255, 0.92);
    font-size: 13px;
    font-weight: 650;
    line-height: 1.25;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .conversation-item small {
    overflow: hidden;
    color: rgba(170, 205, 255, 0.48);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .suggestion-row {
    display: flex;
    gap: 8px;
    overflow-x: auto;
    padding: 0 18px 12px;
  }

  .suggestion-row button {
    flex: 0 0 auto;
    gap: 7px;
    max-width: min(360px, 80vw);
    height: 32px;
    padding: 0 11px;
    border-radius: 999px;
    font-size: 12px;
  }

  .suggestion-row button span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .transcript {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: 16px;
    min-height: 0;
    overflow: auto;
    padding: 20px 18px;
  }

  .message-row {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr);
    gap: 10px;
    width: min(100%, 900px);
  }

  .message-row.user {
    align-self: flex-end;
    grid-template-columns: minmax(0, 1fr) 34px;
    max-width: min(84%, 760px);
  }

  .message-row.user .message-avatar {
    grid-column: 2;
    grid-row: 1;
  }

  .message-row.user .message-bubble {
    grid-column: 1;
    grid-row: 1;
    background: rgba(45, 116, 255, 0.22);
    border-color: rgba(125, 180, 255, 0.22);
  }

  .message-avatar {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 34px;
    height: 34px;
    border: 1px solid rgba(255, 255, 255, 0.1);
    border-radius: 50%;
    background: rgba(255, 255, 255, 0.07);
    color: rgba(230, 242, 255, 0.86);
    user-select: none;
    -webkit-user-select: none;
  }

  .message-bubble {
    min-width: 0;
    padding: 12px 13px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 8px;
    background: rgba(255, 255, 255, 0.045);
  }

  .runner-job-stack {
    display: grid;
    gap: 8px;
    width: min(100%, 900px);
  }

  .runner-job-card {
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr) auto;
    gap: 10px;
    align-items: center;
    padding: 11px 13px;
    border: 1px solid rgba(75, 150, 255, 0.25);
    border-radius: 8px;
    background: rgba(45, 116, 255, 0.11);
    color: rgba(235, 246, 255, 0.94);
  }

  .runner-job-card.terminal {
    border-color: rgba(242, 193, 95, 0.28);
    background: rgba(242, 193, 95, 0.08);
  }

  .runner-job-pulse {
    display: grid;
    place-items: center;
    width: 34px;
    height: 34px;
    border-radius: 50%;
    background: rgba(47, 128, 255, 0.14);
  }

  .runner-job-pulse span {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: rgb(94, 171, 255);
    box-shadow: 0 0 0 0 rgba(94, 171, 255, 0.42);
    animation: runner-pulse 1.45s ease-out infinite;
  }

  .runner-job-card.terminal .runner-job-pulse span {
    background: rgb(242, 193, 95);
    animation: none;
    box-shadow: none;
  }

  .runner-job-card strong,
  .runner-job-card small {
    display: block;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .runner-job-card strong {
    font-size: 13px;
    font-weight: 750;
  }

  .runner-job-card small {
    margin-top: 3px;
    color: rgba(178, 207, 245, 0.68);
    font-size: 11px;
  }

  .runner-job-actions {
    display: grid;
    justify-items: end;
    gap: 6px;
  }

  .runner-console {
    display: grid;
    gap: 10px;
    width: min(100%, 900px);
    padding: 12px;
    border: 1px solid rgba(96, 165, 250, 0.24);
    border-radius: 8px;
    background: rgba(5, 13, 25, 0.82);
    color: rgba(230, 242, 255, 0.92);
    user-select: text;
    -webkit-user-select: text;
  }

  .runner-console-header,
  .runner-console-panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
  }

  .runner-console-header strong,
  .runner-console-header small {
    display: block;
  }

  .runner-console-header strong {
    color: rgba(248, 250, 252, 0.96);
    font-size: 13px;
    font-weight: 760;
  }

  .runner-console-header small,
  .runner-console-header > span,
  .runner-console-panel-header small {
    color: rgba(170, 205, 255, 0.58);
    font-size: 11px;
    font-weight: 700;
  }

  .runner-console-grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(360px, 0.95fr);
    gap: 10px;
    min-width: 0;
  }

  .runner-console-terminal,
  .runner-console-turn {
    min-width: 0;
    overflow: hidden;
    border: 1px solid rgba(148, 163, 184, 0.16);
    border-radius: 7px;
    background: rgba(2, 6, 12, 0.7);
  }

  .runner-console-panel-header {
    min-height: 32px;
    padding: 0 10px;
    border-bottom: 1px solid rgba(148, 163, 184, 0.13);
    color: rgba(226, 242, 255, 0.84);
    font-size: 11px;
    font-weight: 760;
    text-transform: uppercase;
  }

  .runner-console-turn {
    display: grid;
    align-content: start;
    gap: 10px;
    padding-bottom: 10px;
  }

  .runner-console-command {
    min-height: 170px;
    max-height: 280px;
    margin: 0 10px;
    overflow: auto;
    padding: 9px;
    border: 1px solid rgba(255, 255, 255, 0.08);
    border-radius: 7px;
    background: rgba(0, 0, 0, 0.26);
    color: rgba(226, 242, 255, 0.94);
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', monospace;
    font-size: 12px;
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .runner-console-wait-state {
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr);
    gap: 9px;
    align-items: start;
    margin: 0 10px;
    padding: 9px;
    border: 1px solid rgba(96, 165, 250, 0.18);
    border-radius: 7px;
    background: rgba(11, 30, 57, 0.62);
  }

  .runner-console-wait-state > span {
    width: 9px;
    height: 9px;
    margin-top: 4px;
    border-radius: 999px;
    color: rgb(96, 165, 250);
    background: rgb(96, 165, 250);
    box-shadow: 0 0 0 0 rgba(96, 165, 250, 0.46);
    animation: runner-wait-pulse 1.3s ease-out infinite;
  }

  .runner-console-wait-state.waiting > span {
    color: rgb(45, 212, 191);
    background: rgb(45, 212, 191);
    box-shadow: 0 0 0 0 rgba(45, 212, 191, 0.42);
  }

  .runner-console-wait-state div {
    display: grid;
    gap: 3px;
    min-width: 0;
  }

  .runner-console-wait-state small {
    color: rgba(147, 197, 253, 0.76);
    font-size: 10px;
    font-weight: 780;
    text-transform: uppercase;
  }

  .runner-console-wait-state strong {
    color: rgba(239, 246, 255, 0.94);
    font-size: 12px;
    font-weight: 760;
    line-height: 1.35;
  }

  .runner-console-wait-state em {
    color: rgba(203, 213, 225, 0.68);
    font-size: 11px;
    font-style: normal;
    line-height: 1.35;
  }

  .runner-console-copy {
    display: grid;
    gap: 4px;
    padding: 0 10px;
  }

  .runner-console-copy span {
    color: rgba(170, 205, 255, 0.58);
    font-size: 11px;
    font-weight: 760;
    text-transform: uppercase;
  }

  .runner-console-copy p,
  .runner-console-empty {
    margin: 0;
    color: rgba(226, 242, 255, 0.84);
    font-size: 12px;
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .runner-console-empty {
    padding: 0 10px;
  }

  .runner-console-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    padding: 0 10px;
  }

  .runner-console-action-cluster .runner-console-actions {
    padding: 0;
  }

  @keyframes runner-wait-pulse {
    0% {
      box-shadow: 0 0 0 0 currentColor;
      opacity: 1;
    }
    100% {
      box-shadow: 0 0 0 8px transparent;
      opacity: 0.76;
    }
  }

  @keyframes runner-pulse {
    0% {
      box-shadow: 0 0 0 0 rgba(94, 171, 255, 0.42);
    }
    100% {
      box-shadow: 0 0 0 13px rgba(94, 171, 255, 0);
    }
  }

  .message-meta {
    display: flex;
    gap: 8px;
    margin-bottom: 7px;
    color: rgba(170, 205, 255, 0.5);
    font-size: 11px;
    user-select: none;
    -webkit-user-select: none;
  }

  .message-meta span {
    color: rgba(220, 238, 255, 0.72);
    font-weight: 650;
  }

  .message-content {
    display: grid;
    gap: 9px;
    user-select: text;
    -webkit-user-select: text;
  }

  .message-content :global(*) {
    user-select: text;
    -webkit-user-select: text;
  }

  .message-attachments {
    display: grid;
    gap: 10px;
    margin-top: 10px;
    user-select: none;
    -webkit-user-select: none;
  }

  .runner-evidence-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
    margin-top: 10px;
  }

  .runner-transcript-footer {
    width: min(100%, 900px);
    margin-top: 0;
  }

  .runner-evidence-button {
    display: inline-flex;
    align-items: center;
    gap: 7px;
    min-height: 34px;
    padding: 0 11px;
    border: 1px solid rgba(96, 165, 250, 0.28);
    border-radius: 7px;
    background: rgba(37, 99, 235, 0.16);
    color: rgba(225, 241, 255, 0.94);
    font-size: 12px;
    font-weight: 750;
  }

  .runner-evidence-button small {
    color: rgba(170, 205, 255, 0.58);
    font-size: 11px;
    font-weight: 650;
  }

  .runner-evidence-button:disabled {
    cursor: wait;
    opacity: 0.62;
  }

  .runner-evidence-button:not(:disabled):hover {
    border-color: rgba(125, 190, 255, 0.48);
    background: rgba(37, 99, 235, 0.26);
  }

  .runner-evidence-error {
    margin: 8px 0 0;
    color: rgba(252, 165, 165, 0.9);
    font-size: 12px;
  }

  .runner-transcript-footer-error {
    width: min(100%, 900px);
  }

  .command-approval-card {
    display: grid;
    gap: 12px;
    margin-top: 12px;
    padding: 12px;
    border: 1px solid rgba(96, 165, 250, 0.24);
    border-radius: 8px;
    background: rgba(6, 18, 38, 0.72);
  }

  .command-approval-card.blocked {
    border-color: rgba(248, 113, 113, 0.28);
    background: rgba(38, 8, 14, 0.48);
  }

  .command-approval-header,
  .command-approval-actions {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .command-action-cluster {
    display: grid;
    justify-items: end;
    gap: 8px;
    min-width: min(100%, 260px);
  }

  .runner-console-action-cluster {
    justify-items: stretch;
    margin: 0 10px;
  }

  .command-takeover-row {
    display: flex;
    justify-content: flex-end;
    width: 100%;
  }

  .command-approval-actions {
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .command-approval-header {
    justify-content: space-between;
    align-items: flex-start;
  }

  .command-approval-header strong {
    display: block;
    color: rgba(248, 250, 252, 0.96);
    font-size: 13px;
    font-weight: 750;
  }

  .command-approval-header small,
  .command-approval-grid span {
    color: rgba(170, 205, 255, 0.58);
    font-size: 11px;
    font-weight: 700;
    text-transform: uppercase;
  }

  .command-approval-actions button,
  .runner-console-actions button,
  .command-takeover {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex: 0 0 auto;
    min-height: 32px;
    padding: 0 10px;
    border-radius: 7px;
    font-size: 12px;
    font-weight: 750;
  }

  .command-takeover {
    border-color: rgba(129, 140, 248, 0.42);
    background: rgba(79, 70, 229, 0.22);
    color: rgba(224, 231, 255, 0.98);
  }

  .command-takeover.compact {
    min-height: 30px;
  }

  .command-takeover:disabled,
  .command-approval-actions button:disabled,
  .runner-console-actions button:disabled {
    cursor: wait;
    opacity: 0.6;
  }

  .takeover-error {
    margin: 0;
    max-width: 320px;
    color: rgba(252, 165, 165, 0.92);
    font-size: 12px;
    line-height: 1.35;
    text-align: right;
  }

  .command-deny {
    border-color: rgba(248, 113, 113, 0.35);
    background: rgba(127, 29, 29, 0.18);
    color: rgba(254, 202, 202, 0.94);
  }

  .command-approve {
    border-color: rgba(74, 222, 128, 0.35);
    background: rgba(22, 101, 52, 0.2);
    color: rgba(187, 247, 208, 0.96);
  }

  .command-desktop-control {
    border-color: rgba(56, 189, 248, 0.36);
    background: rgba(14, 116, 144, 0.18);
    color: rgba(186, 230, 253, 0.96);
  }

  .command-approval-command,
  .command-output {
    margin: 0;
    overflow: auto;
    max-width: 100%;
    padding: 10px;
    border: 1px solid rgba(255, 255, 255, 0.09);
    border-radius: 7px;
    background: rgba(0, 0, 0, 0.26);
    color: rgba(226, 242, 255, 0.96);
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', monospace;
    font-size: 12px;
    line-height: 1.45;
    white-space: pre-wrap;
  }

  .command-approval-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 12px;
  }

  .command-approval-grid p,
  .command-policy-note,
  .command-notes {
    margin: 0;
    color: rgba(226, 238, 250, 0.84);
    font-size: 13px;
    line-height: 1.45;
  }

  .command-policy-note {
    color: rgba(252, 211, 77, 0.9);
  }

  .command-notes {
    display: grid;
    gap: 5px;
    padding-left: 18px;
  }

  .message-content p {
    margin: 0;
    color: rgba(238, 247, 255, 0.88);
    font-size: 14px;
    line-height: 1.55;
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .markdown-content {
    color: rgba(238, 247, 255, 0.88);
    font-size: 14px;
    line-height: 1.55;
    overflow-wrap: anywhere;
  }

  .markdown-content.streaming-welcome:empty::after,
  .markdown-content.streaming-welcome :global(p:last-child)::after {
    display: inline-block;
    width: 7px;
    height: 1.12em;
    margin-left: 2px;
    border-radius: 999px;
    background: rgba(180, 215, 255, 0.82);
    content: '';
    vertical-align: -0.18em;
    animation: welcome-caret 0.9s steps(2, start) infinite;
  }

  @keyframes welcome-caret {
    50% {
      opacity: 0;
    }
  }

  .markdown-content :global(p),
  .markdown-content :global(ul),
  .markdown-content :global(ol),
  .markdown-content :global(pre),
  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3) {
    margin: 0;
  }

  .markdown-content :global(p + p),
  .markdown-content :global(p + ul),
  .markdown-content :global(p + ol),
  .markdown-content :global(ul + p),
  .markdown-content :global(ol + p),
  .markdown-content :global(pre + p) {
    margin-top: 10px;
  }

  .markdown-content :global(ul),
  .markdown-content :global(ol) {
    display: grid;
    gap: 5px;
    padding-left: 20px;
  }

  .markdown-content :global(h1),
  .markdown-content :global(h2),
  .markdown-content :global(h3) {
    color: rgba(246, 250, 255, 0.95);
    font-size: 14px;
    font-weight: 750;
    line-height: 1.35;
  }

  .markdown-content :global(pre) {
    overflow: auto;
    max-width: 100%;
    padding: 10px;
    border: 1px solid rgba(255, 255, 255, 0.09);
    border-radius: 7px;
    background: rgba(0, 0, 0, 0.22);
  }

  .markdown-content :global(code) {
    padding: 2px 5px;
    border-radius: 5px;
    background: rgba(125, 180, 255, 0.1);
    color: rgba(224, 240, 255, 0.96);
    font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, 'Liberation Mono', monospace;
    font-size: 12px;
  }

  .markdown-content :global(pre code) {
    padding: 0;
    background: transparent;
  }

  .markdown-content :global(a) {
    color: rgba(125, 190, 255, 0.95);
    text-decoration: underline;
    text-underline-offset: 3px;
  }

  .typing {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 64px;
    min-height: 42px;
  }

  .typing.with-status {
    width: auto;
    min-width: min(320px, 100%);
    gap: 10px;
  }

  .typing-dots,
  .activity-spinner {
    display: flex;
    align-items: center;
    gap: 5px;
  }

  .typing-dots span,
  .activity-spinner span {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: rgba(170, 205, 255, 0.7);
    animation: pulse-dot 1s infinite ease-in-out;
  }

  .activity-spinner span {
    background: rgba(93, 181, 255, 0.9);
  }

  .typing-dots span:nth-child(2),
  .activity-spinner span:nth-child(2) { animation-delay: 0.15s; }
  .typing-dots span:nth-child(3),
  .activity-spinner span:nth-child(3) { animation-delay: 0.3s; }

  .activity-text {
    color: rgba(220, 238, 255, 0.82);
    font-size: 13px;
    font-weight: 650;
    line-height: 1.35;
  }

  @keyframes pulse-dot {
    0%, 80%, 100% { transform: translateY(0); opacity: 0.45; }
    40% { transform: translateY(-3px); opacity: 1; }
  }

  .composer {
    display: grid;
    gap: 8px;
    padding: 14px 18px 16px;
    border-top: 1px solid rgba(255, 255, 255, 0.08);
  }

  .composer-box {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 40px;
    align-items: end;
	    gap: 8px;
	    padding: 8px;
	    border: 1px solid rgba(255, 255, 255, 0.11);
	    border-radius: 8px;
    background: rgba(0, 0, 0, 0.18);
  }

  .composer-box.with-stop {
    grid-template-columns: minmax(0, 1fr) auto 40px;
  }

  .composer-box.limit {
    border-color: rgba(248, 113, 113, 0.44);
    box-shadow: 0 0 0 1px rgba(248, 113, 113, 0.16);
  }

  .send-button {
    width: 36px;
    height: 36px;
    border-radius: 7px;
  }

  .stop-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 6px;
    min-width: 86px;
    height: 36px;
    padding: 0 11px;
    border-color: rgba(248, 113, 113, 0.48);
    border-radius: 7px;
    background: rgba(127, 29, 29, 0.26);
    color: rgba(254, 202, 202, 0.96);
    font-size: 12px;
    font-weight: 750;
  }

  .stop-button:disabled {
    cursor: wait;
    opacity: 0.72;
  }

  .send-button:disabled {
    cursor: not-allowed;
	    opacity: 0.42;
	  }

  .send-button {
    border-color: rgba(77, 145, 255, 0.42);
    background: rgba(36, 105, 240, 0.22);
  }

  .composer-box textarea {
    min-height: 36px;
    max-height: 168px;
    resize: none;
    border: 0;
    outline: 0;
    background: transparent;
    color: rgba(238, 247, 255, 0.94);
    font-size: 14px;
    line-height: 1.45;
    padding: 8px 2px;
  }

  .composer-box textarea::placeholder {
    color: rgba(170, 205, 255, 0.42);
  }

  .stop-control-error {
    margin: 0;
    color: rgba(254, 202, 202, 0.92);
    font-size: 12px;
  }

  .delete-dialog-content {
    display: grid;
    grid-template-columns: 42px minmax(0, 1fr);
    gap: 14px;
    padding-right: 24px;
  }

  .replay-dialog-content {
    display: grid;
    gap: 14px;
    padding-right: 24px;
    min-width: min(860px, calc(100vw - 72px));
  }

  .replay-dialog-header {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 16px;
    padding-right: 24px;
  }

  .replay-dialog-header h2 {
    margin: 0;
    color: rgba(248, 250, 252, 0.96);
    font-size: 18px;
    font-weight: 760;
    line-height: 1.25;
  }

  .replay-dialog-header p {
    margin: 5px 0 0;
    color: rgba(220, 238, 255, 0.68);
    font-size: 12px;
    line-height: 1.45;
  }

  .replay-dialog-header span {
    color: rgba(170, 205, 255, 0.46);
  }

  .replay-counter {
    flex: 0 0 auto;
    padding: 4px 8px;
    border: 1px solid rgba(125, 180, 255, 0.2);
    border-radius: 999px;
    color: rgba(220, 238, 255, 0.78) !important;
    font-size: 11px;
    font-weight: 750;
  }

  .replay-loading,
  .replay-error {
    display: flex;
    align-items: center;
    gap: 8px;
    min-height: 180px;
    color: rgba(220, 238, 255, 0.78);
    font-size: 13px;
  }

  .replay-error {
    margin: 0;
    color: rgba(252, 165, 165, 0.92);
  }

  .replay-frame-stage {
    display: grid;
    place-items: center;
    max-height: min(62vh, 620px);
    overflow: hidden;
  }

  .replay-frame-stage :global(.attachment-image) {
    width: 100%;
    margin: 0;
  }

  .replay-frame-stage :global(.attachment-frame) {
    width: 100%;
    max-width: none;
  }

  .replay-narration {
    display: grid;
    gap: 5px;
    padding: 10px 12px;
    border: 1px solid rgba(125, 180, 255, 0.18);
    border-radius: 8px;
    background: rgba(3, 9, 25, 0.38);
  }

  .replay-narration strong {
    color: rgba(248, 250, 252, 0.94);
    font-size: 12px;
    font-weight: 760;
  }

  .replay-narration p {
    margin: 0;
    color: rgba(220, 238, 255, 0.78);
    font-size: 13px;
    line-height: 1.45;
  }

  .replay-controls {
    display: flex;
    justify-content: center;
    gap: 8px;
  }

  .replay-control-button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 38px;
    height: 34px;
    border: 1px solid rgba(125, 180, 255, 0.24);
    border-radius: 7px;
    background: rgba(255, 255, 255, 0.07);
    color: rgba(225, 241, 255, 0.92);
  }

  .replay-control-button.primary {
    border-color: rgba(96, 165, 250, 0.38);
    background: rgba(37, 99, 235, 0.25);
  }

  .replay-control-button:not(:disabled):hover {
    border-color: rgba(125, 190, 255, 0.48);
    background: rgba(37, 99, 235, 0.24);
  }

  .replay-control-button:disabled {
    cursor: not-allowed;
    opacity: 0.42;
  }

  .delete-dialog-icon {
    display: flex;
    align-items: center;
    justify-content: center;
    width: 42px;
    height: 42px;
    border: 1px solid rgba(248, 113, 113, 0.32);
    border-radius: 8px;
    background: rgba(127, 29, 29, 0.22);
    color: rgba(252, 165, 165, 0.95);
  }

  .delete-dialog-copy {
    display: grid;
    gap: 8px;
    min-width: 0;
  }

  .delete-dialog-copy h2 {
    margin: 0;
    color: rgba(248, 250, 252, 0.96);
    font-size: 18px;
    font-weight: 750;
    line-height: 1.25;
  }

  .delete-dialog-copy p {
    margin: 0;
    color: rgba(220, 238, 255, 0.78);
    font-size: 14px;
    line-height: 1.5;
  }

  .delete-dialog-copy strong {
    color: rgba(248, 250, 252, 0.94);
    font-weight: 750;
  }

  .delete-dialog-error {
    color: rgba(252, 165, 165, 0.9) !important;
  }

  .delete-dialog-actions {
    grid-column: 2;
    display: flex;
    justify-content: flex-end;
    gap: 10px;
    margin-top: 6px;
  }

  .dialog-button {
    min-width: 104px;
    height: 38px;
    border: 1px solid rgba(255, 255, 255, 0.12);
    border-radius: 7px;
    color: rgba(238, 247, 255, 0.92);
    font-size: 13px;
    font-weight: 700;
    transition: background 0.16s ease, border-color 0.16s ease, opacity 0.16s ease;
  }

  .dialog-button.secondary {
    background: rgba(255, 255, 255, 0.07);
  }

  .dialog-button.danger {
    border-color: rgba(248, 113, 113, 0.38);
    background: rgba(185, 28, 28, 0.34);
  }

  .dialog-button:not(:disabled):hover {
    border-color: rgba(255, 255, 255, 0.24);
    background: rgba(255, 255, 255, 0.11);
  }

  .dialog-button.danger:not(:disabled):hover {
    border-color: rgba(248, 113, 113, 0.54);
    background: rgba(220, 38, 38, 0.42);
  }

  .dialog-button:disabled {
    cursor: not-allowed;
    opacity: 0.52;
  }

  .viewer-launch-overlay {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 20px;
    background: rgba(2, 6, 23, 0.62);
    backdrop-filter: blur(10px);
    -webkit-backdrop-filter: blur(10px);
  }

  .viewer-launch-panel {
    position: relative;
    display: grid;
    justify-items: center;
    gap: 12px;
    width: min(100%, 380px);
    padding: 24px;
    border: 1px solid rgba(96, 165, 250, 0.24);
    border-radius: 8px;
    background: rgba(5, 14, 32, 0.94);
    box-shadow: 0 24px 70px rgba(0, 0, 0, 0.42);
    text-align: center;
  }

  .viewer-launch-spinner {
    width: 34px;
    height: 34px;
    border: 3px solid rgba(96, 165, 250, 0.22);
    border-top-color: rgba(147, 197, 253, 0.98);
    border-radius: 999px;
    animation: viewer-launch-spin 0.9s linear infinite;
  }

  .viewer-launch-title {
    color: rgba(248, 250, 252, 0.96);
    font-size: 15px;
    font-weight: 760;
  }

  .viewer-launch-copy {
    color: rgba(203, 213, 225, 0.82);
    font-size: 13px;
    line-height: 1.45;
  }

  .viewer-launch-error {
    margin: 0;
    color: rgba(252, 165, 165, 0.94);
    font-size: 12px;
    line-height: 1.35;
  }

  .viewer-launch-timeout {
    width: 100%;
  }

  .viewer-launch-timeout-actions {
    display: flex;
    justify-content: center;
    gap: 10px;
    margin-top: 4px;
  }

  .viewer-launch-close {
    position: absolute;
    top: 10px;
    right: 10px;
    display: grid;
    place-items: center;
    width: 28px;
    height: 28px;
    border-radius: 999px;
    background: rgba(255, 255, 255, 0.08);
    color: rgba(226, 232, 240, 0.86);
    font-size: 20px;
    line-height: 1;
  }

  .viewer-launch-close:hover {
    background: rgba(255, 255, 255, 0.14);
  }

  @keyframes viewer-launch-spin {
    to {
      transform: rotate(360deg);
    }
  }

  :global(html.light) .conversation-rail,
  :global(html.light) .chat-shell {
    border-color: rgba(100, 158, 220, 0.24);
    background: rgba(255, 255, 255, 0.62);
    box-shadow: 0 16px 42px rgba(0, 38, 120, 0.08);
  }

  :global(html.light) .rail-header,
  :global(html.light) .composer {
    border-color: rgba(100, 158, 220, 0.16);
  }

  :global(html.light) .rail-header span,
  :global(html.light) .conversation-item span {
    color: rgba(10, 30, 95, 0.9);
  }

  :global(html.light) .rail-header small,
  :global(html.light) .conversation-item small,
  :global(html.light) .message-meta {
    color: rgba(10, 42, 108, 0.55);
  }

  :global(html.light) .rail-header button,
  :global(html.light) .conversation-delete,
  :global(html.light) .suggestion-row button,
  :global(html.light) .send-button,
  :global(html.light) .conversation-item,
  :global(html.light) .message-avatar,
  :global(html.light) .message-bubble,
  :global(html.light) .composer-box {
    border-color: rgba(100, 158, 220, 0.24);
    background: rgba(255, 255, 255, 0.42);
    color: rgba(10, 42, 108, 0.75);
  }

  :global(html.light) .conversation-item.active {
    border-color: rgba(50, 120, 255, 0.26);
    background: rgba(50, 120, 255, 0.12);
    color: rgba(10, 30, 95, 0.92);
  }

  :global(html.light) .message-row.user .message-bubble {
    background: rgba(50, 120, 255, 0.14);
  }

  :global(html.light) .message-content p,
  :global(html.light) .markdown-content,
  :global(html.light) .markdown-content :global(p),
  :global(html.light) .markdown-content :global(ul),
  :global(html.light) .markdown-content :global(ol),
  :global(html.light) .composer-box textarea {
    color: rgba(10, 30, 95, 0.88);
  }

  :global(html.light) .markdown-content :global(h1),
  :global(html.light) .markdown-content :global(h2),
  :global(html.light) .markdown-content :global(h3) {
    color: rgba(8, 28, 88, 0.94);
  }

  :global(html.light) .markdown-content.streaming-welcome:empty::after,
  :global(html.light) .markdown-content.streaming-welcome :global(p:last-child)::after {
    background: rgba(25, 78, 156, 0.72);
  }

  :global(html.light) .message-meta span {
    color: rgba(10, 30, 95, 0.68);
  }

	  :global(html.light) .composer-box textarea::placeholder {
	    color: rgba(10, 42, 108, 0.42);
	  }

	  :global(html.light) .stop-button {
	    border-color: rgba(220, 38, 38, 0.28);
	    background: rgba(254, 226, 226, 0.72);
	    color: rgba(127, 29, 29, 0.92);
	  }

	  :global(html.light) .stop-control-error {
	    color: rgba(153, 27, 27, 0.88);
	  }

  :global(html.light) .viewer-launch-overlay {
    background: rgba(15, 23, 42, 0.24);
  }

  :global(html.light) .viewer-launch-panel {
    border-color: rgba(100, 158, 220, 0.24);
    background: rgba(255, 255, 255, 0.94);
    box-shadow: 0 24px 70px rgba(15, 23, 42, 0.18);
  }

  :global(html.light) .viewer-launch-title {
    color: rgba(8, 28, 88, 0.94);
  }

  :global(html.light) .viewer-launch-copy {
    color: rgba(10, 42, 108, 0.68);
  }

  :global(html.light) .viewer-launch-error,
  :global(html.light) .takeover-error {
    color: rgba(153, 27, 27, 0.88);
  }

  :global(html.light) .viewer-launch-close {
    background: rgba(15, 23, 42, 0.06);
    color: rgba(15, 23, 42, 0.72);
  }

  :global(html.light) .runner-console {
    border-color: rgba(100, 158, 220, 0.24);
    background: rgba(248, 252, 255, 0.82);
    color: rgba(10, 30, 95, 0.88);
  }

  :global(html.light) .runner-console-header strong,
  :global(html.light) .runner-console-panel-header,
  :global(html.light) .runner-console-copy p,
  :global(html.light) .runner-console-empty {
    color: rgba(10, 30, 95, 0.9);
  }

  :global(html.light) .runner-console-header small,
  :global(html.light) .runner-console-header > span,
  :global(html.light) .runner-console-panel-header small,
  :global(html.light) .runner-console-copy span {
    color: rgba(10, 42, 108, 0.55);
  }

  :global(html.light) .runner-console-terminal,
  :global(html.light) .runner-console-turn {
    border-color: rgba(100, 158, 220, 0.18);
    background: rgba(255, 255, 255, 0.52);
  }

  :global(html.light) .runner-console-panel-header {
    border-bottom-color: rgba(100, 158, 220, 0.16);
  }

  :global(html.light) .runner-console-command {
    border-color: rgba(100, 158, 220, 0.16);
    background: rgba(255, 255, 255, 0.66);
    color: rgba(8, 28, 88, 0.9);
  }

  :global(html.light) .runner-console-wait-state {
    border-color: rgba(37, 99, 235, 0.18);
    background: rgba(219, 234, 254, 0.62);
  }

  :global(html.light) .runner-console-wait-state.waiting {
    border-color: rgba(13, 148, 136, 0.2);
    background: rgba(204, 251, 241, 0.56);
  }

  :global(html.light) .runner-console-wait-state small {
    color: rgba(29, 78, 216, 0.68);
  }

  :global(html.light) .runner-console-wait-state strong {
    color: rgba(15, 23, 42, 0.9);
  }

  :global(html.light) .runner-console-wait-state em {
    color: rgba(51, 65, 85, 0.72);
  }

	  @media (max-width: 1280px) {
    .command-center-page {
      grid-template-columns: minmax(0, 1fr) 240px;
    }

    .command-center-page.rail-collapsed {
      grid-template-columns: minmax(0, 1fr) 52px;
    }
  }

  @media (max-width: 900px) {
    .command-center-page {
      grid-template-columns: minmax(0, 1fr);
      height: calc(100vh - 96px);
      min-height: 560px;
    }

    .conversation-rail {
      display: none;
    }

    .chat-shell {
      grid-column: 1;
    }

    .runner-console-grid {
      grid-template-columns: 1fr;
    }

  }

  @media (max-width: 640px) {
    .command-center-page {
      height: calc(100vh - 88px);
      min-height: 520px;
    }

    .transcript,
    .composer {
      padding-left: 12px;
      padding-right: 12px;
    }

    .suggestion-row {
      padding-left: 12px;
      padding-right: 12px;
    }
    .message-row,
    .message-row.user {
      width: 100%;
      max-width: 100%;
    }
  }
</style>
