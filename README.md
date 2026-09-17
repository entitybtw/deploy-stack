# deploy-stack

Universal Docker deployment platform with web UI, API, cluster management, and Forgejo CI/CD runners.

**Zero-config деплой:** в репозитории достаточно указать label раннера (`runs-on`) и порт —
`deploy-stack` сам определит язык, соберёт контейнер и поднимет сайт.

```yaml
name: Deploy
on: { push: { branches: [main] } }
jobs:
  deploy:
    runs-on: web        # label раннера (node-web)
    env: { PORT: 8084 }      # внешний порт сайта
    steps:
      - uses: actions/checkout@v4
      - run: deploy
```

## Features

- **Zero-config deploy** — `deploy` в CI: имя из репозитория, язык авто, порт из `PORT`/`.deploy`
- **Containers** — list, exec, logs, inspect, start/stop/restart/remove
- **Cluster** — карточки серверов + «manage» открывает полноценный менеджер ноды
  (вкладки Containers / Runners / System, действия exec·logs·start·stop·restart·rm)
- **Runners** — Forgejo/Gitea/GitHub runner management with labels, logs, edit
- **Settings** — accent color, themes (dark/light/black), show/hide all containers
- **Auto-detection** — PHP, Python, Node.js, Go, Rust, Ruby, Java, .NET, Elixir, Haskell, Lua, Zig, Nim, Swift, C/C++, Bun, Deno
- **CLI** — `deploy` (auto) / `add` / `rm` / `list` / `logs` / `restart` / `serve` / `add-runner`
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
./install.sh              # сгенерирует .env со случайным паролем и запустит стек
# или вручную:
cp .env.example .env      # задай DEPLOY_ADMIN_PASS
docker compose up -d --build
# панель на :3000, раннер подключается при непустом DEPLOY_RUNNER_REGTOKEN
```

> **IPv6:** если у хоста сломан IPv6, `docker build` может «висеть» на `apk update`.
> Собирай через `docker build --network=host -t deploy-stack:latest .`
> или добавь в `/etc/docker/daemon.json`:
> ```json
> { "dns": ["1.1.1.1", "8.8.8.8"], "ipv6": false }
> ```

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
deploy                                  # Авто-деплой текущего репо (CI) / интерактивно
deploy --port 8084 [--name N --type T]  # Авто-деплой с явными параметрами
deploy add <name> <dir> [--port N]      # Добавить сайт (авто-определение языка)
deploy rm <name>                        # Удалить сайт
deploy list                             # Список сайтов: порт + статус
deploy logs <name> [--tail N]           # Логи сайта
deploy restart <name>                   # Перезапустить сайт
deploy runners                          # Список раннеров
deploy add-runner <name> <token>        # Добавить Forgejo runner
deploy rm-runner [name]                 # Удалить раннер(ы)
deploy status                           # Контейнеры сайтов
deploy up / down                        # Поднять/остановить всё
deploy serve --port 3000                # Панель-менеджер (web UI + API)
```

### Файл `.deploy` (опционально)

Положите в корень репозитория — тогда в workflow достаточно `runs-on`:

```
NAME=my-site
PORT=8084
TYPE=static      # static | php | python | node | go
```

### Доп. тома для конкретного сайта (ключи/секреты)

Чтобы не раздавать секреты всем сайтам, дополнительные bind-тома задаются
точечно в файле `site-volumes.conf` рядом с бинарём (`DEPLOY_HOME`) или в
переменной `DEPLOY_SITE_VOLUMES`. Формат строки:

```
<имя-сервиса>=<host-path>:<container-path>[:ro][,<host-path>:<container-path>...]
```

Пример — приватный SSH-ключ только для одного сайта:

```
panel=/etc/ssh/reverse.key:/etc/deploy-secrets/reverse.key:ro
```

PHP-контейнер при старте копирует `/etc/deploy-secrets/*` в `/etc/ssh/`
с правами `www-data:www-data 0600`, поэтому `ssh`/`rsync` из PHP работают
и не ругаются на «too open permissions».
Файл `site-volumes.conf` в git не попадает (см. `.gitignore`).


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
    runs-on: web            # label раннера
    env:
      PORT: 8084                 # внешний порт сайта
    steps:
      - uses: actions/checkout@v4
      - run: deploy              # язык определится сам
```

Для PHP-сайтов, которым нужен свой web-сервер, укажите `TYPE: php`
(шаблон поднимает nginx + php-fpm в одном контейнере) или положите
свой `Dockerfile` — deploy-stack использует его автоматически.

## Forgejo Runner Setup

### Prerequisites

- Forgejo instance with Actions enabled
- Runner token from Forgejo admin panel (Site Administration → Actions → Runners)

### Register runner (одной командой)

```bash
cd /root/deploy-stack
./deploy add-runner web <REGISTRATION_TOKEN> --url http://git.example.com:3000
```

Раннер сам поднимется в контейнере, установит docker-cli/rsync/node, слинкует
`deploy` и запустится с указанными label'ами. Для каждого сервера — свой label:

| Сервер | Label |
|--------|-------|
| node-4 | `my-site:host` |
| node-web | `web:host` |
| node-app | `worker:host` |

### Runner docker-compose

Раннер включён в `docker-compose.yml` (профиль `ci`): он монтирует
`/var/run/docker.sock`, `./www` и `./deploy-data` (ради бинаря `deploy`).

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
# если панель запущена через docker compose:
docker compose up -d --build
# если как systemd-сервис на хосте:
CGO_ENABLED=0 go build -ldflags="-s -w" -o deploy . && systemctl restart deploy-stack
```

## Supported Languages (auto-detected)

| Language | Dockerfile | Внутренний порт |
|----------|------------|------------------|
| Static HTML | nginx:alpine | 80 |
| PHP 8.2 | nginx + php-fpm (в одном контейнере) | 80 |
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
