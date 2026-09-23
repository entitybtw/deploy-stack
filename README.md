# deploy-stack

Universal Docker deployment platform with web UI, API, multi-node cluster management, and Forgejo CI/CD runners.

**Zero-config deploy:** в репозитории достаточно указать label раннера (`runs-on`) и порт —
`deploy-stack` сам определит язык, соберёт контейнер и поднимет сайт.

```yaml
name: Deploy
on: { push: { branches: [main] } }
jobs:
  deploy:
    runs-on: web        # label раннера
    env: { PORT: 8084 }      # внешний порт сайта
    steps:
      - uses: actions/checkout@v4
      - run: deploy
```

## Features

### Web UI (v1.5.1)

| Tab | What you get |
|-----|--------------|
| **Sites** | All compose sites: port, status, open ↗, **term**, restart/stop/rm, search, auto-refresh |
| **Containers** | All Docker containers: image, ports, **term**, inspect, start/stop/restart/rm |
| **Cluster** | **Nodes** — cards with sites/containers/runners counts, online/offline, manage/ping/edit/rm. **All sites** — every site from every node in one list with **full control** (open↗ by node host, term, stop/restart/start, rm, manage) |
| **Runners** | Forgejo/Gitea/GitHub runners: labels, **term**, edit, restart/stop/rm |
| **Settings** | Accent color, themes (dark/light/black), show/hide all containers |

**Terminal (everywhere):** кнопка `term` открывает модалку с логами + строкой ввода команд.
Работает и на main-ноде, и на любой ноде кластера (exec стримится в реальном времени).
Есть `↻ logs` и `clear`.

**Deploy to node:** в форме деплоя можно выбрать целевую ноду кластера.

**Toasts** вместо alert; счётчики Sites/Nodes в top-bar.

### Engine

- **Zero-config deploy** — `deploy` в CI: имя из репозитория, язык авто, порт из `PORT`/`.deploy`
- **Auto-detection** — PHP, Python, Node.js, Go, Rust, Ruby, Java, .NET, Elixir, Haskell, Lua, Zig, Nim, Swift, C/C++, Bun, Deno
- **CLI** — `deploy` (auto) / `add` / `rm` / `list` / `logs` / `restart` / `serve` / `add-runner`
- **Webhook** — auto-deploy on push via Forgejo/Gitea webhooks
- **Cluster API** — aggregate status, remote exec/logs (streaming), remote deploy, node CRUD

## Architecture

```
Host
├── Forgejo (git.example.com) + Actions
├── node-web      :9090  + N sites  + runner label: web
├── node-app      :9090  + N sites  + runner label: app
└── node-ci   :9090  + N sites     + runner label: ci
         ↑
    All nodes join one Cluster (manage from any panel)
```

Each node runs the same panel (Docker or host binary). Any panel can act as
the "main" UI and remotely manage the others.

## Quick Start

### 1. Build

```bash
git clone https://git.example.com/example/deploy-stack.git
cd deploy-stack
CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -ldflags="-s -w" -o deploy .
```

Or with Docker (no local Go needed):

```bash
docker run --rm -v "$PWD":/src -w /src -e CGO_ENABLED=0 -e GOOS=linux -e GOARCH=amd64 \
  golang:1.23-alpine sh -c "go vet ./... && go build -ldflags='-s -w' -o /src/deploy ."
```

### 2. Install on a server

```bash
scp deploy root@SERVER:/root/deploy-stack/deploy
scp -r static/ root@SERVER:/root/deploy-stack/static/
mkdir -p /root/deploy-stack/data
/root/deploy-stack/deploy serve --port 9090
```

### 3. Or docker-compose

```bash
git clone https://git.example.com/example/deploy-stack.git
cd deploy-stack
./install.sh              # .env with random password + up
# or
cp .env.example .env      # set DEPLOY_ADMIN_PASS
docker compose up -d --build
# panel on :3000
```

> **IPv6:** if host IPv6 is broken, `docker build` may hang on `apk update`.
> Use `docker build --network=host -t deploy-stack:latest .` or set in
> `/etc/docker/daemon.json`: `{ "dns": ["1.1.1.1","8.8.8.8"], "ipv6": false }`

### 4. First login

Open `http://SERVER:PORT` → default `admin` / `admin`.

Change password:

```bash
DEPLOY_ADMIN_PASS=your-password /root/deploy-stack/deploy serve --port 9090
```

## CLI

```bash
deploy                                  # auto-deploy current repo (CI)
deploy --port 8084 [--name N --type T]  # auto-deploy with flags
deploy add <name> <dir> [--port N]      # add site (auto language)
deploy rm <name>                        # remove site
deploy list                             # sites: port + status
deploy logs <name> [--tail N]           # site logs
deploy restart <name>                   # restart site
deploy runners                          # list runners
deploy add-runner <name> <token>        # add Forgejo runner
deploy rm-runner [name]                 # remove runner(s)
deploy status                           # site containers
deploy up / down                        # start/stop all
deploy serve --port 3000                # panel (web UI + API)
```

### `.deploy` file (optional)

Put in repo root — then workflow only needs `runs-on`:

```
NAME=my-site
PORT=8084
TYPE=static      # static | php | python | node | go
```

### Per-site extra volumes (secrets/keys)

`site-volumes.conf` next to the binary (or `DEPLOY_SITE_VOLUMES`):

```
<service>=<host-path>:<container-path>[:ro][,...]
```

Example:

```
panel=/etc/ssh/reverse.key:/etc/deploy-secrets/reverse.key:ro
```

