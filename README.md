# deploy-stack

Universal Docker deployment platform with web UI, API, cluster management, and Forgejo CI/CD runners.

## Features

- **Containers** — list, exec, logs, inspect, start/stop/restart/remove
- **Cluster** — карточки серверов + «manage» открывает полноценный менеджер ноды
  (вкладки Containers / Runners / System, действия exec·logs·start·stop·restart·rm)
- **Runners** — Forgejo/Gitea/GitHub runner management with labels, logs, edit
- **Settings** — accent color, themes (dark/light/black), show/hide all containers
- **Auto-detection** — PHP, Python, Node.js, Go, Rust, Ruby, Java, .NET, Elixir, Haskell, Lua, Zig, Nim, Swift, C/C++, Bun, Deno
- **CLI** — `deploy add/rm/list/serve/add-runner/rm-runner`
- **Webhook** — auto-deploy on push via Forgejo/Gitea webhooks

## Architecture

```
Host
├── node-ci: Forgejo (git.example.com:3006) + services
├── node-4: deploy-stack (port 9090) + example.com (port 8088)
│   └── Runner: my-site:host
├── node-web: deploy-stack (port 9090) + N sites
│   └── Runner: webserver:host
└── node-app: deploy-stack (port 9090) + N sites
    └── Runner: worker:host
```

## Quick Start

### 1. Install Go and build

```bash
# On your build machine
git clone https://git.example.com/example/deploy-stack.git
cd deploy-stack
CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o deploy .
```

### 2. Deploy to server (Docker/host)

```bash
# Copy binary and static files
scp deploy root@SERVER:/root/deploy-stack/deploy
scp -r static/ root@SERVER:/root/deploy-stack/static/

# Create data dir
mkdir -p /root/deploy-stack/data

# Start
/root/deploy-stack/deploy serve --port 9090
```

### 3. Or use docker-compose

```bash
# On the target server
cd /root/deploy-stack
docker compose up -d
```

### Контейнерная поставка (одна команда для любого Docker-хоста)

В репозитории лежит готовый `docker-compose.yml` который поднимает панель-менеджер
и Forgejo runner одним стеком (панель работает через `docker.sock`, обязателен Linux-хост):

```bash
git clone https://git.example.com/example/deploy-stack.git
cd deploy-stack
cp .env.example .env        # задай DEPLOY_ADMIN_PASS (и токен раннера — опц.)
docker compose up -d        # панель on :3000 (+ раннер при DEPLOY_RUNNER_REGTOKEN)
```

Панель можно запускать и как системный сервис на хосте (без контейнера) — тогда она
управляет локальным демоном напрямую; оба способа дают один и тот же API/UI.


### 4. First login

Open `http://SERVER:9090` in browser.  
Default credentials: `admin` / `admin`

Change password via environment:

```bash
DEPLOY_ADMIN_PASS=your-password /root/deploy-stack/deploy serve --port 9090
```

## CLI Usage

```bash
deploy                                  # Interactive mode
deploy add <name> <dir> [--port N]      # Add site (auto-detect language)
deploy rm <name>                        # Remove site
deploy list                             # List all sites
deploy runners                          # List all runners
deploy add-runner <name> <token>        # Add Forgejo runner
deploy rm-runner [name]                 # Remove runner(s)
deploy status                           # Show containers
deploy up / down                        # Start/stop all
deploy serve --port 9090                # Start web server
```

## API

All endpoints require `Authorization: Bearer <token>` header.

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/login` | Get auth token |
| POST | `/api/v1/logout` | Invalidate token |
| GET | `/api/v1/status` | Server status |
| GET | `/api/v1/containers` | List containers |
| POST | `/api/v1/containers/:name/exec` | Execute command in container |
| GET | `/api/v1/containers/:name/logs` | Container logs |
| GET | `/api/v1/containers/:name/inspect` | Container inspect JSON |
| POST | `/api/v1/containers/:name/start` | Start container |
| POST | `/api/v1/containers/:name/stop` | Stop container |
| POST | `/api/v1/containers/:name/restart` | Restart container |
| DELETE | `/api/v1/containers/:name/remove` | Remove container |
| GET | `/api/v1/runners` | List runners |
| POST | `/api/v1/runners/add` | Add runner |
| POST | `/api/v1/runners/:name/edit` | Edit runner labels |
| GET | `/api/v1/runners/:name/logs` | Runner logs |
| POST | `/api/v1/runners/:name/restart` | Restart runner |
| POST | `/api/v1/runners/:name/stop` | Stop runner |
| DELETE | `/api/v1/runners/:name` | Remove runner |
| GET | `/api/v1/servers` | List cluster servers |
| POST | `/api/v1/servers` | Add server to cluster |
| DELETE | `/api/v1/servers/:id` | Remove server |
| GET | `/api/v1/servers/:id/containers` | Remote containers |
| GET | `/api/v1/servers/:id/runners` | Remote runners |
| POST | `/api/v1/servers/:id/containers/:name/exec` | Remote exec |

### Login example

```bash
curl -X POST http://localhost:9090/api/v1/login \
  -H "Content-Type: application/json" \
  -d '{"username":"<user>","password":"<pass>"}'
