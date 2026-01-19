# syntax=docker/dockerfile:1
FROM rust:1.83 as builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /data
RUN mkdir -p /app/vespa/application/security
COPY vespa/application/security/ca.pem /app/vespa/application/security/ca.pem
COPY vespa/application/security/clients.pem /app/vespa/application/security/clients.pem
COPY --from=builder /app/target/release/vespa_code_search /usr/local/bin/vespa_code_search
EXPOSE 3001
CMD ["/usr/local/bin/vespa_code_search"]
