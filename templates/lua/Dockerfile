FROM lua:5.4-alpine
RUN apk add --no-cache luarocks
WORKDIR /app
COPY . .
CMD ["lua", "main.lua"]