# Returns: {"status":"ok","token":"<token>"}
```

### Container exec example

```bash
curl -X POST http://localhost:9090/api/v1/containers/my-site-php/exec \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"cmd":"ls /var/www"}'
```

## Runner Labels

Labels determine which workflow runs on which server. Format: `label:host`

| Label | Server | Sites |
|-------|--------|-------|
| `my-site:host` | node-4 | my-app (example.com) |
| `web:host` | node-web | site-alpha, site-beta |
| `worker:host` | node-app | worker, worker-old |

### Workflow example

```yaml
name: Deploy
on:
  push:
    branches: [main]
jobs:
  deploy:
    runs-on: web  # matches runner with label "web"
    steps:
      - uses: actions/checkout@v4
      - name: Deploy
        run: |
          rsync -a --delete --exclude='.git' ./ /var/www/my-site/
          deploy add my-site /var/www/my-site --port 8080
```

## Forgejo Runner Setup

### Prerequisites

- Forgejo instance with Actions enabled
- Runner token from Forgejo admin panel

### Register runner

```bash
# On the target host
mkdir -p /root/forgejo-runner/data
# Download act_runner binary
# Register with token:
./act_runner register --config /root/forgejo-runner/data/config.yaml
```

### Runner docker-compose (deploy-stack)

The runner is included in `docker-compose.yml`:

```yaml
runner:
  image: data.forgejo.org/forgejo/runner:4.0.0
  container_name: runner
  user: root
  restart: unless-stopped
  volumes:
    - /root/forgejo-runner/data:/data
    - /var/run/docker.sock:/var/run/docker.sock
    - /var/www:/var/www
    - /root/deploy-stack:/root/deploy-stack
  command: >
    sh -c "sleep 10 &&
           apk update && apk add --no-cache nodejs npm rsync &&
           ln -sf /root/deploy-stack/deploy /usr/local/bin/deploy &&
           forgejo-runner daemon --config /data/config.yaml"
```

## Cluster Management

Add remote deploy-stack servers to manage them from one UI:

1. Go to **Cluster** tab
2. Click **+** → Add Server
3. Enter: Name, Host (IP), API Port, Login, Password
4. Click **Connect**
5. Click **manage** to open full control panel

The panel shows all containers and runners on the remote server with exec, logs, start/stop/restart controls.

## Updating

```bash
cd /root/deploy-stack
git pull
CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o deploy .
pkill -9 -f "deploy serve"
nohup /root/deploy-stack/deploy serve --port 9090 >/dev/null 2>&1 &
```

## Supported Languages (auto-detected)

| Language | Dockerfile | Default Port |
|----------|------------|--------------|
| Static HTML | nginx:alpine | 8080 |
| PHP 8.2 | php:8.2-fpm-alpine | 9000 |
| Python 3.12 | python:3.12-alpine | 8000 |
| Node.js 20 | node:20-alpine | 3000 |
| Bun | oven/bun:alpine | 3000 |
| Deno | denoland/deno:alpine | 8000 |
| Go | golang:1.22-alpine | 8080 |
| Rust | rust:1.77-alpine | 8080 |
| Java/Kotlin | eclipse-temurin:21 | 8080 |
| .NET/C# | mcr.microsoft.com/dotnet | 8080 |
| Ruby 3.3 | ruby:3.3-alpine | 3000 |
| Elixir 1.16 | elixir:1.16-alpine | 4000 |
| Haskell 9.6 | haskell:9.6-alpine | 8080 |
| Lua 5.4 | lua:5.4-alpine | 8080 |
| Zig 0.11 | zig:0.11-alpine | 8080 |
| Nim | nimlang/nim:alpine | 8080 |
| Swift 5.10 | swift:5.10-alpine | 8080 |
| C/C++ | gcc:alpine | 8080 |

## License

MIT
