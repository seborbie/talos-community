CREATE SCHEMA IF NOT EXISTS command_center;

CREATE TABLE IF NOT EXISTS command_center.conversations (
  id TEXT NOT NULL,
  organization_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  title TEXT NOT NULL,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  updated_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT conversations_pkey PRIMARY KEY (id)
);

CREATE TABLE IF NOT EXISTS command_center.messages (
  id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL,
  model TEXT,
  response_id TEXT,
  metadata_jsonb JSONB,
  created_at TIMESTAMPTZ(3) NOT NULL DEFAULT CURRENT_TIMESTAMP,
  CONSTRAINT messages_pkey PRIMARY KEY (id),
  CONSTRAINT command_center_messages_role_check CHECK (role IN ('user', 'assistant'))
);

CREATE INDEX IF NOT EXISTS command_center_conversations_org_user_updated_idx
  ON command_center.conversations (organization_id, user_id, updated_at);

CREATE INDEX IF NOT EXISTS command_center_messages_conversation_created_idx
  ON command_center.messages (conversation_id, created_at);

ALTER TABLE command_center.messages
  DROP CONSTRAINT IF EXISTS command_center_messages_conversation_id_fkey,
  ADD CONSTRAINT command_center_messages_conversation_id_fkey
    FOREIGN KEY (conversation_id)
    REFERENCES command_center.conversations(id)
    ON DELETE CASCADE
    ON UPDATE CASCADE;
