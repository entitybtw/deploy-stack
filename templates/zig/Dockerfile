FROM ziglang/zig:0.11-alpine AS build
WORKDIR /src
COPY . .
RUN zig build -Drelease-safe=true

FROM alpine:3.19
COPY --from=build /src/zig-out/bin/server /server
EXPOSE 8080
CMD ["/server"]
