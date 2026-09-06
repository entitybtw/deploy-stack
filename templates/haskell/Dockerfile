FROM haskell:9.6-alpine AS build
WORKDIR /src
COPY . .
RUN cabal build all

FROM alpine:3.19
COPY --from=build /src/dist-newstyle/**/server /server
EXPOSE 8080
CMD ["/server"]
