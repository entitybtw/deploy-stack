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
	"php":    "PHP 8.2 (nginx + php-fpm)",
	"python": "Python 3.12 (gunicorn)",
	"node":   "Node.js 20",
	"go":     "Go (compiled binary)",
}

var defaultPorts = map[string]int{"static": 8080, "php": 8080, "python": 8000, "node": 3000, "go": 8080}

// ═══════════════════════════════════════════
//  Paths
// ═══════════════════════════════════════════

// deployHome — рабочий каталог менеджера: где живут sites/, templates/, static/.
// По умолчанию — каталог исполняемого файла (системд/раннер: /root/deploy-stack),
// в контейнере задаётся переменной DEPLOY_HOME.
func deployHome() string {
	if h := os.Getenv("DEPLOY_HOME"); h != "" {
		return filepath.Clean(h)
	}
	return filepath.Clean(filepath.Dir(os.Args[0]))
}

// composePath — файл, в котором живут сервисы сайтов (отдельно от стека панели).
func composePath() string {
	if p := os.Getenv("DEPLOY_SITES"); p != "" {
		return filepath.Clean(p)
	}
	return filepath.Join(deployHome(), "sites", "docker-compose.yml")
}

// runnersComposePath — файл, в котором живут сервисы CI-раннеров.
func runnersComposePath() string {
	if p := os.Getenv("DEPLOY_RUNNERS"); p != "" {
		return filepath.Clean(p)
	}
	return filepath.Join(deployHome(), "runners", "docker-compose.yml")
}

// templatesSearch returns candidate host dirs with Dockerfile/nginx-шаблонов.
func templatesSearch() []string {
	return []string{
		filepath.Join(deployHome(), "templates"),
		"/usr/local/share/deploy-stack/templates",
		filepath.Join(filepath.Dir(os.Args[0]), "templates"),
	}
}

// resolveTemplatesDir picks 1-ю существующую templates-папку, иначе дефолт в DEPLOY_HOME.
func resolveTemplatesDir() string {
	for _, d := range templatesSearch() {
		if fi, err := os.Stat(d); err == nil && fi.IsDir() {
			return d
		}
	}
	return templatesSearch()[0]
}

// runnerDataRoot — корень данных Forgejo-runner'ов (config.yaml/.runner).
func runnerDataRoot() string {
	if h := os.Getenv("RUNNER_DATA"); h != "" {
		return filepath.Clean(h)
	}
	return "/root/forgejo-runner"
}

func prompt(msg string) string {
	fmt.Print(msg)
	s := bufio.NewScanner(os.Stdin)
	s.Scan()
	return strings.TrimSpace(s.Text())
}

func isCI() bool {
	return os.Getenv("CI") != "" || os.Getenv("GITHUB_ACTIONS") != "" || os.Getenv("DEPLOY_AUTO") != ""
}

func isTTY() bool {
	fi, err := os.Stdin.Stat()
	if err != nil {
		return false
	}
	return fi.Mode()&os.ModeCharDevice != 0
}

// ═══════════════════════════════════════════
//  Port management
// ═══════════════════════════════════════════

// dockerHostPorts — реально опубликованные на хосте порты (docker ps),
// чтобы авто-выбор порта не наступал на уже запущенные контейнеры.
func dockerHostPorts() map[int]bool {
	ports := map[int]bool{}
	out, err := exec.Command("docker", "ps", "-a", "--format", "{{.Ports}}").Output()
	if err != nil {
		return ports
	}
	for _, m := range regexp.MustCompile(`:(\d+)->`).FindAllStringSubmatch(string(out), -1) {
		if p, err := strconv.Atoi(m[1]); err == nil {
			ports[p] = true
		}
	}
	return ports
}

func usedPorts() map[int]bool {
	ports := dockerHostPorts()
	if data, err := os.ReadFile(composePath()); err == nil {
		for _, m := range regexp.MustCompile(`(\d+):\d+"`).FindAllStringSubmatch(string(data), -1) {
			if p, err := strconv.Atoi(m[1]); err == nil {
				ports[p] = true
			}
		}
	}
	return ports
}

func freePort(start int) int {
	used := usedPorts()
	for used[start] {
		start++
	}
	return start
}

