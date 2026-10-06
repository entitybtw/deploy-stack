FROM rust:1.99-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && cargo build --release && rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM debian:bookworm-slim
# sqlite3 — для install/migrate.sh, libssl3 — native-tls (SMTP/IMAP), ca-certificates — HTTPS
RUN apt-get update \
 && apt-get install -y --no-install-recommends sqlite3 libssl3 ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/server /usr/local/bin/server
EXPOSE 8080
CMD ["/usr/local/bin/server"]
