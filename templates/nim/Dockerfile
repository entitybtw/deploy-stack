FROM nimlang/nim:alpine
WORKDIR /app
COPY *.nimble ./
RUN nimble install -d -y
COPY . .
RUN nim c -d:release server.nim

FROM alpine:3.19
COPY --from=build /app/server /server
EXPOSE 8080
CMD ["/server"]
