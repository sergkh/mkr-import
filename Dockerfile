FROM rust:1.88-alpine AS builder
WORKDIR /app
RUN apk add --no-cache musl-dev

COPY Cargo.toml ./
COPY src ./src
RUN cargo build --release

FROM alpine:3.22 AS runner
WORKDIR /app
RUN apk add --no-cache ca-certificates
COPY --from=builder /app/target/release/mkr-import /usr/local/bin/mkr-import
CMD ["mkr-import", "watch"]