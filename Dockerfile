# Build stage — компилируем статический Rust-бинарь менеджера.
# ВНИМАНИЕ: если у хоста сломан IPv6, docker build может висеть на fetch.
# Собирай так:  docker build --network=host -t deploy-stack:latest .
# Ограничение параллелизма: сервер общий, максимум 2 нити (cargo -j2).
FROM --platform=$BUILDPLATFORM rust:1.80-alpine AS build
WORKDIR /src
RUN apk add --no-cache musl-dev
ENV CARGO_BUILD_JOBS=2
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs \
    && cargo build --release --locked || true
COPY src ./src
COPY static ./static
RUN touch src/main.rs && cargo build --release --locked -j2

# Runtime — контейнер-менеджер
FROM alpine:3.20
RUN apk add --no-cache ca-certificates docker-cli docker-cli-compose curl git openssh-client rsync bash

# Разметка по умолчанию для образа
ENV DEPLOY_HOME=/deploy \
    DEPLOY_SITES=/deploy/sites/docker-compose.yml \
    DEPLOY_RUNNERS=/deploy/runners/docker-compose.yml \
    RUNNER_DATA=/runner

COPY --from=build /src/target/release/deploy-stack /usr/local/bin/deploy-stack

# Статик и шаблоны — читаются из DEPLOY_HOME если смонтированы, либо из фоллбеков
COPY static/  /usr/local/share/deploy-stack/static/
COPY templates/ /usr/local/share/deploy-stack/templates/

# Точки монтирования для панели (панель пишет в DEPLOY_HOME data/, sites/, runners/)
VOLUME ["/deploy", "/runner"]

WORKDIR /deploy
EXPOSE 3000

# command см. docker-compose: deploy-stack serve --port 3000
ENTRYPOINT ["/usr/local/bin/deploy-stack"]
CMD ["serve", "--port", "3000"]
