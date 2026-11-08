# upscaler-bridge (Rust). Build from the repository root:
#   docker build -f server/Dockerfile -t pixelboost-upscaler-bridge .
FROM rust:1-bookworm AS build
WORKDIR /src
COPY rust/ rust/
RUN cd rust && cargo build --release -p pb-server --locked \
 && cp target/release/upscaler-bridge /upscaler-bridge

FROM debian:bookworm-slim
# curl: the compose healthcheck calls /health with it.
RUN apt-get update && apt-get install -y --no-install-recommends curl ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /upscaler-bridge /usr/local/bin/upscaler-bridge
# Bound to localhost: reached only through the Cloudflare tunnel (host network).
ENV HOST=127.0.0.1 PORT=8003
CMD ["upscaler-bridge"]
