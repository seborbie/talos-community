import { createGeneratedSecret, type SecretSurface } from "../../secureNotes";
import type {
  CommandCenterMcpServer,
  CommandCenterMcpTool,
} from "../types";

const SERVER_NAME = "talos-secret-broker";
const SERVER_VERSION = "0.1.0";

function readString(value: unknown): string {
  return typeof value === "string" ? value.trim() : "";
}

function readSurface(value: unknown): SecretSurface | null {
  return value === "shell" || value === "desktop" || value === "note_only" ? value : null;
}

function readPositiveInt(value: unknown): number | undefined {
  const parsed = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) return undefined;
  return Math.trunc(parsed);
}

const createGeneratedSecretTool: CommandCenterMcpTool = {
  definition: {
    name: "create_generated_secret",
    description:
      "Generate a password secret for a Talos workflow without revealing the plaintext. Call only after the target workflow and device are known. Returns only a secret handle, shell/desktop references, and a recipient-bound one-time secure note link.",
    inputSchema: {
      type: "object",
      additionalProperties: false,
      required: ["kind", "purpose", "surface"],
      properties: {
        kind: {
          type: "string",
          enum: ["password"],
          description: "The kind of secret to generate. Only password is supported.",
        },
        purpose: {
          type: "string",
          description: "Brief reason for the generated secret, such as a temporary AD password.",
        },
        surface: {
          type: "string",
          enum: ["shell", "desktop", "note_only"],
          description: "Where the secret will be used.",
        },
        minWordLength: { type: "number", description: "Optional minimum dictionary word length." },
        maxWordLength: { type: "number", description: "Optional maximum dictionary word length." },
        maxPasswordLength: { type: "number", description: "Optional maximum generated password length." },
      },
    },
  },
  handler: async (args, context) => {
    const kind = readString(args.kind);
    if (kind !== "password") {
      throw new Error("Only password secret generation is supported");
    }
    const purpose = readString(args.purpose) || "Generated Talos secret";
    const surface = readSurface(args.surface);
    if (!surface) {
      throw new Error("surface must be shell, desktop, or note_only");
    }
    const result = await createGeneratedSecret({
      organizationId: context.organizationId,
      userId: context.userId,
      kind: "password",
      surface,
      purpose,
      passwordOptions: {
        minWordLength: readPositiveInt(args.minWordLength),
        maxWordLength: readPositiveInt(args.maxWordLength),
        maxPasswordLength: readPositiveInt(args.maxPasswordLength),
      },
    });
    return {
      ...result,
      message:
        surface === "shell" && result.shellReference
          ? `Generated secret is available as ${result.shellReference}.`
          : surface === "desktop" && result.desktopReference
            ? `Generated secret is available for desktop injection as ${result.desktopReference}.`
            : "Generated secret is available through the secure note link.",
    };
  },
};

export function createSecretBrokerMcpServer(): CommandCenterMcpServer {
  return {
    name: SERVER_NAME,
    version: SERVER_VERSION,
    tools: [createGeneratedSecretTool],
  };
}
