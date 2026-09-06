package main

import (
	"bufio"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

var templates = map[string]string{
	"static": "Static HTML/CSS/JS (nginx)",
	"php":    "PHP 8.2 (php-fpm)",
	"python": "Python 3.12 (gunicorn)",
	"node":   "Node.js 20",
	"go":     "Go (compiled binary)",
}

var defaultPorts = map[string]int{"static": 8080, "php": 9000, "python": 8000, "node": 3000, "go": 8080}

func stackDir() string   { ex, _ := os.Executable(); return filepath.Dir(ex) }
func composePath() string { return filepath.Join(stackDir(), "docker-compose.yml") }

func prompt(msg string) string {
	fmt.Print(msg)
	s := bufio.NewScanner(os.Stdin)
	s.Scan()
	return strings.TrimSpace(s.Text())
}

// ═══════════════════════════════════════════
//  Port management
// ═══════════════════════════════════════════

func usedPorts() map[int]bool {
	ports := map[int]bool{}
	data, err := os.ReadFile(composePath())
	if err != nil { return ports }
	for _, m := range regexp.MustCompile(`127\.0\.0\.1:(\d+):`).FindAllStringSubmatch(string(data), -1) {
		p, _ := strconv.Atoi(m[1]); ports[p] = true
	}
	return ports
}

func freePort(start int) int {
	used := usedPorts()
	for used[start] { start++ }
	return start
}

// ═══════════════════════════════════════════
//  Universal language detection
// ═══════════════════════════════════════════

type LangInfo struct {
	Name       string
	Template   string
	Port       int
	Extensions []string
}

func detectLang(dir string) LangInfo {
	skip := map[string]bool{"vendor": true, "node_modules": true, ".git": true, "__pycache__": true, ".venv": true, "dist": true, "build": true}
	var files []string
	filepath.Walk(dir, func(p string, i os.FileInfo, e error) error {
		if e != nil || i.IsDir() {
			if i != nil && skip[i.Name()] { return filepath.SkipDir }
			return nil
		}
		files = append(files, i.Name())
		return nil
	})

	has := func(ext string) bool { for _, f := range files { if strings.HasSuffix(f, ext) { return true } }; return false }
	contains := func(name string) bool { for _, f := range files { if f == name { return true } }; return false }

	// Rust
	if contains("Cargo.toml") {
		return LangInfo{"rust", "rust", 8080, nil}
	}
	// Go
	if contains("go.mod") {
		return LangInfo{"go", "go", 8080, nil}
	}
	// Java/Kotlin
	if contains("pom.xml") || contains("build.gradle") || contains("build.gradle.kts") {
		return LangInfo{"jvm", "jvm", 8080, nil}
	}
	// .NET/C#
	if contains("*.csproj") || contains("*.sln") || has(".cs") {
		return LangInfo{"dotnet", "dotnet", 8080, nil}
	}
	// Ruby
	if contains("Gemfile") || has(".rb") {
		return LangInfo{"ruby", "ruby", 3000, nil}
	}
	// Elixir
	if contains("mix.exs") {
		return LangInfo{"elixir", "elixir", 4000, nil}
	}
	// Node.js / TypeScript / Bun / Deno
	if contains("package.json") {
		if contains("bun.lockb") || contains("bun.lock") {
			return LangInfo{"bun", "bun", 3000, nil}
		}
		if has(".ts") || has(".tsx") {
			return LangInfo{"node-ts", "node", 3000, nil}
		}
		return LangInfo{"node", "node", 3000, nil}
	}
	if contains("deno.json") || contains("deno.jsonc") {
		return LangInfo{"deno", "deno", 8000, nil}
	}
	// Python
	if has(".py") || contains("requirements.txt") || contains("pyproject.toml") || contains("Pipfile") || contains("poetry.lock") {
		framework := "python"
		if has(".py") {
			content := readFileFirstN(dir, 50)
			if strings.Contains(content, "django") || contains("manage.py") { framework = "django" }
			if strings.Contains(content, "flask") { framework = "flask" }
			if strings.Contains(content, "fastapi") { framework = "fastapi" }
			if strings.Contains(content, "gunicorn") { framework = "python" }
		}
		return LangInfo{framework, framework, 8000, nil}
	}
	// PHP
	if has(".php") || contains("composer.json") {
		exts := []string{"curl", "mbstring", "opcache"}
		if has(".php") {
			content := readFileFirstN(dir, 100)
			if strings.Contains(content, "sqlite") || strings.Contains(content, "PDO") {
				exts = append(exts, "pdo_sqlite")
			}
			if strings.Contains(content, "gd") || strings.Contains(content, "imagecreate") {
				exts = append(exts, "gd")
			}
			if strings.Contains(content, "redis") { exts = append(exts, "redis") }
			if strings.Contains(content, "xml") || strings.Contains(content, "DOMDocument") {
				exts = append(exts, "dom")
			}
		}
		return LangInfo{"php", "php", 9000, exts}
	}
	// Haskell
	if contains("stack.yaml") || contains("*.cabal") || has(".hs") {
		return LangInfo{"haskell", "haskell", 8080, nil}
	}
	// Lua
	if has(".lua") || contains("luarocks.lock") {
		return LangInfo{"lua", "lua", 8080, nil}
	}
	// Zig
	if contains("build.zig") {
		return LangInfo{"zig", "zig", 8080, nil}
	}
	// Nim
	if contains("*.nimble") || has(".nim") {
		return LangInfo{"nim", "nim", 8080, nil}
	}
	// Swift
	if contains("Package.swift") {
		return LangInfo{"swift", "swift", 8080, nil}
	}
	// C/C++
	if has(".c") || has(".cpp") || has(".h") || has(".hpp") || contains("CMakeLists.txt") || contains("Makefile") {
		return LangInfo{"c", "c", 8080, nil}
	}
	// Static fallback
	return LangInfo{"static", "static", 8080, nil}
}

func readFileFirstN(dir string, n int) string {
	var lines []string
	filepath.Walk(dir, func(p string, i os.FileInfo, e error) error {
		if e != nil || i.IsDir() || len(lines) >= n { return nil }
		if strings.HasSuffix(i.Name(), ".py") || strings.HasSuffix(i.Name(), ".php") || strings.HasSuffix(i.Name(), ".js") || strings.HasSuffix(i.Name(), ".ts") {
			data, err := os.ReadFile(p)
			if err == nil { lines = append(lines, string(data)) }
		}
		return nil
	})
	return strings.Join(lines, "\n")
}

func detectDB(dir string) string {
	var db string
	filepath.Walk(dir, func(p string, i os.FileInfo, e error) error {
		if e != nil || i.IsDir() { return nil }
		if strings.HasSuffix(i.Name(), ".db") || strings.HasSuffix(i.Name(), ".sqlite") || strings.HasSuffix(i.Name(), ".sqlite3") {
			if i.Size() > 0 { db = p }
		}
		return nil
	})
	return db
}

// ═══════════════════════════════════════════
//  Universal Dockerfile generator
// ═══════════════════════════════════════════

func genDockerfile(dir string, lang LangInfo) string {
	switch lang.Name {
	case "static":
		return `FROM nginx:alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
`
	case "php":
		return fmt.Sprintf(`FROM php:8.2-fpm-alpine
RUN apk add --no-cache sqlite curl-dev oniguruma-dev
RUN docker-php-ext-install %s
RUN printf '[www]\\nuser = www-data\\ngroup = www-data\\nlisten = 0.0.0.0:9000\\npm = dynamic\\npm.max_children = 3\\npm.start_servers = 1\\npm.min_spare_servers = 1\\npm.max_spare_servers = 2\\n' > /usr/local/etc/php-fpm.d/zz-custom.conf
WORKDIR /var/www
`, strings.Join(lang.Extensions, " "))

	case "python", "django", "flask", "fastapi":
		return `FROM python:3.12-alpine
RUN apk add --no-cache gcc musl-dev
WORKDIR /app
COPY requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY . .
CMD ["sh", "-c", "gunicorn --bind 0.0.0.0:8000 ${APP_MODULE:-app:app}"]
`
	case "node", "node-ts":
		return `FROM node:20-alpine
WORKDIR /app
COPY package*.json ./
RUN npm ci --omit=dev
COPY . .
CMD ["node", "index.js"]
`
	case "bun":
		return `FROM oven/bun:alpine
WORKDIR /app
COPY package*.json bun.lock* ./
RUN bun install --production
COPY . .
CMD ["bun", "run", "index.ts"]
`
	case "deno":
		return `FROM denoland/deno:alpine
WORKDIR /app
COPY . .
CMD ["deno", "run", "--allow-net", "--allow-env", "main.ts"]
`
	case "go":
		return `FROM golang:1.22-alpine AS build
WORKDIR /src
COPY go.mod go.sum ./
RUN go mod download
COPY . .
RUN CGO_ENABLED=0 go build -o /app/server .

FROM alpine:3.19
COPY --from=build /app/server /server
EXPOSE 8080
CMD ["/server"]
`
	case "rust":
		return `FROM rust:1.77-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main(){}" > src/main.rs && cargo build --release && rm -rf src
COPY src ./src
RUN touch src/main.rs && cargo build --release

FROM alpine:3.19
COPY --from=build /target/release/server /server
EXPOSE 8080
CMD ["/server"]
`
	case "jvm":
		return `FROM eclipse-temurin:21-jdk-alpine AS build
WORKDIR /src
COPY . .
RUN if [ -f "pom.xml" ]; then ./mvnw package -DskipTests; elif [ -f "build.gradle" ]; then ./gradlew bootJar; fi

FROM eclipse-temurin:21-jre-alpine
COPY --from=build /src/target/*.jar /app.jar
EXPOSE 8080
CMD ["java", "-jar", "/app.jar"]
`
	case "dotnet":
		return `FROM mcr.microsoft.com/dotnet/sdk:8.0-alpine AS build
WORKDIR /src
COPY . .
RUN dotnet publish -c Release -o /app

FROM mcr.microsoft.com/dotnet/aspnet:8.0-alpine
COPY --from=build /app /app
EXPOSE 8080
WORKDIR /app
CMD ["dotnet", "app.dll"]
`
	case "ruby":
		return `FROM ruby:3.3-alpine
WORKDIR /app
COPY Gemfile* ./
RUN bundle install
COPY . .
CMD ["ruby", "app.rb"]
`
	case "elixir":
		return `FROM elixir:1.16-alpine
RUN apk add --no-cache build-base
WORKDIR /app
COPY mix.exs mix.lock ./
RUN mix deps.get && mix compile
COPY . .
CMD ["mix", "phx.server"]
`
	case "haskell":
		return `FROM haskell:9.6-alpine AS build
WORKDIR /src
COPY . .
RUN cabal build all

FROM alpine:3.19
COPY --from=build /src/dist-newstyle/**/server /server
EXPOSE 8080
CMD ["/server"]
`
	case "lua":
		return `FROM lua:5.4-alpine
RUN apk add --no-cache luarocks
WORKDIR /app
COPY . .
CMD ["lua", "main.lua"]
`
	case "zig":
		return `FROM zig:0.11-alpine AS build
WORKDIR /src
COPY . .
RUN zig build -Drelease-safe=true

FROM alpine:3.19
COPY --from=build /src/zig-out/bin/server /server
EXPOSE 8080
CMD ["/server"]
`
	case "nim":
		return `FROM nimlang/nim:alpine
WORKDIR /app
COPY *.nimble ./
RUN nimble install -d -y
COPY . .
RUN nim c -d:release server.nim

FROM alpine:3.19
COPY --from=build /app/server /server
EXPOSE 8080
CMD ["/server"]
`
	case "swift":
		return `FROM swift:5.10-alpine AS build
WORKDIR /src
COPY . .
RUN swift build -c release

FROM alpine:3.19
COPY --from=build /src/.build/release/server /server
EXPOSE 8080
CMD ["/server"]
`
	case "c":
		return `FROM gcc:alpine AS build
WORKDIR /src
COPY . .
RUN gcc -O2 -o server *.c -lm

FROM alpine:3.19
COPY --from=build /src/server /server
EXPOSE 8080
CMD ["/server"]
`
	}
	return `FROM alpine:3.19
WORKDIR /app
COPY . .
CMD ["sh", "-c", "echo 'No Dockerfile detected. Add one to your project.' && sleep infinity"]
`
}

// ═══════════════════════════════════════════
//  Compose manipulation
// ═══════════════════════════════════════════

func readCompose() string {
	data, err := os.ReadFile(composePath())
	if err != nil { return defaultCompose() }
	return string(data)
}

func writeCompose(c string) { os.WriteFile(composePath(), []byte(c), 0644) }

func defaultCompose() string {
	return `services:
  runner:
    image: data.forgejo.org/forgejo/runner:4.0.0
    container_name: runner
    restart: unless-stopped
    volumes:
      - /root/forgejo-runner/data:/data
      - /var/run/docker.sock:/var/run/docker.sock
      - /var/www:/var/www
    command: sh -c "sleep 10 && apk update && apk add --no-cache nodejs npm && forgejo-runner daemon --config /data/config.yaml"
`
}

func buildServiceBlock(name, dir string, port int, lang LangInfo) string {
	tmpl := filepath.Join(stackDir(), "templates", lang.Template)
	safeName := strings.ToLower(name)

	// Check if project has its own Dockerfile
	if _, err := os.Stat(filepath.Join(dir, "Dockerfile")); err == nil {
		tmpl = dir
	}

	switch lang.Name {
	case "static":
		return fmt.Sprintf(`  %s:
    image: nginx:alpine
    container_name: %s
    restart: unless-stopped
    ports: ["127.0.0.1:%d:80"]
    volumes:
      - %s:/usr/share/nginx/html:ro
      - %s/nginx.conf:/etc/nginx/conf.d/default.conf:ro
`, safeName, safeName, port, dir, tmpl)
	case "php":
		s := safeName + "-php"
		return fmt.Sprintf(`  %s:
    build: %s
    container_name: %s
    restart: unless-stopped
    ports: ["127.0.0.1:%d:9000"]
    volumes:
      - %s:/var/www/%s
`, s, tmpl, s, port, dir, name)
	default:
		return fmt.Sprintf(`  %s:
    build: %s
    container_name: %s
    restart: unless-stopped
    ports: ["127.0.0.1:%d:8080"]
    volumes:
      - %s:/app
`, safeName, tmpl, safeName, port, dir)
	}
}

func removeService(content, name string) string {
	for _, s := range []string{name, name + "-php", name + "-node", name + "-go", name + "-python", name + "-rust", name + "-jvm", name + "-dotnet", name + "-ruby", name + "-bun", name + "-deno"} {
		re := regexp.MustCompile(`(?m)^  ` + regexp.QuoteMeta(s) + `:.*`)
		loc := re.FindStringIndex(content)
		if loc == nil { continue }
		start := loc[0]
		end := loc[1]
		for end < len(content) {
			if end >= len(content) { break }
			if content[end] == '\n' {
				if end+1 < len(content) && content[end+1] == ' ' { end++; continue }
				break
			}
			end++
		}
		content = content[:start] + content[end:]
	}
	return content
}

func listServices() []string {
	var out []string
	for _, m := range regexp.MustCompile(`(?m)^  (\S+):`).FindAllStringSubmatch(readCompose(), -1) {
		if m[1] != "runner" && !strings.HasPrefix(m[1], "runner-") { out = append(out, m[1]) }
	}
	return out
}

func runnerNames() []string {
	var out []string
	for _, m := range regexp.MustCompile(`(?m)^  (runner\S*):`).FindAllStringSubmatch(readCompose(), -1) {
		out = append(out, m[1])
	}
	return out
}

// ═══════════════════════════════════════════
//  Runner management
// ═══════════════════════════════════════════

func addRunner(name, token, forgejoURL string) {
	svc := "runner"
	if name != "" { svc = "runner-" + name }
	content := readCompose()
	content = removeService(content, svc)
	block := fmt.Sprintf(`  %s:
    image: data.forgejo.org/forgejo/runner:4.0.0
    container_name: %s
    restart: unless-stopped
    environment:
      - FORGEJO_RUNNER_REGISTRATION_TOKEN=%s
      - FORGEJO_URL=%s
    volumes:
      - /root/forgejo-runner/%s:/data
      - /var/run/docker.sock:/var/run/docker.sock
      - /var/www:/var/www
    command: sh -c "sleep 10 && apk update && apk add --no-cache nodejs npm && forgejo-runner daemon --config /data/config.yaml"
`, svc, svc, token, forgejoURL, svc)
	content = strings.TrimRight(content, "\n") + "\n" + block
	writeCompose(content)
	fmt.Printf("✓ Runner '%s' added\n", svc)
}

// ═══════════════════════════════════════════
//  Docker
// ═══════════════════════════════════════════

func dockerCompose(args ...string) {
	cmd := exec.Command("docker", append([]string{"compose", "-f", composePath()}, args...)...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	cmd.Run()
}

// ═══════════════════════════════════════════
//  Interactive
// ═══════════════════════════════════════════

func interactive() {
	fmt.Println("\n╔══════════════════════════════════════════╗")
	fmt.Println("║        Deploy Stack — Add new site       ║")
	fmt.Println("╚══════════════════════════════════════════╝\n")

	name := prompt("  Site name: ")
	if name == "" { fmt.Println("  Error: name required"); return }

	dir := prompt(fmt.Sprintf("  Code directory [/var/www/%s]: ", name))
	if dir == "" { dir = "/var/www/" + name }
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		if strings.ToLower(prompt(fmt.Sprintf("  Create %s? [Y/n]: ", dir))) != "n" {
			os.MkdirAll(dir, 0755)
		} else { return }
	}

	lang := detectLang(dir)
	db := detectDB(dir)

	fmt.Printf("\n  Detected: %s\n", lang.Name)
	if db != "" { fmt.Printf("  Database: %s\n", filepath.Base(db)) }

	autoPort := freePort(lang.Port)
	portStr := prompt(fmt.Sprintf("  Port [%d auto]: ", autoPort))
	port := autoPort
	if portStr != "" && strings.ToLower(portStr) != "auto" { port, _ = strconv.Atoi(portStr) }

	fmt.Printf("\n  ┌──────────────────────────────┐\n")
	fmt.Printf("  │ Name:  %-22s │\n", name)
	fmt.Printf("  │ Dir:   %-22s │\n", dir)
	fmt.Printf("  │ Lang:  %-22s │\n", lang.Name)
	fmt.Printf("  │ Port:  %-22d │\n", port)
	if db != "" { fmt.Printf("  │ DB:    %-22s │\n", filepath.Base(db)) }
	fmt.Printf("  └──────────────────────────────┘\n")

	if strings.ToLower(prompt("\n  Deploy? [Y/n]: ")) == "n" { return }

	content := readCompose()
	content = removeService(content, name)
	content = strings.TrimRight(content, "\n") + "\n" + buildServiceBlock(name, dir, port, lang)
	writeCompose(content)

	svc := name; if lang.Name == "php" { svc = name + "-php" }
	fmt.Println()
	dockerCompose("up", "-d", "--build", svc)
	fmt.Printf("\n  ✓ %s deployed on port %d\n", name, port)
}

// ═══════════════════════════════════════════
//  Main
// ═══════════════════════════════════════════

func main() {
	if len(os.Args) < 2 { interactive(); return }

	switch os.Args[1] {
	case "add":
		if len(os.Args) < 4 { fmt.Println("Usage: deploy add <name> <dir> [--port N] [--type TYPE]"); return }
		name, dir := os.Args[2], os.Args[3]
		lang := detectLang(dir); port := freePort(lang.Port)
		for i, a := range os.Args {
			if a == "--port" && i+1 < len(os.Args) { port, _ = strconv.Atoi(os.Args[i+1]) }
			if a == "--type" && i+1 < len(os.Args) {
				tp := os.Args[i+1]
				if _, ok := templates[tp]; ok { lang = LangInfo{tp, tp, defaultPorts[tp], nil} }
			}
		}
		fmt.Printf("Adding %s (%s) port %d...\n", name, lang.Name, port)
		c := readCompose(); c = removeService(c, name)
		c = strings.TrimRight(c, "\n") + "\n" + buildServiceBlock(name, dir, port, lang)
		writeCompose(c)
		svc := name; if lang.Name == "php" { svc = name + "-php" }
		dockerCompose("up", "-d", "--build", svc)

		// Auto-migration: detect and run migration scripts
		migrations := []string{
			filepath.Join(dir, "install", "migrate.sh"),
			filepath.Join(dir, "migrate.sh"),
			filepath.Join(dir, "db", "migrate.sh"),
		}
		for _, m := range migrations {
			if _, err := os.Stat(m); err == nil {
				fmt.Printf("  Running migration: %s\n", filepath.Base(filepath.Dir(m)))
				cmd := exec.Command("docker", "exec", svc, "sh", m)
				cmd.Stdout = os.Stdout; cmd.Stderr = os.Stderr
				cmd.Run()
				break
			}
		}
		// Laravel artisan migrate
		if _, err := os.Stat(filepath.Join(dir, "artisan")); err == nil {
			fmt.Println("  Running: php artisan migrate --force")
			cmd := exec.Command("docker", "exec", svc, "php", "/var/www/"+name+"/artisan", "migrate", "--force")
			cmd.Stdout = os.Stdout; cmd.Stderr = os.Stderr
			cmd.Run()
		}
		fmt.Printf("✓ %s deployed on port %d\n", name, port)

	case "rm":
		if len(os.Args) < 3 { fmt.Println("Usage: deploy rm <name>"); return }
		n := os.Args[2]
		for _, s := range []string{n, n + "-php"} { dockerCompose("down", s) }
		writeCompose(removeService(readCompose(), n))
		fmt.Printf("✓ %s removed\n", n)

	case "list":
		for _, s := range listServices() { fmt.Printf("  %s\n", s) }
	case "runners":
		for _, r := range runnerNames() { fmt.Printf("  %s\n", r) }
	case "add-runner":
		if len(os.Args) < 4 { fmt.Println("Usage: deploy add-runner <name> <token> [--url URL]"); return }
		url := "https://forgejo.example.com"
		for i, a := range os.Args { if a == "--url" && i+1 < len(os.Args) { url = os.Args[i+1] } }
		addRunner(os.Args[2], os.Args[3], url)
		dockerCompose("up", "-d", os.Args[2])
	case "rm-runner":
		n := ""; if len(os.Args) >= 3 { n = os.Args[2] }
		if n == "" {
			for _, r := range runnerNames() { dockerCompose("down", r) }
			writeCompose(defaultCompose())
		} else {
			dockerCompose("down", "runner-"+n)
			writeCompose(removeService(readCompose(), "runner-"+n))
		}
		fmt.Println("✓ Runner removed")
	case "status": dockerCompose("ps")
	case "up":     dockerCompose("up", "-d")
	case "down":   dockerCompose("down")
	case "serve":
		port := 3000
		for i, a := range os.Args {
			if a == "--port" && i+1 < len(os.Args) {
				port, _ = strconv.Atoi(os.Args[i+1])
			}
		}
		startWebServer(port)
	default:
		fmt.Print(`deploy-stack — Universal site deployer

Usage:
  deploy                                  Interactive mode
  deploy add <name> <dir> [--port N]      Add site (auto-detect language)
  deploy rm <name>                        Remove site
  deploy list                             List all sites
  deploy runners                          List all runners
  deploy add-runner <name> <token>        Add Forgejo runner
  deploy rm-runner [name]                 Remove runner(s)
  deploy status                           Show containers
  deploy up / down                        Start/stop all

Supported languages (auto-detected):
  PHP, Python, Django, Flask, FastAPI, Node.js, TypeScript, Bun, Deno,
  Go, Rust, Java, Kotlin, .NET/C#, Ruby, Elixir, Haskell, Lua, Zig,
  Nim, Swift, C/C++, and any static site
`)
	}
}
