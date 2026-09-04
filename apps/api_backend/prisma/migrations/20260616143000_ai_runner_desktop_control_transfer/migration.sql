ALTER TABLE command_center.ai_runner_command_approvals
  DROP CONSTRAINT IF EXISTS command_center_ai_runner_command_approvals_status_check,
  ADD CONSTRAINT command_center_ai_runner_command_approvals_status_check
    CHECK (
      status IN (
        'pending',
        'approved',
        'denied',
        'desktop_control_requested',
        'executing',
        'executed',
        'failed',
        'expired',
        'policy_blocked'
      )
    );
