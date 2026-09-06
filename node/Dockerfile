FROM node:20-alpine

ARG NPM_INSTALL=true

WORKDIR /app

COPY package*.json ./
RUN if [ "$NPM_INSTALL" = "true" ] && [ -f package.json ]; then npm ci --omit=dev; fi

COPY . .

EXPOSE 3000
