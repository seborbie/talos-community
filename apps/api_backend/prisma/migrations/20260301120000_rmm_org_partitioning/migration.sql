-- Enforce tenant ownership directly on devices and telemetry artifacts.

ALTER TABLE public.rmm_devices
  ADD COLUMN IF NOT EXISTS organization_id TEXT;

UPDATE public.rmm_devices d
SET organization_id = c.organization_id
FROM public.customers c
WHERE d.organization_id IS NULL
  AND d.customer_id = c.id;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM public.rmm_devices WHERE organization_id IS NULL) THEN
    RAISE EXCEPTION 'rmm_devices contains rows without organization_id; assign customer/org before migration';
  END IF;
END $$;

ALTER TABLE public.rmm_devices
  ALTER COLUMN organization_id SET NOT NULL;

CREATE INDEX IF NOT EXISTS rmm_devices_organization_id_idx
  ON public.rmm_devices (organization_id);

CREATE UNIQUE INDEX IF NOT EXISTS rmm_devices_organization_id_agent_id_key
  ON public.rmm_devices (organization_id, agent_id);

ALTER TABLE public.rmm_devices
  DROP CONSTRAINT IF EXISTS rmm_devices_organization_id_fkey,
  ADD CONSTRAINT rmm_devices_organization_id_fkey
    FOREIGN KEY (organization_id) REFERENCES public."Organization"(id)
    ON DELETE RESTRICT ON UPDATE CASCADE;

-- Helper: add column + backfill from device scope
ALTER TABLE rmm_telemetry.snapshot_ingest ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.snapshot_request ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_state ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_installed_app ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_service ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_startup_item ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_windows_feature ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_pending_update ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_installed_update ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.processed_message_log ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.device_event ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.fact_state_current ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.fact_change_log ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.fact_baseline ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.routing_decision ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.remediation_job ADD COLUMN IF NOT EXISTS organization_id TEXT;
ALTER TABLE rmm_telemetry.remediation_step ADD COLUMN IF NOT EXISTS organization_id TEXT;

UPDATE rmm_telemetry.snapshot_ingest t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.snapshot_request t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_state t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_installed_app t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_service t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_startup_item t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_windows_feature t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_pending_update t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_installed_update t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.processed_message_log t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.device_event t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.fact_state_current t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.fact_change_log t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.fact_baseline t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.routing_decision t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.remediation_job t SET organization_id = d.organization_id FROM public.rmm_devices d WHERE t.organization_id IS NULL AND t.agent_id = d.agent_id;
UPDATE rmm_telemetry.remediation_step s SET organization_id = j.organization_id FROM rmm_telemetry.remediation_job j WHERE s.organization_id IS NULL AND s.job_id = j.id;

UPDATE rmm_telemetry.fact_baseline_scope f
SET organization_id = c.organization_id
FROM public.customers c
WHERE f.organization_id IS NULL
  AND f.customer_id = c.id;

UPDATE rmm_telemetry.fact_baseline_scope f
SET organization_id = d.organization_id
FROM public.rmm_devices d
WHERE f.organization_id IS NULL
  AND f.agent_id = d.agent_id;

UPDATE rmm_telemetry.fact_baseline_scope
SET organization_id = scope_key
WHERE organization_id IS NULL
  AND scope_type = 'organization';

UPDATE rmm_telemetry.routing_rule rr
SET organization_id = c.organization_id
FROM public.customers c
WHERE rr.organization_id IS NULL
  AND rr.customer_id = c.id;

UPDATE rmm_telemetry.routing_rule rr
SET organization_id = d.organization_id
FROM public.rmm_devices d
WHERE rr.organization_id IS NULL
  AND rr.agent_id = d.agent_id;

DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM rmm_telemetry.snapshot_ingest WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.snapshot_request WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_state WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_installed_app WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_service WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_startup_item WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_windows_feature WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_pending_update WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_installed_update WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.processed_message_log WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.device_event WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.fact_state_current WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.fact_change_log WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.fact_baseline WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.fact_baseline_scope WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.routing_rule WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.routing_decision WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.remediation_job WHERE organization_id IS NULL) OR
     EXISTS (SELECT 1 FROM rmm_telemetry.remediation_step WHERE organization_id IS NULL) THEN
    RAISE EXCEPTION 'one or more telemetry rows cannot be resolved to organization_id';
  END IF;
END $$;