// containerHostPort — фактический host-порт запущенного сервиса (0 если нет).
func containerHostPort(name string) int {
	out, err := exec.Command("docker", "inspect", "-f",
		`{{range $p, $conf := .NetworkSettings.Ports}}{{range $conf}}{{.HostPort}} {{end}}{{end}}`, name).Output()
	if err != nil {
		return 0
	}
	for _, f := range strings.Fields(string(out)) {
		if p, err := strconv.Atoi(f); err == nil && p > 0 {
			return p
		}
	}
	return 0
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
			if i != nil && skip[i.Name()] {
				return filepath.SkipDir
			}
			return nil
		}
		files = append(files, i.Name())
		return nil
	})

	has := func(ext string) bool {
		for _, f := range files {
			if strings.HasSuffix(f, ext) {
				return true
			}
		}
		return false
	}
	contains := func(name string) bool {
		for _, f := range files {
			if f == name {
				return true
			}
		}
		return false
	}

	if contains("Cargo.toml") {
		return LangInfo{"rust", "rust", 8080, nil}
	}
	if contains("go.mod") {
		return LangInfo{"go", "go", 8080, nil}
	}
	if contains("pom.xml") || contains("build.gradle") || contains("build.gradle.kts") {
		return LangInfo{"jvm", "jvm", 8080, nil}
	}
	if contains("Gemfile") || has(".rb") {
		return LangInfo{"ruby", "ruby", 3000, nil}
	}
	if contains("mix.exs") {
		return LangInfo{"elixir", "elixir", 4000, nil}
	}
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
	if has(".py") || contains("requirements.txt") || contains("pyproject.toml") || contains("Pipfile") || contains("poetry.lock") {
		framework := "python"
		if has(".py") {
			content := readFileFirstN(dir, 50)
			if strings.Contains(content, "django") || contains("manage.py") {
				framework = "django"
			}
			if strings.Contains(content, "flask") {
				framework = "flask"
			}
			if strings.Contains(content, "fastapi") {
				framework = "fastapi"
			}
		}
		return LangInfo{framework, framework, 8000, nil}
	}
	if has(".php") || contains("composer.json") || contains("index.php") {
		exts := []string{"curl", "mbstring", "opcache", "pdo_sqlite"}
		if has(".php") {
			content := readFileFirstN(dir, 100)
			if strings.Contains(content, "gd") || strings.Contains(content, "imagecreate") {
				exts = append(exts, "gd")
			}
			if strings.Contains(content, "redis") {
				exts = append(exts, "redis")
			}
		}
		return LangInfo{"php", "php", 8080, exts}
	}
	if contains("stack.yaml") || has(".hs") {
		return LangInfo{"haskell", "haskell", 8080, nil}
	}
	if has(".lua") {
		return LangInfo{"lua", "lua", 8080, nil}
	}
	if contains("build.zig") {
		return LangInfo{"zig", "zig", 8080, nil}
	}
	if has(".nim") {
		return LangInfo{"nim", "nim", 8080, nil}
	}
	if contains("Package.swift") {
		return LangInfo{"swift", "swift", 8080, nil}
	}
	if has(".c") || has(".cpp") || contains("CMakeLists.txt") {
		return LangInfo{"c", "c", 8080, nil}
	}
	return LangInfo{"static", "static", 8080, nil}
}

func readFileFirstN(dir string, n int) string {
	var lines []string
	filepath.Walk(dir, func(p string, i os.FileInfo, e error) error {
		if e != nil || i.IsDir() || len(lines) >= n {
			return nil
		}
		if strings.HasSuffix(i.Name(), ".py") || strings.HasSuffix(i.Name(), ".php") || strings.HasSuffix(i.Name(), ".js") || strings.HasSuffix(i.Name(), ".ts") {
			if data, err := os.ReadFile(p); err == nil {
				lines = append(lines, string(data))
			}
		}
		return nil
	})
	return strings.Join(lines, "\n")
}

func detectDB(dir string) string {
	var db string
	filepath.Walk(dir, func(p string, i os.FileInfo, e error) error {
		if e != nil || i.IsDir() {
			return nil
		}
		if strings.HasSuffix(i.Name(), ".db") || strings.HasSuffix(i.Name(), ".sqlite") || strings.HasSuffix(i.Name(), ".sqlite3") {
			if i.Size() > 0 {
				db = p
			}
		}
		return nil
	})
	return db
}

