-- AlterTable
ALTER TABLE "Agent" ALTER COLUMN "twiml" SET DEFAULT '<?xml version="1.0" encoding="UTF-8"?><Response><Connect><Stream url="wss://YOUR_WEBSOCKET_HOST/media-stream"><Parameter name="agentId" value="{{agentId}}"/></Stream></Connect></Response>';
