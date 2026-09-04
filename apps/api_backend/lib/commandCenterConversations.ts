import { Prisma } from "@prisma/client";
import { prisma } from "./prisma";

const DEFAULT_TITLE = "New command session";
const MAX_TITLE_CHARS = 80;

export type CommandCenterConversationContext = {
  organizationId: string;
  userId: string;
};

export type CommandCenterConversationSummary = {
  id: string;
  title: string;
  createdAt: string;
  updatedAt: string;
};

export type CommandCenterStoredMessage = {
  id: string;
  role: "user" | "assistant";
  content: string;
  model: string | null;
  responseId: string | null;
  metadata: unknown | null;
  createdAt: string;
};

type CommandCenterConversationDb = Pick<
  typeof prisma,
  "commandCenterConversation" | "commandCenterMessage"
>;

type ConversationRecord = {
  id: string;
  title: string;
  createdAt: Date | string;
  updatedAt: Date | string;
};

type MessageRecord = {
  id: string;
  role: string;
  content: string;
  model: string | null;
  responseId: string | null;
  metadata: unknown | null;
  createdAt: Date | string;
};

function toIso(value: Date | string): string {
  return value instanceof Date ? value.toISOString() : new Date(value).toISOString();
}

function toConversationSummary(record: ConversationRecord): CommandCenterConversationSummary {
  return {
    id: record.id,
    title: record.title,
    createdAt: toIso(record.createdAt),
    updatedAt: toIso(record.updatedAt),
  };
}

function toStoredMessage(record: MessageRecord): CommandCenterStoredMessage {
  return {
    id: record.id,
    role: record.role === "assistant" ? "assistant" : "user",
    content: record.content,
    model: record.model,
    responseId: record.responseId,
    metadata: record.metadata,
    createdAt: toIso(record.createdAt),
  };
}

export function compactCommandCenterConversationTitle(content: string): string {
  const clean = content.replace(/\s+/g, " ").trim();
  if (!clean) {
    return DEFAULT_TITLE;
  }
  return clean.length > MAX_TITLE_CHARS ? `${clean.slice(0, MAX_TITLE_CHARS - 3).trim()}...` : clean;
}

export async function listCommandCenterConversations(
  context: CommandCenterConversationContext,
  db: CommandCenterConversationDb = prisma,
): Promise<CommandCenterConversationSummary[]> {
  const records = await db.commandCenterConversation.findMany({
    where: {
      organizationId: context.organizationId,
      userId: context.userId,
    },
    orderBy: { updatedAt: "desc" },
    select: {
      id: true,
      title: true,
      createdAt: true,
      updatedAt: true,
    },
  });
  return records.map(toConversationSummary);
}

export async function createCommandCenterConversation(
  context: CommandCenterConversationContext,
  options: { title?: string | null } = {},
  db: CommandCenterConversationDb = prisma,
): Promise<CommandCenterConversationSummary> {
  const record = await db.commandCenterConversation.create({
    data: {
      organizationId: context.organizationId,
      userId: context.userId,
      title: compactCommandCenterConversationTitle(options.title || DEFAULT_TITLE),
    },
    select: {
      id: true,
      title: true,
      createdAt: true,
      updatedAt: true,
    },
  });
  return toConversationSummary(record);
}

export async function getCommandCenterConversation(
  context: CommandCenterConversationContext,
  conversationId: string,
  db: CommandCenterConversationDb = prisma,
): Promise<CommandCenterConversationSummary | null> {
  const record = await db.commandCenterConversation.findFirst({
    where: {
      id: conversationId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
    select: {
      id: true,
      title: true,
      createdAt: true,
      updatedAt: true,
    },
  });
  return record ? toConversationSummary(record) : null;
}

export async function listCommandCenterMessages(
  context: CommandCenterConversationContext,
  conversationId: string,
  db: CommandCenterConversationDb = prisma,
): Promise<CommandCenterStoredMessage[] | null> {
  const conversation = await db.commandCenterConversation.findFirst({
    where: {
      id: conversationId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
    select: {
      id: true,
      messages: {
        orderBy: { createdAt: "asc" },
        select: {
          id: true,
          role: true,
          content: true,
          model: true,
          responseId: true,
          metadata: true,
          createdAt: true,
        },
      },
    },
  });
  if (!conversation) {
    return null;
  }
  return conversation.messages.map(toStoredMessage);
}

export async function appendCommandCenterMessage(
  context: CommandCenterConversationContext,
  conversationId: string,
  message: {
    role: "user" | "assistant";
    content: string;
    model?: string | null;
    responseId?: string | null;
    metadata?: unknown | null;
  },
  db: CommandCenterConversationDb = prisma,
): Promise<CommandCenterStoredMessage | null> {
  const conversation = await db.commandCenterConversation.findFirst({
    where: {
      id: conversationId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
    select: { id: true },
  });
  if (!conversation) {
    return null;
  }

  const data: any = {
    conversationId,
    role: message.role,
    content: message.content,
    model: message.model ?? null,
    responseId: message.responseId ?? null,
  };
  if (message.metadata !== undefined) {
    data.metadata = message.metadata === null ? Prisma.JsonNull : message.metadata;
  }

  const record = await db.commandCenterMessage.create({
    data,
    select: {
      id: true,
      role: true,
      content: true,
      model: true,
      responseId: true,
      metadata: true,
      createdAt: true,
    },
  });

  await db.commandCenterConversation.update({
    where: { id: conversationId },
    data: { updatedAt: new Date() },
  });

  return toStoredMessage(record);
}

export async function deleteCommandCenterConversation(
  context: CommandCenterConversationContext,
  conversationId: string,
  db: CommandCenterConversationDb = prisma,
): Promise<boolean> {
  const result = await db.commandCenterConversation.deleteMany({
    where: {
      id: conversationId,
      organizationId: context.organizationId,
      userId: context.userId,
    },
  });
  return result.count > 0;
}
