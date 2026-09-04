# Dev only: debug build. Build from repo root: docker build -f apps/talos_server/Dockerfile -t talos/talos-server:dev .
# From apps/: bun run docker:talos-server
# For the full stack (one shared build, five service images): use infra/Dockerfile.rust-servers
FROM rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS build
WORKDIR /workspace
COPY apps ./
RUN cargo build --locked -p talos_server

FROM debian:bookworm-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /workspace/target/debug/talos_server /usr/local/bin/talos_server
EXPOSE 17110
CMD ["talos_server"]
