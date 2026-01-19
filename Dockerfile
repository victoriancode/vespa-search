# syntax=docker/dockerfile:1
FROM rust:1.83 as builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
WORKDIR /app
ARG VESPA_CLIENT_CERT
ARG VESPA_CLIENT_KEY
ARG VESPA_CA_CERT
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /data
RUN mkdir -p /app/vespa/application/security
COPY vespa/application/security/clients.pem /app/vespa/application/security/clients.pem
RUN if [ -n "$VESPA_CLIENT_CERT" ]; then echo "$VESPA_CLIENT_CERT" > /app/vespa/application/security/client.pem; fi
RUN if [ -n "$VESPA_CLIENT_KEY" ]; then echo "$VESPA_CLIENT_KEY" > /app/vespa/application/security/client.key; fi
RUN if [ -n "$VESPA_CA_CERT" ]; then echo "$VESPA_CA_CERT" > /app/vespa/application/security/clients.pem; fi
COPY --from=builder /app/target/release/vespa_code_search /usr/local/bin/vespa_code_search
EXPOSE 3001
CMD ["/usr/local/bin/vespa_code_search"]
