FROM rust:1-slim AS builder

RUN apt-get update && apt-get install -y \
    pkg-config libssl-dev curl \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add wasm32-unknown-unknown
RUN cargo install --locked trunk

WORKDIR /app
COPY . .
RUN trunk build --release

FROM nginx:alpine
COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf
EXPOSE 80
