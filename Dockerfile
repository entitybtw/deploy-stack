# Build stage — компилируем статический Go-бинарь менеджера
FROM golang:1.23-alpine AS build
WORKDIR /src
COPY go.mod ./
RUN go mod download
COPY *.go ./
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o /deploy-stack .

# Runtime — контейнер-менеджер
FROM alpine:3.20
RUN apk add --no-cache ca-certificates docker-cli docker-cli-compose curl git openssh-client rsync bash

# Разметка по умолчанию для образа
ENV DEPLOY_HOME=/deploy \
    RUNNER_DATA=/runner

COPY --from=build /deploy-stack /usr/local/bin/deploy-stack

# Статик и шаблоны — читаются из DEPLOY_HOME если смонтированы, либо из фоллбеков
COPY static/  /usr/local/share/deploy-stack/static/
COPY templates/ /usr/local/share/deploy-stack/templates/

# Точки монтирования для панели (панель пишет в DEPLOY_HOME docker-compose.yml и data/)
VOLUME ["/deploy", "/runner"]

WORKDIR /deploy
EXPOSE 3000

# command см. docker-compose: deploy-stack serve --port 3000
ENTRYPOINT ["/usr/local/bin/deploy-stack"]
