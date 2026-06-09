FROM rust:bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/emr /usr/local/bin/emr
EXPOSE 3200
ENTRYPOINT ["emr", "serve"]
