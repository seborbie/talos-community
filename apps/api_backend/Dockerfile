FROM oven/bun:1.3.14-alpine@sha256:5acc90a93e91ff07bf72aa90a7c9f0fa189765aec90b47bdbf2152d2196383c0 AS deps
WORKDIR /workspace/apps
COPY apps/package.json apps/bun.lock apps/bunfig.toml ./
COPY apps/scripts/check-toolchain.ts ./scripts/check-toolchain.ts
COPY apps/api_backend/package.json ./api_backend/package.json
COPY apps/frontend/package.json ./frontend/package.json
COPY apps/talos_permissions_helper/package.json ./talos_permissions_helper/package.json
COPY apps/talos_protocol_types/package.json ./talos_protocol_types/package.json
COPY apps/talos_viewer/package.json ./talos_viewer/package.json
COPY apps/talos_worker_chat/package.json ./talos_worker_chat/package.json
COPY apps/api_backend/prisma ./api_backend/prisma
RUN bun install --frozen-lockfile --filter api-backend
WORKDIR /workspace/apps/api_backend
RUN bunx prisma generate --schema=./prisma/schema.prisma

FROM oven/bun:1.3.14-alpine@sha256:5acc90a93e91ff07bf72aa90a7c9f0fa189765aec90b47bdbf2152d2196383c0 AS runner
WORKDIR /workspace/apps/api_backend
ENV NODE_ENV=production \
    PORT=3001
COPY --from=deps /workspace/apps/node_modules /workspace/apps/node_modules
COPY --from=deps /workspace/apps/api_backend/node_modules ./node_modules
COPY apps/api_backend/package.json ./
COPY apps/api_backend/prisma ./prisma
COPY apps/api_backend/lib ./lib
COPY apps/api_backend/routes ./routes
COPY apps/api_backend/middleware ./middleware
COPY apps/api_backend/server.ts ./server.ts
EXPOSE 3001
CMD ["bun", "server.ts"]
