FROM rust:1.86-bookworm AS builder
WORKDIR /workspace

COPY Cargo.toml Cargo.lock* ./
COPY src ./src
COPY examples ./examples
COPY docs ./docs
COPY scripts ./scripts
COPY README.md CHANGELOG.md LICENSE ./

RUN cargo build --release --all-features --bin paradown

FROM debian:bookworm-slim
RUN apt-get update \
  && apt-get install -y --no-install-recommends ca-certificates \
  && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /workspace/target/release/paradown /usr/local/bin/paradown
COPY --from=builder /workspace/examples/config.toml /app/paradown.toml

ENTRYPOINT ["/usr/local/bin/paradown"]