// ═══════════════════════════════════════════
//  Compose helpers
// ═══════════════════════════════════════════

var serviceNameRe = regexp.MustCompile(`[^a-z0-9_.-]+`)

// serviceName превращает имя сайта в валидное имя docker-сервиса.
func serviceName(name string) string {
	s := serviceNameRe.ReplaceAllString(strings.ToLower(name), "-")
	s = strings.Trim(s, "-.")
	if s == "" {
		s = "site"
	}
	return s
}

func defaultCompose() string { return "services: {}\nvolumes: {}\n" }

func readCompose() string {
	data, err := os.ReadFile(composePath())
	if err != nil || strings.TrimSpace(string(data)) == "" {
		return defaultCompose()
	}
	return string(data)
}

func writeCompose(c string) {
	p := composePath()
	os.MkdirAll(filepath.Dir(p), 0755)
	os.WriteFile(p, []byte(c), 0644)
}

func readRunnersCompose() string {
	data, err := os.ReadFile(runnersComposePath())
	if err != nil || strings.TrimSpace(string(data)) == "" {
		return defaultCompose()
	}
	return string(data)
}

func writeRunnersCompose(c string) {
	p := runnersComposePath()
	os.MkdirAll(filepath.Dir(p), 0755)
	os.WriteFile(p, []byte(c), 0644)
}

func insertService(content, block string) string {
	block = strings.Trim(block, "\n")
	if strings.Contains(content, "services: {}") {
		return strings.Replace(content, "services: {}", "services:\n"+block, 1)
	}
	lines := strings.Split(content, "\n")
	for i, line := range lines {
		trimmed := strings.TrimSpace(line)
		if (trimmed == "volumes:" || trimmed == "volumes: {}") && !strings.HasPrefix(line, " ") && !strings.HasPrefix(line, "\t") {
			return strings.Join(lines[:i], "\n") + "\n" + block + "\n" + strings.Join(lines[i:], "\n")
		}
	}
	return strings.TrimRight(content, "\n") + "\n" + block + "\n"
}

