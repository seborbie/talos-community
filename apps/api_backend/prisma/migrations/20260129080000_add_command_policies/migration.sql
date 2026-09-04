-- Create command_policies table
CREATE TABLE command_policies (
  id BIGSERIAL PRIMARY KEY,
  command_name TEXT NOT NULL,
  scope_type TEXT NOT NULL CHECK (scope_type IN ('global', 'organization', 'customer', 'role')),
  organization_id TEXT,
  customer_id TEXT,
  role_scope TEXT CHECK (role_scope IN ('SUPER_ADMIN', 'AGENT_ADMIN', 'VIEWER')),
  policy_type TEXT NOT NULL CHECK (policy_type IN ('allow', 'deny')),
  allowed_parameters JSONB DEFAULT '[]',
  forbidden_parameters JSONB DEFAULT '[]',
  description TEXT,
  reason TEXT,
  created_by TEXT NOT NULL,
  created_at TIMESTAMPTZ DEFAULT now(),
  updated_at TIMESTAMPTZ DEFAULT now(),
  CONSTRAINT fk_organization FOREIGN KEY (organization_id) REFERENCES "Organization"("id") ON DELETE CASCADE,
  CONSTRAINT fk_customer FOREIGN KEY (customer_id) REFERENCES customers(id) ON DELETE CASCADE,
  UNIQUE(command_name, scope_type, organization_id, customer_id, role_scope)
);

CREATE INDEX idx_policies_org ON command_policies(organization_id);
CREATE INDEX idx_policies_customer ON command_policies(customer_id);
CREATE INDEX idx_policies_command ON command_policies(command_name);

-- Create command_execution_log table
CREATE TABLE command_execution_log (
  id BIGSERIAL PRIMARY KEY,
  organization_id TEXT NOT NULL,
  customer_id TEXT,
  user_id TEXT NOT NULL,
  agent_id TEXT NOT NULL,
  command TEXT NOT NULL,
  was_allowed BOOLEAN NOT NULL,
  denial_reason TEXT,
  matched_policy_id BIGINT,
  execution_time_ms INTEGER,
  exit_code INTEGER,
  output_length INTEGER,
  created_at TIMESTAMPTZ DEFAULT now()
);

CREATE INDEX idx_execution_log_org ON command_execution_log(organization_id, created_at);
CREATE INDEX idx_execution_log_agent ON command_execution_log(agent_id, created_at);

-- Seed global allowlist
INSERT INTO command_policies (command_name, scope_type, policy_type, description, created_by) VALUES
  ('Get-ComputerInfo', 'global', 'allow', 'System information retrieval', 'system'),
  ('Get-CimInstance', 'global', 'allow', 'WMI/CIM queries (with class whitelist)', 'system'),
  ('Get-WmiObject', 'global', 'allow', 'WMI queries (with class whitelist)', 'system'),
  ('systeminfo', 'global', 'allow', 'System information command', 'system'),
  ('Get-Service', 'global', 'allow', 'Service status checking', 'system'),
  ('Get-Process', 'global', 'allow', 'Process monitoring', 'system'),
  ('Get-Volume', 'global', 'allow', 'Disk volume information', 'system'),
  ('Get-Disk', 'global', 'allow', 'Physical disk information', 'system'),
  ('Get-PhysicalDisk', 'global', 'allow', 'Physical disk details', 'system'),
  ('Get-Partition', 'global', 'allow', 'Partition information', 'system'),
  ('Get-NetIPAddress', 'global', 'allow', 'IP address information', 'system'),
  ('Get-NetAdapter', 'global', 'allow', 'Network adapter information', 'system'),
  ('Get-NetRoute', 'global', 'allow', 'Routing table information', 'system'),
  ('Test-Connection', 'global', 'allow', 'Network connectivity testing', 'system'),
  ('Test-NetConnection', 'global', 'allow', 'Advanced network testing', 'system'),
  ('Get-EventLog', 'global', 'allow', 'Event log retrieval', 'system'),
  ('Get-WinEvent', 'global', 'allow', 'Advanced event log queries', 'system'),
  ('Get-Counter', 'global', 'allow', 'Performance counter data', 'system'),
  ('Select-Object', 'global', 'allow', 'Object property selection', 'system'),
  ('Where-Object', 'global', 'allow', 'Object filtering', 'system'),
  ('Sort-Object', 'global', 'allow', 'Object sorting', 'system'),
  ('Format-Table', 'global', 'allow', 'Table formatting', 'system'),
  ('Format-List', 'global', 'allow', 'List formatting', 'system'),
  ('Measure-Object', 'global', 'allow', 'Object aggregation', 'system');

-- Global denies (dangerous commands)
INSERT INTO command_policies (command_name, scope_type, policy_type, description, reason, created_by) VALUES
  ('Invoke-Expression', 'global', 'deny', 'Code execution', 'Arbitrary code execution risk', 'system'),
  ('Invoke-Command', 'global', 'deny', 'Remote execution', 'Remote command execution risk', 'system'),
  ('Invoke-WebRequest', 'global', 'deny', 'Web requests', 'Data exfiltration risk', 'system'),
  ('Invoke-RestMethod', 'global', 'deny', 'REST API calls', 'Data exfiltration risk', 'system'),
  ('Start-Process', 'global', 'deny', 'Process execution', 'Arbitrary process execution risk', 'system'),
  ('Start-Job', 'global', 'deny', 'Background jobs', 'Uncontrolled execution risk', 'system'),
  ('Remove-Item', 'global', 'deny', 'File deletion', 'Data loss risk', 'system'),
  ('Remove-Service', 'global', 'deny', 'Service deletion', 'System stability risk', 'system'),
  ('Set-ExecutionPolicy', 'global', 'deny', 'Policy modification', 'Security policy bypass risk', 'system'),
  ('New-Item', 'global', 'deny', 'File creation', 'System modification risk', 'system'),
  ('Set-Content', 'global', 'deny', 'File writing', 'System modification risk', 'system'),
  ('Add-Content', 'global', 'deny', 'File appending', 'System modification risk', 'system'),
  ('Out-File', 'global', 'deny', 'File output', 'System modification risk', 'system'),
  ('Clear-EventLog', 'global', 'deny', 'Log clearing', 'Audit trail destruction risk', 'system'),
  ('Clear-History', 'global', 'deny', 'History clearing', 'Audit trail destruction risk', 'system'),
  ('Enter-PSSession', 'global', 'deny', 'Remote session', 'Remote access risk', 'system'),
  ('New-PSSession', 'global', 'deny', 'Session creation', 'Remote access risk', 'system');

-- Add WMI/CIM class whitelist as parameter constraints
UPDATE command_policies
SET allowed_parameters = '[
  {"name": "ClassName", "allowed_values": [
    "Win32_OperatingSystem",
    "Win32_ComputerSystem",
    "Win32_Processor",
    "Win32_PhysicalMemory",
    "Win32_DiskDrive",
    "Win32_LogicalDisk",
    "Win32_NetworkAdapter",
    "Win32_NetworkAdapterConfiguration",
    "Win32_BIOS",
    "Win32_BaseBoard"
  ]}
]'::jsonb
WHERE command_name IN ('Get-CimInstance', 'Get-WmiObject') AND scope_type = 'global';
