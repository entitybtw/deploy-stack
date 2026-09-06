FROM swift:5.10-alpine AS build
WORKDIR /src
COPY . .
RUN swift build -c release

FROM alpine:3.19
COPY --from=build /src/.build/release/server /server
EXPOSE 8080
CMD ["/server"]
