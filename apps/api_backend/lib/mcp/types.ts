export type CommandCenterMcpStatusEvent = {
  phase: "thinking" | "tool";
  message: string;
};

export type CommandCenterMcpContext = {
  userId: string;
  userEmail: string | null;
  organizationId: string;
  organizationName: string | null;
  role: string;
  conversationId?: string | null;
  abortSignal?: AbortSignal;
  emitStatus?: (event: CommandCenterMcpStatusEvent) => void | Promise<void>;
  addAttachment?: (attachment: {
    id: string;
    type: "image";
    mimeType: string;
    name: string;
    artifactId: string;
    width?: number;
    height?: number;
  }) => void;
};

export type CommandCenterMcpToolDefinition = {
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
};

export type CommandCenterMcpTool = {
  definition: CommandCenterMcpToolDefinition;
  handler: (
    args: Record<string, unknown>,
    context: CommandCenterMcpContext,
  ) => Promise<unknown>;
};

export type CommandCenterMcpServer = {
  name: string;
  version: string;
  tools: CommandCenterMcpTool[];
};
