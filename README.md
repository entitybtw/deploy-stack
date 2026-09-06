# deploy-stack

Универсальный деплой-стек на Docker Compose. Автоматизирует развёртывание сайтов на **любом языке** в один клик.

## Возможности

- **Авто-детект языка** — PHP, Python, Node.js, Go, Rust, Java, .NET, Ruby, Elixir, Haskell, Lua, Zig, Nim, Swift, C/C++ и статика
- **Авто-детект фреймворков** — Django, Flask, FastAPI, Bun, Deno
- **Авто-подбор порта** — не занятые порты определяются автоматически
- **Авто-генерация Dockerfile** — на основе файлов проекта
- **Авто-установка зависимостей** — npm, pip, go mod, cargo, composer, bundle
- **SQLite пробрасывается** — БД монтируется из хоста, не перезаписывается
- **Мульти-раннер** — несколько Forgejo Runner'ов в одном compose
- **Кросс-платформенный** — Linux, macOS, Windows (amd64/arm64)

## Быстрый старт

### На сервере (Linux amd64)

```bash
# Скачать и установить
curl -L https://git.example.com/example/deploy-stack/raw/branch/main/dist/deploy-linux-amd64 -o /usr/local/bin/deploy
chmod +x /usr/local/bin/deploy

# Инициализировать
cd /root && mkdir -p deploy-stack && cd deploy-stack
deploy
```

### На macOS

```bash
brew install deploy-stack
# или скачать бинарник вручную
```

### Интерактивный режим

```bash
deploy
```

```
╔══════════════════════════════════════════╗
║        Deploy Stack — Add new site       ║
╚══════════════════════════════════════════╝

  Site name: mysite
  Code directory [/var/www/mysite]:
  Detected: php

  ┌──────────────────────────────┐
  │ Name:  mysite                │
  │ Dir:   /var/www/mysite       │
  │ Lang:  php                   │
  │ Port:  9001                  │
  │ DB:    database.sqlite       │
  └──────────────────────────────┘

  Deploy? [Y/n]: Y
  ✓ mysite deployed on port 9001
```

## Команды

| Команда | Описание |
|---------|----------|
| `deploy` | Интерактивный режим |
| `deploy add <name> <dir>` | Добавить сайт (авто-детект языка и порта) |
| `deploy add <name> <dir> --port 8080 --type go` | С указанием порта и типа |
| `deploy rm <name>` | Удалить сайт |
| `deploy list` | Список всех сайтов |
| `deploy status` | Статус контейнеров |
| `deploy up` | Запустить всё |
| `deploy down` | Остановить всё |
| `deploy add-runner <name> <token>` | Добавить Forgejo Runner |
| `deploy rm-runner [name]` | Удалить Runner |
| `deploy runners` | Список Runner'ов |

## Workflow для Forgejo

В репозитории сайта создай `.forgejo/workflows/deploy.yml`:

```yaml
name: Deploy

on:
  push:
    branches: [main]

jobs:
  deploy:
    runs-on: example  # или имя твоего раннера
    steps:
      - uses: actions/checkout@v4

      - name: Deploy
        run: |
          rsync -a --delete --exclude='.git' ./ /var/www/mysite/
          deploy add mysite /var/www/mysite
```

Или просто:

```yaml
      - name: Deploy
        run: |
          rsync -a --delete --exclude='.git' ./ /var/www/mysite/
          docker compose -f /root/deploy-stack/docker-compose.yml up -d --build mysite-php
```

## Архитектура

```
/root/deploy-stack/
├── docker-compose.yml      ← все сервисы: раннеры + сайты
├── deploy                  ← бинарник (Go)
├── .env                    ← токены раннеров
├── templates/              ← Dockerfile для каждого языка
│   ├── php/Dockerfile
│   ├── python/Dockerfile
│   ├── go/Dockerfile
│   ├── node/Dockerfile
│   ├── rust/Dockerfile
│   ├── static/nginx.conf
│   └── ...
└── dist/                   ← бинарники для всех платформ
    ├── deploy-linux-amd64
    ├── deploy-linux-arm64
    ├── deploy-darwin-amd64
    ├── deploy-darwin-arm64
    └── deploy-windows-amd64.exe
```

## Поддерживаемые языки

| Язык | Авто-детект | Фреймворки |
|------|-------------|------------|
| PHP | `.php`, `composer.json` | Laravel, Symfony, WordPress |
| Python | `.py`, `requirements.txt` | Django, Flask, FastAPI |
| Node.js | `package.json` | Express, Nest.js, Next.js |
| Bun | `bun.lockb` | — |
| Deno | `deno.json` | — |
| Go | `go.mod` | Gin, Echo, Fiber |
| Rust | `Cargo.toml` | Actix, Axum |
| Java/Kotlin | `pom.xml`, `build.gradle` | Spring Boot |
| .NET/C# | `*.csproj` | ASP.NET |
| Ruby | `Gemfile` | Rails, Sinatra |
| Elixir | `mix.exs` | Phoenix |
| Haskell | `*.cabal`, `stack.yaml` | — |
| Lua | `.lua` | — |
| Zig | `build.zig` | — |
| Nim | `*.nimble` | — |
| Swift | `Package.swift` | — |
| C/C++ | `.c`, `.cpp`, `CMakeLists.txt` | — |
| Статика | `index.html` | nginx |

## Добавить свой Dockerfile

Если нужна кастомная сборка — положи `Dockerfile` в корень проекта:

```
/var/www/mysite/
├── Dockerfile     ← deploy использует его вместо шаблона
├── main.go
└── go.mod
```

## Лицензия

MIT
