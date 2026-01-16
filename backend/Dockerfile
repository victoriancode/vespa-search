# syntax=docker/dockerfile:1
FROM rust:1.79 as builder
WORKDIR /app
COPY backend/Cargo.toml ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release
RUN rm -rf src
COPY backend/src ./src
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/vespa_code_search /usr/local/bin/vespa_code_search
EXPOSE 3001
CMD ["/usr/local/bin/vespa_code_search"]
