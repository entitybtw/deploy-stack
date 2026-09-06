# deploy-stack

**Universal Docker deployment platform.** One binary, any language, zero config.

Deploy any site with a single `git push`. No Dockerfiles, no YAML templates, no manual setup.

## Features

- **Auto-detect language** — PHP, Python, Go, Rust, Node.js, Ruby, Java, .NET, Elixir, Haskell, Lua, Zig, Nim, Swift, C/C++, and static sites
- **Auto-detect frameworks** — Django, Flask, FastAPI, Laravel, Bun, Deno
- **Auto-generate Dockerfile** — based on project files
- **Auto-pick port** — finds available ports automatically
- **Auto-migrate** — runs `install/migrate.sh` or `artisan migrate` on deploy
- **SQLite safe** — database files are never overwritten by rsync
- **Multi-runner** — multiple Forgejo runners in one compose
- **Single binary** — 3.5MB Go binary, no dependencies

## Quick Start

### Install

```bash
# Linux amd64
curl -L https://git.example.com/example/deploy-stack/raw/branch/main/dist/deploy-linux-amd64 -o /usr/local/bin/deploy
chmod +x /usr/local/bin/deploy
```

### First deploy

```bash
# Interactive mode
deploy

# Or CLI
deploy add mysite /var/www/mysite --port 8080
```

### Add to your repo

Create `.forgejo/workflows/deploy.yml`:

```yaml
name: Deploy
on:
  push:
    branches: [main]
jobs:
  deploy:
    runs-on: example
    steps:
      - uses: actions/checkout@v4
      - name: Deploy
        run: |
          rsync -a --delete \
            --exclude='.git' \
            --exclude='*.db' --exclude='*.sqlite' --exclude='*.sqlite3' \
            --exclude='*.db-wal' --exclude='*.db-shm' \
            ./ /var/www/mysite/
          deploy add mysite /var/www/mysite --port 8080
```

**Push code → site is live.** That's it.

## Commands

| Command | Description |
|---------|-------------|
| `deploy` | Interactive mode — add site step by step |
| `deploy add <name> <dir>` | Add site (auto-detect language & port) |
| `deploy add <name> <dir> --port 8080 --type php` | Add with explicit port and type |
| `deploy rm <name>` | Remove site |
| `deploy list` | List all sites |
| `deploy status` | Show container status |
| `deploy up` | Start all services |
| `deploy down` | Stop all services |
| `deploy add-runner <name> <token>` | Add Forgejo runner |
| `deploy rm-runner [name]` | Remove runner(s) |
| `deploy runners` | List all runners |

## Supported Languages

| Language | Detection | Frameworks |
|----------|-----------|------------|
| PHP | `.php`, `composer.json` | Laravel, Symfony, WordPress |
| Python | `.py`, `requirements.txt` | Django, Flask, FastAPI |
| Node.js | `package.json` | Express, Nest.js |
| TypeScript | `.ts` + `package.json` | — |
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
| Static | `index.html` | nginx |

## How It Works

```
git push → Forgejo Runner → deploy add → Docker Compose → Site live
```

1. **Push code** to Forgejo
2. **Runner** picks up the push and runs the workflow
3. **rsync** copies code to `/var/www/<site>/` (excluding `.git` and `*.db`)
4. **deploy** auto-detects language, generates Dockerfile, builds image
5. **Docker Compose** starts the container
6. **Migration** runs automatically if `install/migrate.sh` exists

## Database Handling

- Database files (`*.db`, `*.sqlite`) are **excluded from rsync** — they stay on the server forever
- On **first deploy**: migration creates tables
- On **subsequent deploys**: migration adds new tables/columns, data preserved
- SQLite files are mounted via Docker volumes — no data loss on restart

```
Git repo:     code only (no .db files)
Server:       code + database.sqlite (persistent)
```

## Architecture

```
/root/deploy-stack/
├── docker-compose.yml      # All services: runners + sites
├── deploy                  # Go binary (single file)
├── templates/              # Dockerfiles for each language
│   ├── php/Dockerfile
│   ├── python/Dockerfile
│   ├── go/Dockerfile
│   ├── node/Dockerfile
│   ├── rust/Dockerfile
│   └── static/nginx.conf
├── .env                    # Runner tokens
└── dist/                   # Binaries for all platforms
    ├── deploy-linux-amd64
    ├── deploy-linux-arm64
    ├── deploy-darwin-amd64
    ├── deploy-darwin-arm64
    └── deploy-windows-amd64.exe
```

## Custom Dockerfile

If you need a custom build — put a `Dockerfile` in your project root:

```
/var/www/mysite/
├── Dockerfile     # deploy uses this instead of the template
├── main.go
└── go.mod
```

## Build from Source

```bash
git clone https://git.example.com/example/deploy-stack.git
cd deploy-stack
go build -o deploy .
```

### Cross-compile

```bash
GOOS=linux GOARCH=amd64 go build -o dist/deploy-linux-amd64 .
GOOS=linux GOARCH=arm64 go build -o dist/deploy-linux-arm64 .
GOOS=darwin GOARCH=arm64 go build -o dist/deploy-darwin-arm64 .
GOOS=windows GOARCH=amd64 go build -o dist/deploy-windows-amd64.exe .
```

## License

MIT License — Copyright (c) 2026 [example](https://git.example.com/example)
