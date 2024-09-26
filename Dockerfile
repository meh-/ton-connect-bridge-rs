FROM rust:bookworm as builder
WORKDIR /app

# pre-build dependencies to cache them
RUN mkdir src && echo "fn main() {}" > src/main.rs
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release
RUN rm -rf src

# build the app
COPY ./src ./src
COPY ./config ./config
RUN cargo build --release


FROM debian:bookworm-slim
WORKDIR /app
COPY --from=builder /app/target/release/ton-connect-bridge-rs /app/ton-connect-bridge-rs
COPY --from=builder /app/config/default.yml /app/config/default.yml

RUN apt-get update \
    && apt-get install -y ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN update-ca-certificates

CMD ["/app/ton-connect-bridge-rs"]