func removeService(content, name string) string {
	candidates := []string{name}
	for _, suffix := range []string{"-php", "-node", "-go", "-python", "-rust", "-jvm", "-dotnet", "-ruby", "-bun", "-deno"} {
		candidates = append(candidates, name+suffix)
	}
	for _, s := range candidates {
		re := regexp.MustCompile(`(?m)^  ` + regexp.QuoteMeta(s) + `:.*`)
		loc := re.FindStringIndex(content)
		if loc == nil {
			continue
		}
		start, end := loc[0], loc[1]
		for end < len(content) {
			if content[end] == '\n' {
				if end+1 < len(content) && content[end+1] == ' ' {
					end++
					continue
				}
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
	for _, m := range regexp.MustCompile(`(?m)^  ([a-zA-Z0-9_.-]+):`).FindAllStringSubmatch(readCompose(), -1) {
		switch m[1] {
		case "services", "volumes", "networks":
			continue
		}
		out = append(out, m[1])
	}
	return out
}

// runnerNames — реальные контейнеры раннеров (а не только из compose).
func runnerNames() []string {
	out, err := exec.Command("docker", "ps", "-a", "--format", "{{.Names}}").Output()
	if err != nil {
		return nil
	}
	var r []string
	for _, n := range strings.Fields(string(out)) {
		if n == "runner" || strings.HasPrefix(n, "runner-") {
			r = append(r, n)
		}
	}
	return r
}

// ═══════════════════════════════════════════
//  Dockerfile generation
// ═══════════════════════════════════════════

func innerPort(lang LangInfo) int {
	switch lang.Name {
	case "static", "php":
		return 80
	case "python", "django", "flask", "fastapi", "deno":
		return 8000
	case "node", "node-ts", "bun", "ruby":
		return 3000
	case "elixir":
		return 4000
	default:
		return 8080
	}
}

// prepareBuild создаёт .deploy.Dockerfile в каталоге сайта из шаблона
// (для языков, которые собираются в образ). Так docker build берёт контекст
// из папки сайта (host-путь, видимый демону), а не из внутренностей контейнера.
func prepareBuild(dir string, lang LangInfo) error {
	if lang.Name == "static" {
		return nil
	}
	// Если репозиторий даёт собственный Dockerfile — используем его (кроме php,
	// где legacy Dockerfile только с php-fpm ломает деплой).
	if lang.Name != "php" {
		if _, err := os.Stat(filepath.Join(dir, "Dockerfile")); err == nil {
			return nil
		}
	}
	data, err := os.ReadFile(filepath.Join(resolveTemplatesDir(), lang.Template, "Dockerfile"))
	if err != nil {
		return fmt.Errorf("шаблон %s не найден: %v", lang.Template, err)
	}
	di := filepath.Join(dir, ".dockerignore")
	if _, err := os.Stat(di); err != nil {
		os.WriteFile(di, []byte(".git\nnode_modules\n__pycache__\n.venv\ndist\nbuild\n*.db\n*.sqlite\n*.sqlite3\n*.db-wal\n*.db-shm\n*.zip\n"), 0644)
	}
	return os.WriteFile(filepath.Join(dir, ".deploy.Dockerfile"), data, 0644)
}

// prepareStaticNginx кладёт nginx-конфиг в папку сайта (.deploy.nginx.conf),
// чтобы контейнер монтировал host-путь, видимый docker-демону.
func prepareStaticNginx(dir string) (string, error) {
	data, err := os.ReadFile(filepath.Join(resolveTemplatesDir(), "static", "nginx.conf"))
	if err != nil {
		return "", err
	}
	p := filepath.Join(dir, ".deploy.nginx.conf")
	if err := os.WriteFile(p, data, 0644); err != nil {
		return "", err
	}
	return p, nil
}

// ═══════════════════════════════════════════
//  Service block
// ═══════════════════════════════════════════

func buildServiceBlock(name, dir string, port int, lang LangInfo) string {
	safe := serviceName(name)
	bind := os.Getenv("DEPLOY_BIND")
	if bind == "" {
		bind = "0.0.0.0"
	}
	ownDockerfile := false
	if _, err := os.Stat(filepath.Join(dir, "Dockerfile")); err == nil {
		ownDockerfile = true
	}

	switch lang.Name {
	case "static":
		return fmt.Sprintf(`  %s:
    image: nginx:alpine
    container_name: %s
    restart: unless-stopped
    ports: ["%s:%d:80"]
    volumes:
      - %s:/usr/share/nginx/html:ro
      - %s/.deploy.nginx.conf:/etc/nginx/conf.d/default.conf:ro
    labels:
      - "deploy-stack.site=%s"
`, safe, safe, bind, port, dir, dir, name)

	case "php":
		// Site-контекст: .deploy.Dockerfile (nginx + php-fpm) кладётся prepareBuild.
		// Старый legacy Dockerfile репозитория (только php-fpm) игнорируется.
		return fmt.Sprintf(`  %s:
    build:
      context: %s
      dockerfile: .deploy.Dockerfile
    container_name: %s
    restart: unless-stopped
    ports: ["%s:%d:80"]
    volumes:
      - %s:/var/www/html
      - %s:/var/www/%s
    labels:
      - "deploy-stack.site=%s"
`, safe, dir, safe, bind, port, dir, dir, name, name)

	default:
		inner := innerPort(lang)
		df := ".deploy.Dockerfile"
		if ownDockerfile {
			df = "Dockerfile"
		}
		return fmt.Sprintf(`  %s:
    build:
      context: %s
      dockerfile: %s
    container_name: %s
    restart: unless-stopped
    ports: ["%s:%d:%d"]
    environment:
      - PORT=%d
    labels:
      - "deploy-stack.site=%s"
`, safe, dir, df, safe, bind, port, inner, inner, name)
	}
}

// ═══════════════════════════════════════════
//  Deploy
// ═══════════════════════════════════════════

func dockerCompose(args ...string) int { return dockerComposeFile(composePath(), args...) }

func dockerComposeFile(path string, args ...string) int {
	cmd := exec.Command("docker", append([]string{"compose", "-f", path}, args...)...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr
	if err := cmd.Run(); err != nil {
		if ee, ok := err.(*exec.ExitError); ok {
			return ee.ExitCode()
		}
		return 1
	}
	return 0
}

// syncDir копирует исходники в /var/www/<name> (rsync, иначе cp -a).
func syncDir(src, dst string) error {
	os.MkdirAll(dst, 0755)
	if _, err := exec.LookPath("rsync"); err == nil {
		cmd := exec.Command("rsync", "-a", "--delete",
			"--exclude=.git", "--exclude=*.db", "--exclude=*.sqlite", "--exclude=*.sqlite3",
			"--exclude=*.db-wal", "--exclude=*.db-shm", "--exclude=.deploy.Dockerfile",
			src+"/", dst+"/")
		if out, err := cmd.CombinedOutput(); err != nil {
			return fmt.Errorf("rsync: %v: %s", err, strings.TrimSpace(string(out)))
		}
		return nil
	}
	return copyTree(src, dst)
}

func copyTree(src, dst string) error {
	return filepath.Walk(src, func(p string, fi os.FileInfo, err error) error {
		if err != nil {
			return err
		}
		rel, _ := filepath.Rel(src, p)
		if rel == "." {
			return nil
		}
		if fi.IsDir() {
			if fi.Name() == ".git" {
				return filepath.SkipDir
			}
			return os.MkdirAll(filepath.Join(dst, rel), 0755)
		}
		data, err := os.ReadFile(p)
		if err != nil {
			return nil
		}
		return os.WriteFile(filepath.Join(dst, rel), data, fi.Mode())
	})
}

func runMigrations(svc, dir, name string) {
	base := "/var/www/" + name
	for _, s := range []string{"install/migrate.sh", "migrate.sh", "db/migrate.sh"} {
		if _, err := os.Stat(filepath.Join(dir, s)); err == nil {
			fmt.Printf("  migration: %s\n", s)
			cmd := exec.Command("docker", "exec", svc, "sh", filepath.Join(base, s))
			cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
			cmd.Run()
			return
		}
	}
	if _, err := os.Stat(filepath.Join(dir, "artisan")); err == nil {
		fmt.Println("  migration: php artisan migrate --force")
		cmd := exec.Command("docker", "exec", svc, "php", filepath.Join(base, "artisan"), "migrate", "--force")
		cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
		cmd.Run()
	}
}

// deploySite создаёт/обновляет сервис сайта и поднимает его. Возвращает фактический порт.
func deploySite(name, dir string, port int, lang LangInfo) (int, error) {
	if lang.Name == "static" {
		if _, err := prepareStaticNginx(dir); err != nil {
			return 0, fmt.Errorf("nginx-конфиг: %v", err)
		}
	}
	if err := prepareBuild(dir, lang); err != nil {
		return 0, err
	}
	safe := serviceName(name)
	c := readCompose()
	c = removeService(c, safe)
	c = insertService(c, buildServiceBlock(name, dir, port, lang))
	writeCompose(c)

	if code := dockerCompose("up", "-d", "--build", safe); code != 0 {
		return 0, fmt.Errorf("docker compose завершился с ошибкой для %s", safe)
	}
	runMigrations(safe, dir, name)

	actual := containerHostPort(safe)
	if actual == 0 {
		actual = port
	}
	return actual, nil
}

// envOr returns first non-empty string.
func envOr(keys ...string) string {
	for _, k := range keys {
		if v := os.Getenv(k); v != "" {
			return v
		}
	}
	return ""
}

func ciRepoName() string {
	for _, k := range []string{"GITHUB_REPOSITORY", "CI_PROJECT_PATH", "GITEA_REPOSITORY", "DEPLOY_REPO"} {
		if v := os.Getenv(k); v != "" {
			parts := strings.Split(v, "/")
			return parts[len(parts)-1]
		}
	}
	return ""
}

// loadDeployConfig читает опциональный deploy-конфиг в каталоге сайта:
// .deploy.env / deploy.env / .deploy (KEY=VALUE или KEY: VALUE).
func loadDeployConfig(dir string) map[string]string {
	cfg := map[string]string{}
	for _, f := range []string{".deploy.env", "deploy.env", ".deploy"} {
		data, err := os.ReadFile(filepath.Join(dir, f))
		if err != nil {
			continue
		}
		for _, line := range strings.Split(string(data), "\n") {
			line = strings.TrimSpace(line)
			if line == "" || strings.HasPrefix(line, "#") {
				continue
			}
			sep := strings.IndexAny(line, "=:")
			if sep <= 0 {
				continue
			}
			k := strings.ToUpper(strings.TrimSpace(line[:sep]))
			v := strings.Trim(strings.TrimSpace(line[sep+1:]), `"'`)
			cfg[k] = v
		}
	}
	return cfg
}

func parseFlags(args []string) map[string]string {
	opts := map[string]string{}
	for i := 0; i < len(args); i++ {
		a := args[i]
		if !strings.HasPrefix(a, "-") {
			continue
		}
		a = strings.TrimLeft(a, "-")
		if eq := strings.Index(a, "="); eq > 0 {
			opts[a[:eq]] = a[eq+1:]
			continue
		}
		if i+1 < len(args) && !strings.HasPrefix(args[i+1], "-") {
			opts[a] = args[i+1]
			i++
		} else {
			opts[a] = "true"
		}
	}
	return opts
}

// autoDeploy — «умный» режим: минимум ввода (label + port в workflow), всё остальное само.
func autoDeploy(args []string) {
	opts := parseFlags(args)

	dir := envOr("DEPLOY_DIR", "GITHUB_WORKSPACE")
	if v := opts["dir"]; v != "" {
		dir = v
	}
	if dir == "" {
		dir, _ = os.Getwd()
	}
	dir, _ = filepath.Abs(dir)

	cfg := loadDeployConfig(dir)

	name := envOr("DEPLOY_SITE")
	if v := opts["name"]; v != "" {
		name = v
	}
	if name == "" {
		name = cfg["NAME"]
	}
	if name == "" {
		name = ciRepoName()
	}
	if name == "" {
		name = filepath.Base(dir)
	}
	name = strings.TrimSpace(name)

	typ := envOr("DEPLOY_TYPE")
	if v := opts["type"]; v != "" {
		typ = v
	}
	if typ == "" {
		typ = cfg["TYPE"]
	}

	var lang LangInfo
	if typ != "" {
		if _, ok := templates[typ]; ok {
			lang = LangInfo{typ, typ, defaultPorts[typ], nil}
		}
	}
	if lang.Name == "" {
		lang = detectLang(dir)
	}

	portStr := envOr("DEPLOY_PORT", "PORT")
	if v := opts["port"]; v != "" {
		portStr = v
	}
	if portStr == "" {
		portStr = cfg["PORT"]
	}
	port := 0
	if portStr != "" {
		port, _ = strconv.Atoi(strings.TrimSpace(portStr))
	}
	if port == 0 {
		port = freePort(lang.Port)
	}

	target := "/var/www/" + name
	if filepath.Clean(dir) != filepath.Clean(target) {
		if err := syncDir(dir, target); err != nil {
			fmt.Printf("  ! синхронизация: %v\n", err)
		}
	}
	srcDir := target
	if fi, err := os.Stat(target); err != nil || !fi.IsDir() {
		srcDir = dir
	}

	fmt.Printf("• %s (%s), каталог: %s\n", name, lang.Name, srcDir)
	actual, err := deploySite(name, srcDir, port, lang)
	if err != nil {
		fmt.Printf("✗ %s: %v\n", name, err)
		os.Exit(1)
	}
	fmt.Printf("✓ %s задеплоен: http://%s:%d\n", name, hostIP(), actual)
}

func hostIP() string {
	if v := os.Getenv("DEPLOY_HOST"); v != "" {
		return v
	}
	return "0.0.0.0"
}

// ═══════════════════════════════════════════
//  Runner management
// ═══════════════════════════════════════════

func addRunner(name, token, forgejoURL string) {
	svc := "runner"
	if name != "" {
		svc = serviceName("runner-" + name)
	}
	content := readRunnersCompose()
	content = removeService(content, svc)
	block := fmt.Sprintf(`  %s:
    image: data.forgejo.org/forgejo/runner:4.0.0
    container_name: %s
    user: root
    restart: unless-stopped
    volumes:
      - %s/%s:/data
      - /var/run/docker.sock:/var/run/docker.sock
      - /var/www:/var/www
      - %s:%s
    environment:
      DEPLOY_HOME: %s
    command: >
      sh -c "apk update && apk add --no-cache docker-cli docker-cli-compose rsync nodejs npm &&
             ln -sf %s/deploy /usr/local/bin/deploy &&
             forgejo-runner daemon --config /data/config.yaml"
`, svc, svc, runnerDataRoot(), svc, deployHome(), deployHome(), deployHome(), deployHome())
	content = insertService(content, block)
	writeRunnersCompose(content)
	dockerComposeFile(runnersComposePath(), "up", "-d", svc)
	fmt.Printf("✓ Runner '%s' added (token: %s, url: %s)\n", svc, mask(token), forgejoURL)
}

func mask(s string) string {
	if len(s) <= 6 {
		return "***"
	}
	return s[:3] + "***" + s[len(s)-3:]
}

// ═══════════════════════════════════════════
//  Interactive
// ═══════════════════════════════════════════

func interactive() {
	fmt.Println("\n╔══════════════════════════════════════════╗")
	fmt.Println("║        Deploy Stack — Add new site       ║")
	fmt.Println("╚══════════════════════════════════════════╝")
	fmt.Println()

	name := prompt("  Site name: ")
	if name == "" {
		fmt.Println("  Error: name required")
		return
	}
	name = strings.TrimSpace(name)

	dir := prompt(fmt.Sprintf("  Code directory [/var/www/%s]: ", name))
	if dir == "" {
		dir = "/var/www/" + name
	}
	if _, err := os.Stat(dir); os.IsNotExist(err) {
		if strings.ToLower(prompt(fmt.Sprintf("  Create %s? [Y/n]: ", dir))) != "n" {
			os.MkdirAll(dir, 0755)
		} else {
			return
		}
	}

	lang := detectLang(dir)
	db := detectDB(dir)

	fmt.Printf("\n  Detected: %s\n", lang.Name)
	if db != "" {
		fmt.Printf("  Database: %s\n", filepath.Base(db))
	}

	autoP := freePort(lang.Port)
	portStr := prompt(fmt.Sprintf("  Port [%d auto]: ", autoP))
	port := autoP
	if portStr != "" && strings.ToLower(portStr) != "auto" {
		if p, err := strconv.Atoi(portStr); err == nil && p > 0 {
			port = p
		}
	}

	fmt.Printf("\n  ┌──────────────────────────────┐\n")
	fmt.Printf("  │ Name:  %-22s │\n", name)
	fmt.Printf("  │ Dir:   %-22s │\n", dir)
	fmt.Printf("  │ Lang:  %-22s │\n", lang.Name)
	fmt.Printf("  │ Port:  %-22d │\n", port)
	if db != "" {
		fmt.Printf("  │ DB:    %-22s │\n", filepath.Base(db))
	}
	fmt.Printf("  └──────────────────────────────┘\n")

	if strings.ToLower(prompt("\n  Deploy? [Y/n]: ")) == "n" {
		return
	}

	actual, err := deploySite(name, dir, port, lang)
	if err != nil {
		fmt.Printf("\n  ✗ %v\n", err)
		return
	}
	fmt.Printf("\n  ✓ %s deployed: http://%s:%d\n", name, hostIP(), actual)
}

// ═══════════════════════════════════════════
//  Main
// ═══════════════════════════════════════════

func usage() {
	fmt.Print(`deploy-stack — Universal site deployer

Usage:
  deploy                                  Авто-деплой текущего репозитория (CI) / интерактивно
  deploy --port N [--name N] [--type T]   Авто-деплой с явными параметрами
  deploy add <name> <dir> [--port N]      Добавить сайт (авто-определение языка)
  deploy rm <name>                        Удалить сайт
  deploy list                             Список сайтов с портами и статусом
  deploy runners                          Список раннеров
  deploy add-runner <name> <token>        Добавить Forgejo runner
  deploy rm-runner [name]                 Удалить раннер(ы)
  deploy logs <name> [--tail N]           Логи сайта
  deploy restart <name>                   Перезапустить сайт
  deploy status                           Контейнеры сайтов
  deploy up / down                        Поднять/остановить все сайты
  deploy serve --port 3000                Панель-менеджер (web UI + API)

Zero-config: в workflow достаточно label (runs-on) и порта:
  jobs:
    deploy:
      runs-on: web
      env: { PORT: 8084 }
      steps:
        - uses: actions/checkout@v4
        - run: deploy

Поддерживаемые языки: PHP, Python, Django, Flask, FastAPI, Node.js, TypeScript,
Bun, Deno, Go, Rust, Java/Kotlin, .NET/C#, Ruby, Elixir, Haskell, Lua, Zig,
Nim, Swift, C/C++, и любой статический сайт.
`)
}

func main() {
	args := os.Args[1:]

	if len(args) == 0 {
		if isCI() || !isTTY() {
			autoDeploy(nil)
		} else {
			interactive()
		}
		return
	}

	switch args[0] {
	case "add":
		if len(args) < 3 {
			fmt.Println("Usage: deploy add <name> <dir> [--port N] [--type T]")
			return
		}
		name, dir := args[1], args[2]
		opts := parseFlags(args[3:])
		lang := detectLang(dir)
		if t := opts["type"]; t != "" {
			if _, ok := templates[t]; ok {
				lang = LangInfo{t, t, defaultPorts[t], nil}
			}
		}
		port := 0
		if p := opts["port"]; p != "" {
			port, _ = strconv.Atoi(p)
		}
		if port == 0 {
			port = freePort(lang.Port)
		}
		actual, err := deploySite(name, dir, port, lang)
		if err != nil {
			fmt.Printf("✗ %v\n", err)
			os.Exit(1)
		}
		fmt.Printf("✓ %s deployed: http://%s:%d\n", name, hostIP(), actual)

	case "rm":
		if len(args) < 2 {
			fmt.Println("Usage: deploy rm <name>")
			return
		}
		n := serviceName(args[1])
		dockerCompose("down", "--remove-orphans", n)
		writeCompose(removeService(readCompose(), n))
		fmt.Printf("✓ %s removed\n", n)

	case "list":
		for _, s := range listServices() {
			st := getContainerStatus(s)
			p := containerHostPort(s)
			fmt.Printf("  %-24s port=%-6d %s\n", s, p, st)
		}

	case "runners":
		for _, r := range runnerNames() {
			fmt.Printf("  %-24s %s\n", r, getContainerStatus(r))
		}

	case "add-runner":
		if len(args) < 3 {
			fmt.Println("Usage: deploy add-runner <name> <token> [--url URL]")
			return
		}
		url := "http://10.0.0.1:3000"
		for i, a := range args {
			if a == "--url" && i+1 < len(args) {
				url = args[i+1]
			}
		}
		addRunner(args[1], args[2], url)

	case "rm-runner":
		n := ""
		if len(args) >= 2 {
			n = args[1]
		}
		if n == "" {
			for _, r := range runnerNames() {
				exec.Command("docker", "rm", "-f", r).Run()
			}
			writeRunnersCompose(defaultCompose())
		} else {
			svc := serviceName("runner-" + n)
			dockerComposeFile(runnersComposePath(), "down", svc)
			writeRunnersCompose(removeService(readRunnersCompose(), svc))
		}
		fmt.Println("✓ Runner removed")

	case "logs":
		if len(args) < 2 {
			fmt.Println("Usage: deploy logs <name> [--tail N]")
			return
		}
		tail := "200"
		for i, a := range args {
			if a == "--tail" && i+1 < len(args) {
				tail = args[i+1]
			}
		}
		cmd := exec.Command("docker", "logs", "--tail", tail, serviceName(args[1]))
		cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
		cmd.Run()

	case "restart":
		if len(args) < 2 {
			fmt.Println("Usage: deploy restart <name>")
			return
		}
		cmd := exec.Command("docker", "restart", serviceName(args[1]))
		cmd.Stdout, cmd.Stderr = os.Stdout, os.Stderr
		cmd.Run()

	case "status":
		dockerCompose("ps")

	case "up":
		if len(args) > 1 {
			dockerCompose("up", "-d", serviceName(args[1]))
		} else {
			dockerCompose("up", "-d")
		}

	case "down":
		dockerCompose("down", "--remove-orphans")

	case "deploy", "auto":
		autoDeploy(args[1:])

	case "serve":
		port := 3000
		for i, a := range args {
			if a == "--port" && i+1 < len(args) {
				port, _ = strconv.Atoi(args[i+1])
			}
		}
		startWebServer(port)

	case "help", "-h", "--help":
		usage()

	default:
		if strings.HasPrefix(args[0], "-") {
			autoDeploy(args)
		} else {
			usage()
		}
	}
}
