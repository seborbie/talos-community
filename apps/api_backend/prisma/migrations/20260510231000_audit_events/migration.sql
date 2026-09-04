CREATE TABLE IF NOT EXISTS public.audit_events (
  id BIGSERIAL PRIMARY KEY,
  organization_id TEXT NULL,
  customer_id TEXT NULL,
  site_id TEXT NULL,
  agent_id TEXT NULL,
  actor_type TEXT NOT NULL DEFAULT 'user',
  user_id TEXT NULL,
  user_email TEXT NULL,
  action_type TEXT NOT NULL,
  target_type TEXT NOT NULL,
  target_id TEXT NULL,
  target_name TEXT NULL,
  result TEXT NOT NULL DEFAULT 'success',
  status_code INTEGER NULL,
  error_message TEXT NULL,
  request_method TEXT NULL,
  request_path TEXT NULL,
  client_ip TEXT NULL,
  user_agent TEXT NULL,
  correlation_id TEXT NULL,
  session_id TEXT NULL,
  metadata_jsonb JSONB NOT NULL DEFAULT '{}'::jsonb,
  occurred_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT audit_events_result_check CHECK (result IN ('success', 'failure', 'blocked')),
  CONSTRAINT audit_events_actor_type_check CHECK (actor_type IN ('user', 'machine', 'agent', 'service', 'system', 'unknown'))
);

CREATE INDEX IF NOT EXISTS audit_events_organization_id_occurred_at_idx
  ON public.audit_events (organization_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_organization_id_action_type_occurred_at_idx
  ON public.audit_events (organization_id, action_type, occurred_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_organization_id_result_occurred_at_idx
  ON public.audit_events (organization_id, result, occurred_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_user_id_occurred_at_idx
  ON public.audit_events (user_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_agent_id_occurred_at_idx
  ON public.audit_events (agent_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_customer_id_occurred_at_idx
  ON public.audit_events (customer_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_site_id_occurred_at_idx
  ON public.audit_events (site_id, occurred_at DESC);
CREATE INDEX IF NOT EXISTS audit_events_session_id_idx
  ON public.audit_events (session_id);
CREATE INDEX IF NOT EXISTS audit_events_correlation_id_idx
  ON public.audit_events (correlation_id);
