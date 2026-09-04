import { describe, expect, test } from "bun:test";
import {
  appendCommandCenterMessage,
  compactCommandCenterConversationTitle,
  createCommandCenterConversation,
  deleteCommandCenterConversation,
  listCommandCenterConversations,
  listCommandCenterMessages,
} from "../lib/commandCenterConversations";

const baseDate = new Date("2026-06-10T09:00:00.000Z");

function createMockDb() {
  let conversationCounter = 0;
  let messageCounter = 0;
  const conversations = [
    {
      id: "conversation-a",
      organizationId: "org-a",
      userId: "user-a",
      title: "Owned chat",
      createdAt: baseDate,
      updatedAt: baseDate,
    },
    {
      id: "conversation-other-user",
      organizationId: "org-a",
      userId: "user-b",
      title: "Other user chat",
      createdAt: baseDate,
      updatedAt: baseDate,
    },
    {
      id: "conversation-other-org",
      organizationId: "org-b",
      userId: "user-a",
      title: "Other org chat",
      createdAt: baseDate,
      updatedAt: baseDate,
    },
  ];
  let messages = [
    {
      id: "message-a",
      conversationId: "conversation-a",
      role: "user",
      content: "Show installed apps",
      model: null,
      responseId: null,
      metadata: null,
      createdAt: baseDate,
    },
    {
      id: "message-other",
      conversationId: "conversation-other-user",
      role: "user",
      content: "Private",
      model: null,
      responseId: null,
      metadata: null,
      createdAt: baseDate,
    },
  ];

  const selectRecord = (record: any, select?: Record<string, any>) => {
    if (!select) return record;
    const selected: Record<string, unknown> = {};
    for (const key of Object.keys(select)) {
      if (key === "messages") {
        selected.messages = messages
          .filter((message) => message.conversationId === record.id)
          .sort((left, right) => left.createdAt.getTime() - right.createdAt.getTime())
          .map((message) => selectRecord(message, select.messages.select));
      } else if (select[key]) {
        selected[key] = record[key];
      }
    }
    return selected;
  };

  const matchesConversationScope = (record: any, where: any) =>
    (!where.id || record.id === where.id) &&
    record.organizationId === where.organizationId &&
    record.userId === where.userId;

  return {
    state: {
      conversations,
      get messages() {
        return messages;
      },
    },
    db: {
      commandCenterConversation: {
        findMany: async ({ where, select }: any) =>
          conversations
            .filter((conversation) => matchesConversationScope(conversation, where))
            .sort((left, right) => right.updatedAt.getTime() - left.updatedAt.getTime())
            .map((conversation) => selectRecord(conversation, select)),
        create: async ({ data, select }: any) => {
          conversationCounter += 1;
          const record = {
            id: `conversation-created-${conversationCounter}`,
            organizationId: data.organizationId,
            userId: data.userId,
            title: data.title,
            createdAt: baseDate,
            updatedAt: baseDate,
          };
          conversations.push(record);
          return selectRecord(record, select);
        },
        findFirst: async ({ where, select }: any) => {
          const record = conversations.find((conversation) => matchesConversationScope(conversation, where));
          return record ? selectRecord(record, select) : null;
        },
        update: async ({ where, data }: any) => {
          const record = conversations.find((conversation) => conversation.id === where.id);
          if (!record) throw new Error("conversation not found");
          if (data.updatedAt) record.updatedAt = data.updatedAt;
          return record;
        },
        deleteMany: async ({ where }: any) => {
          const deletedIds = conversations
            .filter((conversation) => matchesConversationScope(conversation, where))
            .map((conversation) => conversation.id);
          for (const id of deletedIds) {
            const index = conversations.findIndex((conversation) => conversation.id === id);
            if (index >= 0) conversations.splice(index, 1);
          }
          messages = messages.filter((message) => !deletedIds.includes(message.conversationId));
          return { count: deletedIds.length };
        },
      },
      commandCenterMessage: {
        create: async ({ data, select }: any) => {
          messageCounter += 1;
          const record = {
            id: `message-created-${messageCounter}`,
            conversationId: data.conversationId,
            role: data.role,
            content: data.content,
            model: data.model ?? null,
            responseId: data.responseId ?? null,
            metadata: data.metadata ?? null,
            createdAt: baseDate,
          };
          messages.push(record);
          return selectRecord(record, select);
        },
      },
    },
  };
}

describe("Command Center conversations", () => {
  const context = { organizationId: "org-a", userId: "user-a" };

  test("compacts conversation titles from the first user message", () => {
    expect(compactCommandCenterConversationTitle("  Show   installed apps  ")).toBe("Show installed apps");
    expect(compactCommandCenterConversationTitle("")).toBe("New command session");
    expect(compactCommandCenterConversationTitle("a".repeat(100))).toHaveLength(80);
  });

  test("lists only conversations owned by the active user and organization", async () => {
    const { db } = createMockDb();
    const items = await listCommandCenterConversations(context, db as any);

    expect(items).toHaveLength(1);
    expect(items[0].id).toBe("conversation-a");
  });

  test("creates a scoped conversation with a compact title", async () => {
    const { db } = createMockDb();
    const conversation = await createCommandCenterConversation(
      context,
      { title: "  Please inventory endpoint apps  " },
      db as any,
    );

    expect(conversation.id).toBe("conversation-created-1");
    expect(conversation.title).toBe("Please inventory endpoint apps");
  });

  test("returns messages only for an owned conversation", async () => {
    const { db } = createMockDb();

    await expect(listCommandCenterMessages(context, "conversation-other-user", db as any)).resolves.toBeNull();

    const items = await listCommandCenterMessages(context, "conversation-a", db as any);
    expect(items?.map((message) => message.id)).toEqual(["message-a"]);
  });

  test("appends messages only to owned conversations", async () => {
    const { db } = createMockDb();

    const blocked = await appendCommandCenterMessage(context, "conversation-other-org", {
      role: "user",
      content: "blocked",
    }, db as any);
    expect(blocked).toBeNull();

    const created = await appendCommandCenterMessage(context, "conversation-a", {
      role: "assistant",
      content: "Installed apps are listed below.",
      model: "gpt-5.5",
      responseId: "response-1",
    }, db as any);
    expect(created).toMatchObject({
      role: "assistant",
      content: "Installed apps are listed below.",
      model: "gpt-5.5",
      responseId: "response-1",
    });
  });

  test("hard deletes owned conversations and cascades their messages", async () => {
    const { db, state } = createMockDb();

    await expect(deleteCommandCenterConversation(context, "conversation-other-user", db as any)).resolves.toBe(false);
    await expect(deleteCommandCenterConversation(context, "conversation-a", db as any)).resolves.toBe(true);

    expect(state.conversations.some((conversation) => conversation.id === "conversation-a")).toBe(false);
    expect(state.messages.some((message) => message.conversationId === "conversation-a")).toBe(false);
  });
});
