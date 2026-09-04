-- AlterTable
ALTER TABLE "Agent" ADD COLUMN     "inputAudioFormat" TEXT NOT NULL DEFAULT 'g711_ulaw',
ADD COLUMN     "modalities" TEXT[] DEFAULT ARRAY['text', 'audio']::TEXT[],
ADD COLUMN     "openAiModel" TEXT NOT NULL DEFAULT 'gpt-4o-realtime-preview-2024-10-01',
ADD COLUMN     "outputAudioFormat" TEXT NOT NULL DEFAULT 'g711_ulaw',
ADD COLUMN     "systemMessage" TEXT NOT NULL DEFAULT 'You are a helpful AI assistant. Answer any question the user asks.',
ADD COLUMN     "temperature" DOUBLE PRECISION NOT NULL DEFAULT 0.8,
ADD COLUMN     "turnDetectionType" TEXT NOT NULL DEFAULT 'server_vad',
ADD COLUMN     "twiml" TEXT NOT NULL DEFAULT '<?xml version="1.0" encoding="UTF-8"?><Response><Connect><Stream url="wss://YOUR_WEBSOCKET_HOST/media-stream" /></Connect></Response>',
ADD COLUMN     "voice" TEXT NOT NULL DEFAULT 'echo';
