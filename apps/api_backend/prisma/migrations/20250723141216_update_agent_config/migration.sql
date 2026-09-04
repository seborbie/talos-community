-- AlterTable
ALTER TABLE "Agent" ADD COLUMN     "supervisorOpenAiModel" TEXT NOT NULL DEFAULT 'gpt-4o',
ADD COLUMN     "supervisorSystemPrompt" TEXT NOT NULL DEFAULT 'You are an experienced supervisor AI. Handle escalations and complex queries.';
