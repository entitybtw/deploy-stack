FROM gcc:alpine AS build
WORKDIR /src
COPY . .
RUN gcc -O2 -o server *.c -lm

FROM alpine:3.19
COPY --from=build /src/server /server
EXPOSE 8080
CMD ["/server"]
