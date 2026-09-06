FROM oven/bun:alpine
WORKDIR /app
COPY package*.json bun.lock* ./
RUN bun install --production
COPY . .
CMD ["bun", "run", "index.ts"]
