# Build stage — компилируем статический Go-бинарь менеджера.
# ВНИМАНИЕ: если у хоста сломан IPv6, docker build может висеть на apk fetch.
# Собирай так:  docker build --network=host -t deploy-stack:latest .
FROM --platform=$BUILDPLATFORM golang:1.23-alpine AS build
WORKDIR /src
COPY go.mod ./
RUN go mod download
COPY *.go ./
ARG TARGETOS=linux
ARG TARGETARCH=amd64
RUN CGO_ENABLED=0 GOOS=${TARGETOS} GOARCH=${TARGETARCH} go build -ldflags="-s -w" -o /deploy-stack .

# Runtime — контейнер-менеджер
FROM alpine:3.20
RUN apk add --no-cache ca-certificates docker-cli docker-cli-compose curl git openssh-client rsync bash

# Разметка по умолчанию для образа
ENV DEPLOY_HOME=/deploy \
    DEPLOY_SITES=/deploy/sites/docker-compose.yml \
    DEPLOY_RUNNERS=/deploy/runners/docker-compose.yml \
    RUNNER_DATA=/runner

COPY --from=build /deploy-stack /usr/local/bin/deploy-stack

# Статик и шаблоны — читаются из DEPLOY_HOME если смонтированы, либо из фоллбеков
COPY static/  /usr/local/share/deploy-stack/static/
COPY templates/ /usr/local/share/deploy-stack/templates/

# Точки монтирования для панели (панель пишет в DEPLOY_HOME data/, sites/, runners/)
VOLUME ["/deploy", "/runner"]

WORKDIR /deploy
EXPOSE 3000

# command см. docker-compose: deploy-stack serve --port 3000
ENTRYPOINT ["/usr/local/bin/deploy-stack"]
