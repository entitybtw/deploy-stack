FROM golang:1.22-alpine AS build
WORKDIR /src
COPY go.mod ./
RUN go mod download
COPY *.go ./
RUN CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o /deploy .

FROM alpine:3.19
RUN apk add --no-cache ca-certificates sqlite
COPY --from=build /deploy /usr/local/bin/deploy
COPY static/ /usr/local/share/deploy-stack/static/
RUN mkdir -p /data
EXPOSE 3000
CMD ["deploy", "serve", "--port", "3000"]
