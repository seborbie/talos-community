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
RUN bun install --frozen-lockfile --filter frontend

FROM oven/bun:1.3.14-alpine@sha256:5acc90a93e91ff07bf72aa90a7c9f0fa189765aec90b47bdbf2152d2196383c0 AS builder
WORKDIR /workspace/apps/frontend
COPY --from=deps /workspace/apps/node_modules /workspace/apps/node_modules
COPY --from=deps /workspace/apps/frontend/node_modules ./node_modules
COPY apps/frontend ./
ENV NODE_ENV=production
# PUBLIC_* service endpoints are read by adapter-node at container runtime. Do not bake an
# installation's domains into the reusable release image.
RUN bunx --bun svelte-kit sync && bunx --bun vite build

FROM oven/bun:1.3.14-alpine@sha256:5acc90a93e91ff07bf72aa90a7c9f0fa189765aec90b47bdbf2152d2196383c0 AS runner
WORKDIR /workspace/apps/frontend
ENV NODE_ENV=production \
    PORT=3000
COPY --from=deps /workspace/apps/node_modules /workspace/apps/node_modules
COPY --from=deps /workspace/apps/frontend/node_modules /workspace/apps/frontend/node_modules
COPY --from=builder /workspace/apps/frontend/build ./build
COPY --from=builder /workspace/apps/frontend/package.json ./package.json
EXPOSE 3000
CMD ["bun", "./build"]