ALTER TABLE rmm_telemetry.snapshot_ingest ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.snapshot_request ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_state ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_installed_app ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_service ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_startup_item ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_windows_feature ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_pending_update ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_installed_update ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.processed_message_log ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.device_event ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.fact_state_current ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.fact_change_log ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.fact_baseline ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.fact_baseline_scope ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.routing_rule ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.routing_decision ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.remediation_job ALTER COLUMN organization_id SET NOT NULL;
ALTER TABLE rmm_telemetry.remediation_step ALTER COLUMN organization_id SET NOT NULL;

CREATE INDEX IF NOT EXISTS snapshot_ingest_organization_id_collected_at_idx ON rmm_telemetry.snapshot_ingest (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS snapshot_request_organization_id_created_at_idx ON rmm_telemetry.snapshot_request (organization_id, created_at);
CREATE INDEX IF NOT EXISTS device_state_organization_id_collected_at_idx ON rmm_telemetry.device_state (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS device_installed_app_organization_id_collected_at_idx ON rmm_telemetry.device_installed_app (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS device_service_organization_id_collected_at_idx ON rmm_telemetry.device_service (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS device_startup_item_organization_id_collected_at_idx ON rmm_telemetry.device_startup_item (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS device_windows_feature_organization_id_collected_at_idx ON rmm_telemetry.device_windows_feature (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS device_pending_update_organization_id_collected_at_idx ON rmm_telemetry.device_pending_update (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS device_installed_update_organization_id_collected_at_idx ON rmm_telemetry.device_installed_update (organization_id, collected_at);
CREATE INDEX IF NOT EXISTS processed_message_log_organization_id_source_ts_idx ON rmm_telemetry.processed_message_log (organization_id, source_ts);
CREATE INDEX IF NOT EXISTS device_event_organization_id_occurred_at_idx ON rmm_telemetry.device_event (organization_id, occurred_at);
CREATE INDEX IF NOT EXISTS fact_state_current_organization_id_updated_at_idx ON rmm_telemetry.fact_state_current (organization_id, updated_at);
CREATE INDEX IF NOT EXISTS fact_change_log_organization_id_ts_idx ON rmm_telemetry.fact_change_log (organization_id, ts);
CREATE INDEX IF NOT EXISTS fact_baseline_organization_id_updated_at_idx ON rmm_telemetry.fact_baseline (organization_id, updated_at);
CREATE INDEX IF NOT EXISTS routing_decision_organization_id_source_ts_idx ON rmm_telemetry.routing_decision (organization_id, source_ts);
CREATE INDEX IF NOT EXISTS remediation_job_organization_id_requested_at_idx ON rmm_telemetry.remediation_job (organization_id, requested_at);
CREATE INDEX IF NOT EXISTS remediation_step_organization_id_started_at_idx ON rmm_telemetry.remediation_step (organization_id, started_at);

ALTER TABLE rmm_telemetry.snapshot_ingest
  DROP CONSTRAINT IF EXISTS snapshot_ingest_organization_id_fkey,
  ADD CONSTRAINT snapshot_ingest_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.snapshot_request
  DROP CONSTRAINT IF EXISTS snapshot_request_organization_id_fkey,
  ADD CONSTRAINT snapshot_request_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_state
  DROP CONSTRAINT IF EXISTS device_state_organization_id_fkey,
  ADD CONSTRAINT device_state_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_installed_app
  DROP CONSTRAINT IF EXISTS device_installed_app_organization_id_fkey,
  ADD CONSTRAINT device_installed_app_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_service
  DROP CONSTRAINT IF EXISTS device_service_organization_id_fkey,
  ADD CONSTRAINT device_service_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_startup_item
  DROP CONSTRAINT IF EXISTS device_startup_item_organization_id_fkey,
  ADD CONSTRAINT device_startup_item_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_windows_feature
  DROP CONSTRAINT IF EXISTS device_windows_feature_organization_id_fkey,
  ADD CONSTRAINT device_windows_feature_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_pending_update
  DROP CONSTRAINT IF EXISTS device_pending_update_organization_id_fkey,
  ADD CONSTRAINT device_pending_update_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_installed_update
  DROP CONSTRAINT IF EXISTS device_installed_update_organization_id_fkey,
  ADD CONSTRAINT device_installed_update_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.processed_message_log
  DROP CONSTRAINT IF EXISTS processed_message_log_organization_id_fkey,
  ADD CONSTRAINT processed_message_log_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_event
  DROP CONSTRAINT IF EXISTS device_event_organization_id_fkey,
  ADD CONSTRAINT device_event_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_state_current
  DROP CONSTRAINT IF EXISTS fact_state_current_organization_id_fkey,
  ADD CONSTRAINT fact_state_current_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_change_log
  DROP CONSTRAINT IF EXISTS fact_change_log_organization_id_fkey,
  ADD CONSTRAINT fact_change_log_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_baseline
  DROP CONSTRAINT IF EXISTS fact_baseline_organization_id_fkey,
  ADD CONSTRAINT fact_baseline_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_baseline_scope
  DROP CONSTRAINT IF EXISTS fact_baseline_scope_organization_id_fkey,
  ADD CONSTRAINT fact_baseline_scope_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.routing_rule
  DROP CONSTRAINT IF EXISTS routing_rule_organization_id_fkey,
  ADD CONSTRAINT routing_rule_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.routing_decision
  DROP CONSTRAINT IF EXISTS routing_decision_organization_id_fkey,
  ADD CONSTRAINT routing_decision_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.remediation_job
  DROP CONSTRAINT IF EXISTS remediation_job_organization_id_fkey,
  ADD CONSTRAINT remediation_job_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.remediation_step
  DROP CONSTRAINT IF EXISTS remediation_step_organization_id_fkey,
  ADD CONSTRAINT remediation_step_organization_id_fkey
  FOREIGN KEY (organization_id) REFERENCES public."Organization"(id) ON DELETE RESTRICT ON UPDATE CASCADE;

-- Enforce org+agent consistency at DB level (prevents cross-org writes even if app code regresses).
ALTER TABLE rmm_telemetry.snapshot_ingest
  DROP CONSTRAINT IF EXISTS snapshot_ingest_org_agent_fkey,
  ADD CONSTRAINT snapshot_ingest_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.snapshot_request
  DROP CONSTRAINT IF EXISTS snapshot_request_org_agent_fkey,
  ADD CONSTRAINT snapshot_request_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_state
  DROP CONSTRAINT IF EXISTS device_state_org_agent_fkey,
  ADD CONSTRAINT device_state_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_installed_app
  DROP CONSTRAINT IF EXISTS device_installed_app_org_agent_fkey,
  ADD CONSTRAINT device_installed_app_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_service
  DROP CONSTRAINT IF EXISTS device_service_org_agent_fkey,
  ADD CONSTRAINT device_service_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_startup_item
  DROP CONSTRAINT IF EXISTS device_startup_item_org_agent_fkey,
  ADD CONSTRAINT device_startup_item_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_windows_feature
  DROP CONSTRAINT IF EXISTS device_windows_feature_org_agent_fkey,
  ADD CONSTRAINT device_windows_feature_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_pending_update
  DROP CONSTRAINT IF EXISTS device_pending_update_org_agent_fkey,
  ADD CONSTRAINT device_pending_update_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_installed_update
  DROP CONSTRAINT IF EXISTS device_installed_update_org_agent_fkey,
  ADD CONSTRAINT device_installed_update_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.processed_message_log
  DROP CONSTRAINT IF EXISTS processed_message_log_org_agent_fkey,
  ADD CONSTRAINT processed_message_log_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.device_event
  DROP CONSTRAINT IF EXISTS device_event_org_agent_fkey,
  ADD CONSTRAINT device_event_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_state_current
  DROP CONSTRAINT IF EXISTS fact_state_current_org_agent_fkey,
  ADD CONSTRAINT fact_state_current_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_change_log
  DROP CONSTRAINT IF EXISTS fact_change_log_org_agent_fkey,
  ADD CONSTRAINT fact_change_log_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_baseline
  DROP CONSTRAINT IF EXISTS fact_baseline_org_agent_fkey,
  ADD CONSTRAINT fact_baseline_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.routing_decision
  DROP CONSTRAINT IF EXISTS routing_decision_org_agent_fkey,
  ADD CONSTRAINT routing_decision_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.remediation_job
  DROP CONSTRAINT IF EXISTS remediation_job_org_agent_fkey,
  ADD CONSTRAINT remediation_job_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.routing_rule
  DROP CONSTRAINT IF EXISTS routing_rule_org_agent_fkey,
  ADD CONSTRAINT routing_rule_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;

ALTER TABLE rmm_telemetry.fact_baseline_scope
  DROP CONSTRAINT IF EXISTS fact_baseline_scope_org_agent_fkey,
  ADD CONSTRAINT fact_baseline_scope_org_agent_fkey
  FOREIGN KEY (organization_id, agent_id) REFERENCES public.rmm_devices(organization_id, agent_id)
  ON DELETE CASCADE ON UPDATE CASCADE;
