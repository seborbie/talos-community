import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  commandCenterMessageAiRunnerEvidence,
  commandCenterMessageAttachments,
  commandCenterMessageCommandApproval
} from './commandCenterAttachments';

test('commandCenterMessageAttachments keeps normal image attachments inline', () => {
  assert.deepEqual(
    commandCenterMessageAttachments({
      attachments: [
        {
          id: 'attachment-a',
          type: 'image',
          artifactId: 'artifact-a',
          mimeType: 'image/png',
          name: 'screen.png',
          width: 1024,
          height: 768
        }
      ]
    }),
    [
      {
        id: 'attachment-a',
        type: 'image',
        artifactId: 'artifact-a',
        mimeType: 'image/png',
        name: 'screen.png',
        width: 1024,
        height: 768,
        presentation: 'inline',
        jobId: undefined,
        frameSeq: undefined,
        cursor: undefined
      }
    ]
  );
});

test('commandCenterMessageAttachments parses live-frame cursor metadata', () => {
  assert.deepEqual(
    commandCenterMessageAttachments({
      attachments: [
        {
          type: 'image',
          artifactId: 'artifact-b',
          mimeType: 'image/png',
          name: 'desktop-goal-frame-2.png',
          width: 1024,
          height: 768,
          presentation: 'live_frame',
          jobId: 'job-a',
          frameSeq: 2,
          cursor: {
            visible: true,
            x: 320,
            y: 180,
            width: 1024,
            height: 768
          }
        }
      ]
    }),
    [
      {
        id: 'artifact-b',
        type: 'image',
        artifactId: 'artifact-b',
        mimeType: 'image/png',
        name: 'desktop-goal-frame-2.png',
        width: 1024,
        height: 768,
        presentation: 'live_frame',
        jobId: 'job-a',
        frameSeq: 2,
        cursor: {
          visible: true,
          x: 320,
          y: 180,
          width: 1024,
          height: 768
        }
      }
    ]
  );
});

test('commandCenterMessageCommandApproval parses pending command approvals', () => {
  assert.deepEqual(
    commandCenterMessageCommandApproval({
      commandApproval: {
        id: 'approval-a',
        jobId: 'job-a',
        turnIndex: 1,
        status: 'pending',
        command: 'Get-Service W32Time',
        explanation: 'Checks whether the Windows Time service exists.',
        risk: 'Read-only service inspection.',
        notes: ['No changes are made.'],
        message: 'Review this command.',
        policyAllowed: true,
        policyReason: 'Allowed by organization policy',
        output: null,
        outputLength: null,
        exitCode: null,
        error: null,
        updatedAt: '2026-06-15T10:00:00.000Z'
      }
    }),
    {
      id: 'approval-a',
      jobId: 'job-a',
      turnIndex: 1,
      status: 'pending',
      command: 'Get-Service W32Time',
      explanation: 'Checks whether the Windows Time service exists.',
      risk: 'Read-only service inspection.',
      notes: ['No changes are made.'],
      message: 'Review this command.',
      policyAllowed: true,
      policyReason: 'Allowed by organization policy',
      output: null,
      outputLength: null,
      exitCode: null,
      error: null,
      updatedAt: '2026-06-15T10:00:00.000Z'
    }
  );
});

test('commandCenterMessageAiRunnerEvidence parses final runner evidence metadata', () => {
  assert.deepEqual(
    commandCenterMessageAiRunnerEvidence({
      aiRunnerJob: {
        jobId: 'job-a',
        jobType: 'shell_goal',
        status: 'succeeded',
        shellTranscriptAvailable: true,
        desktopReplayAvailable: true,
        replayFrameCount: 3
      }
    }),
    {
      jobId: 'job-a',
      jobType: 'shell_goal',
      status: 'succeeded',
      shellTranscriptAvailable: true,
      desktopReplayAvailable: true,
      replayFrameCount: 3
    }
  );

  assert.equal(
    commandCenterMessageAiRunnerEvidence({
      aiRunnerJob: {
        jobId: 'job-a',
        jobType: 'shell_goal',
        status: 'succeeded',
        shellTranscriptAvailable: false,
        desktopReplayAvailable: false,
        replayFrameCount: 0
      }
    }),
    null
  );
});
