FROM elixir:1.16-alpine
RUN apk add --no-cache build-base
WORKDIR /app
COPY mix.exs mix.lock ./
RUN mix deps.get && mix compile
COPY . .
CMD ["mix", "phx.server"]