PHP container copies `/etc/deploy-secrets/*` → `/etc/ssh/` as `www-data:www-data 0600`
at start, so `ssh`/`rsync` from PHP work. File is gitignored.

## API

All endpoints require `Authorization: Bearer <token>`.

### Auth & status

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/api/v1/login` | Get token |
| POST | `/api/v1/logout` | Invalidate token |
| GET | `/api/v1/status` | Server status (containers/sites/runners/version) |
| GET | `/health` | Liveness (no auth) |

### Sites & containers

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/sites` | Managed sites (name/port/status/image) |
| POST | `/api/v1/sites` | Deploy site |
| GET | `/api/v1/containers` | List containers (`?show_all=true`) |
| GET | `/api/v1/containers/:name/logs` | Logs (`?tail=N`) |
| POST | `/api/v1/containers/:name/exec` | **Streaming** exec — body `{"cmd":"..."}` |
| GET | `/api/v1/containers/:name/inspect` | Inspect JSON |
| POST | `/api/v1/containers/:name/start\|stop\|restart` | Lifecycle |
| DELETE | `/api/v1/containers/:name/remove` | Remove container |

### Runners

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/runners` | List runners |
| POST | `/api/v1/runners/add` | Add runner |
| POST | `/api/v1/runners/:name/edit` | Edit labels |
| GET | `/api/v1/runners/:name/logs` | Runner logs |
| POST | `/api/v1/runners/:name/start\|stop\|restart` | Lifecycle |
| DELETE | `/api/v1/runners/:name` | Remove runner |

### Cluster

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/api/v1/cluster/status` | Aggregate all nodes (parallel) |
| GET | `/api/v1/servers` | List nodes |
| POST | `/api/v1/servers` | Add node |
| PUT | `/api/v1/servers/:id` | Update node (name/host/port/user/pass) |
| DELETE | `/api/v1/servers/:id` | Remove node |
| GET | `/api/v1/servers/:id/ping` | Ping one node |
| GET | `/api/v1/servers/:id/status` | Remote status |
| GET | `/api/v1/servers/:id/sites` | Remote sites |
| POST | `/api/v1/servers/:id/deploy` | Deploy on remote node |
| GET | `/api/v1/servers/:id/containers` | Remote containers |
| GET | `/api/v1/servers/:id/containers/:name/logs` | Remote logs (streamed) |
| POST | `/api/v1/servers/:id/containers/:name/exec` | Remote **streaming** exec |
| GET | `/api/v1/servers/:id/sites/:name/logs` | Remote site logs (streamed) |
| GET | `/api/v1/servers/:id/runners` | Remote runners |
| GET | `/api/v1/servers/:id/runners/:name/logs` | Remote runner logs (streamed) |
| POST/DELETE | `/api/v1/servers/:id/containers/:name/...` | Remote lifecycle proxy |

### Examples

```bash
# login
curl -X POST http://localhost:9090/api/v1/login \
  -H "Content-Type: application/json" \
  -d '{"username":"<user>","password":"<pass>"}'
# → {"status":"ok","token":"<token>"}

# exec
curl -N -X POST http://localhost:9090/api/v1/containers/my-site-php/exec \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{"cmd":"ls /var/www"}'

# cluster aggregate
curl -H "Authorization: Bearer <token>" \
  http://localhost:9090/api/v1/cluster/status
```

## Runner Labels

Format: `label:host` — determines which workflow runs on which node.

| Label | Node |
|-------|------|
| `web` | web node |
| `app` | app node |
| `ci` | build node |

### Workflow example

```yaml
name: Deploy
on:
  push:
    branches: [main]
jobs:
  deploy:
    runs-on: web            # runner label
    env:
      PORT: 8084                 # host port
    steps:
      - uses: actions/checkout@v4
      - run: deploy              # language auto-detected
```

For PHP sites needing their own webserver set `TYPE: php`
(nginx + php-fpm in one container) or drop a `Dockerfile` — auto-detected.

## Forgejo Runner Setup

```bash
cd /root/deploy-stack
./deploy add-runner web <REGISTRATION_TOKEN> --url http://forgejo:3000
```

Runner comes up in a container with docker-cli/rsync/node, links `deploy`,
and uses the given labels. Runner is also in `docker-compose.yml` (profile `ci`)
and mounts `/var/run/docker.sock`, `./www`, `./deploy-data`.

## Cluster Management

1. **Cluster** tab → **+ Node**
2. Name, Host (IP), API Port, Login, Password → **Connect**
3. Node card: **manage** / **ping** / **edit** / **rm**
4. **manage** → Containers / Sites / Runners / System — full control
   (term, start/stop/restart/rm, open↗)
5. **All sites** — every site on every node with the same controls;
   open↗ uses the **node's host**, not the panel host

Remote exec/logs are **streamed** (live output in Terminal).

## Updating

```bash
cd /root/deploy-stack
git pull
docker compose up -d --build          # if compose
# or host binary:
CGO_ENABLED=0 go build -ldflags="-s -w" -o deploy . && systemctl restart deploy-stack
```

**Production deploy note (this cluster):** only replace
`/root/deploy-stack/deploy`, `/root/deploy-stack/deploy-data/deploy`, and
`static/index.html`, then `docker restart deploy-stack` — never touch site
containers or `.env`.

## Supported Languages (auto-detected)

| Language | Base image | Internal port |
|----------|------------|---------------|
| Static HTML | nginx:alpine | 80 |
| PHP 8.2 | nginx + php-fpm | 80 |
| Python 3.12 / Django / Flask / FastAPI | python:3.12-alpine | 8000 |
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
