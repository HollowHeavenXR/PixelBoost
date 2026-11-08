# pixelboost-worker. Build from the repository root (see docker-compose.yml).
FROM rust:1-bookworm AS build
WORKDIR /src
COPY rust/ rust/
RUN cd rust && RUSTFLAGS="-C target-cpu=x86-64-v3" cargo build --release -p pb-worker --locked \
 && cp target/release/pixelboost-worker /pixelboost-worker

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /pixelboost-worker /usr/local/bin/pixelboost-worker
ENV HOST=0.0.0.0 PORT=8095 JOBS_DIR=/jobs MODELS_DIR=/models
CMD ["pixelboost-worker"]
