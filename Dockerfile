# syntax=docker/dockerfile:1
FROM rust:1.79 as backend-builder
WORKDIR /app
COPY Cargo.toml ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs
RUN cargo build --release
RUN rm -rf src
COPY src ./src
RUN cargo build --release

FROM node:20-bookworm as frontend-builder
WORKDIR /app/frontend
COPY frontend/package.json ./
RUN npm install
COPY frontend ./
RUN npm run build

FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /data
COPY --from=backend-builder /app/target/release/vespa_code_search /usr/local/bin/vespa_code_search
COPY --from=frontend-builder /app/frontend/out /app/frontend/out
EXPOSE 3001
CMD ["/usr/local/bin/vespa_code_search"]
