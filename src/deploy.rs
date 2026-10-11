// deploy — подготовка сборки, деплой сайта, миграции, раннеры, авто-деплой.
use crate::compose::*;
use crate::config::*;
use crate::lang::*;
use crate::sync::sync_dir;
use crate::util::*;
use std::collections::BTreeMap;
use std::path::Path;

/// dockerCompose — docker compose -f <sites compose> args (код возврата).
pub fn docker_compose(args: &[&str]) -> i32 {
    docker_compose_file(&compose_path(), args)
}

/// dockerComposeFile — docker compose -f path args.
pub fn docker_compose_file(path: &str, args: &[&str]) -> i32 {
    let mut full: Vec<String> = vec!["compose".into(), "-f".into(), path.into()];
    full.extend(args.iter().map(|a| a.to_string()));
    let argrefs: Vec<&str> = full.iter().map(|s| s.as_str()).collect();
    match std::process::Command::new("docker")
        .args(&argrefs)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .status()
    {
        Ok(st) => st.code().unwrap_or(1),
        Err(_) => 1,
    }
}

/// prepareBuild — пишет .deploy.Dockerfile из шаблона (для собираемых языков).
pub fn prepare_build(dir: &str, lang: &LangInfo, safe: &str) -> Result<(), String> {
    let custom = get_site_custom(safe);
    if lang.name == "static" {
        if custom.tools.is_empty() {
            return Ok(());
        }
        let df = format!("FROM nginx:alpine\n{}\n", tools_run_line(&custom.tools));
        return std::fs::write(Path::new(dir).join(".deploy.Dockerfile"), df)
            .map_err(|e| e.to_string());
    }
    // Свой Dockerfile репозитория — только если нет tools (кроме php).
    if lang.name != "php" && custom.tools.is_empty() && Path::new(dir).join("Dockerfile").exists() {
        return Ok(());
    }
    let tpl = Path::new(&resolve_templates_dir())
        .join(&lang.template)
        .join("Dockerfile");
    let mut data = std::fs::read_to_string(&tpl)
        .map_err(|_| format!("шаблон {} не найден", lang.template))?;
    if !custom.tools.is_empty() {
        data = inject_tools(&data, &custom.tools);
    }
    let di = Path::new(dir).join(".dockerignore");
    if !di.exists() {
        let _ = std::fs::write(
            &di,
            ".git\nnode_modules\n__pycache__\n.venv\ndist\nbuild\n*.db\n*.sqlite\n*.sqlite3\n*.db-wal\n*.db-shm\n*.zip\n",
        );
    }
    std::fs::write(Path::new(dir).join(".deploy.Dockerfile"), data).map_err(|e| e.to_string())
}

fn tools_run_line(tools: &[String]) -> String {
    format!("RUN apk add --no-cache {}", tools.join(" "))
}

/// injectTools — вставляет RUN apk add сразу после первой строки FROM.
fn inject_tools(dockerfile: &str, tools: &[String]) -> String {
    let line = tools_run_line(tools);
    let lines: Vec<&str> = dockerfile.split('\n').collect();
    for (i, l) in lines.iter().enumerate() {
        if l.trim().to_uppercase().starts_with("FROM ") {
            let mut out: Vec<&str> = Vec::new();
            out.extend_from_slice(&lines[..i + 1]);
            out.push(&line);
            out.extend_from_slice(&lines[i + 1..]);
            return out.join("\n");
        }
    }
    format!("{}\n{}\n", dockerfile.trim_end_matches('\n'), line)
}

/// prepareStaticNginx — кладёт .deploy.nginx.conf в папку сайта.
pub fn prepare_static_nginx(dir: &str) -> Result<(), String> {
    let tpl = Path::new(&resolve_templates_dir())
        .join("static")
        .join("nginx.conf");
    let data = std::fs::read_to_string(&tpl).map_err(|e| e.to_string())?;
    std::fs::write(Path::new(dir).join(".deploy.nginx.conf"), data).map_err(|e| e.to_string())
}

/// runMigrations — миграции внутри контейнера (sh-скрипт или artisan).
pub fn run_migrations(svc: &str, dir: &str, name: &str) {
    let base = format!("/var/www/{}", name);
    for s in ["install/migrate.sh", "migrate.sh", "db/migrate.sh"] {
        if Path::new(dir).join(s).exists() {
            println!("  migration: {}", s);
            let script = format!("{}/{}", base, s);
            let _ = run_cmd("docker", &["exec", svc, "sh", &script]);
            return;
        }
    }
    if Path::new(dir).join("artisan").exists() {
        println!("  migration: php artisan migrate --force");
        let artisan = format!("{}/artisan", base);
        let _ = run_cmd("docker", &["exec", svc, "php", &artisan, "migrate", "--force"]);
    }
}

/// deploySite — создаёт/обновляет сервис сайта и поднимает его.
/// inner — внутренний порт из PortSpec (если был задан явно).
pub fn deploy_site(name: &str, dir: &str, port: u16, lang: &LangInfo, inner: Option<u16>) -> Result<u16, String> {
    if lang.name == "static" {
        prepare_static_nginx(dir).map_err(|e| format!("nginx-конфиг: {}", e))?;
    }
    let safe = service_name(name);
    prepare_build(dir, lang, &safe)?;
    let mut c = read_compose();
    c = remove_service(&c, &safe);
    let block = build_service_block(name, dir, port, lang, inner);
    c = insert_service(&c, &block);
    write_compose(&c);

    let code = docker_compose(&["up", "-d", "--build", &safe]);
    if code != 0 {
        return Err(format!("docker compose завершился с ошибкой для {}", safe));
    }
    run_migrations(&safe, dir, name);

    let actual = container_host_port(&safe);
    let actual = if actual == 0 { port } else { actual };
    Ok(actual)
}

/// removeSite — останавливает и удаляет ОДИН сервис (без --remove-orphans!).
pub fn remove_site(n: &str) {
    docker_compose(&["rm", "-s", "-f", n]);
    let _ = run_cmd("docker", &["rm", "-f", n]);
    write_compose(&remove_service(&read_compose(), n));
}

pub fn remove_all_sites() {
    docker_compose(&["down"]);
}

// ═══════════════════════════════════════════
//  Runner management
// ═══════════════════════════════════════════

pub fn add_runner(name: &str, token: &str, forgejo_url: &str) {
    let mut svc = "runner".to_string();
    if !name.is_empty() {
        svc = service_name(&format!("runner-{}", name));
    }
    let mut content = read_runners_compose();
    content = remove_service(&content, &svc);
    let dh = deploy_home();
    let block = format!(
        "  {svc}:\n    image: data.forgejo.org/forgejo/runner:4.0.0\n    container_name: {svc}\n    user: root\n    restart: unless-stopped\n    volumes:\n      - {rdr}/{svc}:/data\n      - /var/run/docker.sock:/var/run/docker.sock\n      - /var/www:/var/www\n      - {dh}:{dh}\n    environment:\n      DEPLOY_HOME: {dh}\n    command: >\n      sh -c \"apk update && apk add --no-cache docker-cli docker-cli-compose rsync nodejs npm &&\n             ln -sf {dh}/deploy /usr/local/bin/deploy &&\n             forgejo-runner daemon --config /data/config.yaml\"\n",
        svc = svc,
        rdr = runner_data_root(),
        dh = dh
    );
    content = insert_service(&content, &block);
    write_runners_compose(&content);
    docker_compose_file(&runners_compose_path(), &["up", "-d", &svc]);
    println!(
        "✓ Runner '{}' added (token: {}, url: {})",
        svc,
        mask(token),
        forgejo_url
    );
}

pub fn mask(s: &str) -> String {
    if s.len() <= 6 {
        return "***".to_string();
    }
    format!("{}***{}", &s[..3], &s[s.len() - 3..])
}

// ═══════════════════════════════════════════
//  Auto-deploy (CI)
// ═══════════════════════════════════════════

/// autoDeploy — «умкий» режим CI: минимум ввода, остальное само.
pub fn auto_deploy(args: &[String]) {
    let opts = parse_flags(args);

    let mut dir = env_or(&["DEPLOY_DIR", "GITHUB_WORKSPACE"]);
    if let Some(v) = opts.get("dir") {
        dir = v.clone();
    }
    if dir.is_empty() {
        dir = std::env::current_dir()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
    }
    dir = crate::config::clean_path(&abs_path(&dir));

    let cfg = load_deploy_config(&dir);

    let mut name = env_or(&["DEPLOY_SITE"]);
    if let Some(v) = opts.get("name") {
        name = v.clone();
    }
    if name.is_empty() {
        name = cfg.get("NAME").cloned().unwrap_or_default();
    }
    if name.is_empty() {
        name = ci_repo_name();
    }
    if name.is_empty() {
        name = Path::new(&dir)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
    }
    name = name.trim().to_string();

    let mut typ = env_or(&["DEPLOY_TYPE"]);
    if let Some(v) = opts.get("type") {
        typ = v.clone();
    }
    if typ.is_empty() {
        typ = cfg.get("TYPE").cloned().unwrap_or_default();
    }

    let mut lang: Option<LangInfo> = None;
    if !typ.is_empty() {
        lang = LangInfo::from_type(&typ);
    }
    let lang = lang.unwrap_or_else(|| detect_lang(&dir));

    // порт: приоритет flag > env DEPLOY_PORT/PORT > .deploy PORT; поддержка 8282:8080.
    // Если порт не задан явно — «липкий»: переиспользуем уже привязанный порт сервиса,
    // чтобы ре-деплой не менял его (иначе freePort дёргал порт каждый раз).
    let mut port_str = env_or(&["DEPLOY_PORT", "PORT"]);
    if let Some(v) = opts.get("port") {
        port_str = v.clone();
    }
    if port_str.is_empty() {
        port_str = cfg.get("PORT").cloned().unwrap_or_default();
    }
    let spec = PortSpec::parse(&port_str);
    let (port, inner) = if spec.external == 0 {
        sticky_port(&name, lang.port)
    } else {
        (spec.external, spec.internal)
    };

    let target = format!("/var/www/{}", name);
    if crate::config::clean_path(&dir) != crate::config::clean_path(&target) {
        if let Err(e) = sync_dir(&dir, &target) {
            println!("  ! синхронизация: {}", e);
        }
    }
    let mut src_dir = target.clone();
    if !Path::new(&target).is_dir() {
        src_dir = dir.clone();
    }

    println!("• {} ({}), каталог: {}", name, lang.name, src_dir);
    match deploy_site(&name, &src_dir, port, &lang, inner) {
        Ok(actual) => println!(
            "✓ {} задеплоен: http://{}:{}",
            name,
            host_ip(),
            actual
        ),
        Err(e) => {
            println!("✗ {}: {}", name, e);
            std::process::exit(1);
        }
    }
}

/// absPath — аналог filepath.Abs.
fn abs_path(p: &str) -> String {
    if p.starts_with('/') {
        return p.to_string();
    }
    let cwd = std::env::current_dir()
        .map(|c| c.to_string_lossy().to_string())
        .unwrap_or_default();
    format!("{}/{}", cwd.trim_end_matches('/'), p)
}

// ═══════════════════════════════════════════
//  Interactive
// ═══════════════════════════════════════════

pub fn interactive() {
    println!("\n╔══════════════════════════════════════════╗");
    println!("║        Deploy Stack — Add new site       ║");
    println!("╚══════════════════════════════════════════╝");
    println!();

    let name = prompt("  Site name: ");
    if name.is_empty() {
        println!("  Error: name required");
        return;
    }
    let name = name.trim().to_string();

    let mut dir = prompt(&format!("  Code directory [/var/www/{}]: ", name));
    if dir.is_empty() {
        dir = format!("/var/www/{}", name);
    }
    if !Path::new(&dir).exists() {
        if prompt(&format!("  Create {}? [Y/n]: ", dir)).to_lowercase() != "n" {
            let _ = std::fs::create_dir_all(&dir);
        } else {
            return;
        }
    }

    let lang = detect_lang(&dir);
    let db = detect_db(&dir);

    println!("\n  Detected: {}", lang.name);
    if !db.is_empty() {
        println!(
            "  Database: {}",
            Path::new(&db).file_name().unwrap_or_default().to_string_lossy()
        );
    }

    // «липкий»: при повторном добавлении существующего сервиса предлагаем его порт
    let suggested = sticky_port(&name, lang.port);
    let port_str = prompt(&format!("  Port [{} auto, можно 8282:8080]: ", suggested.0));
    let spec = PortSpec::parse(&port_str);
    let (port, inner) = if spec.external == 0 {
        suggested
    } else {
        (spec.external, spec.internal)
    };

    println!("\n  ┌──────────────────────────────┐");
    println!("  │ Name:  {:<22} │", name);
    println!("  │ Dir:   {:<22} │", dir);
    println!("  │ Lang:  {:<22} │", lang.name);
    match inner {
        Some(i) => println!("  │ Port:  {:<14} (вн:вн) │", format!("{}:{}", port, i)),
        None => println!("  │ Port:  {:<22} │", port),
    }
    if !db.is_empty() {
        println!(
            "  │ DB:    {:<22} │",
            Path::new(&db).file_name().unwrap_or_default().to_string_lossy()
        );
    }
    println!("  └──────────────────────────────┘");

    if prompt("\n  Deploy? [Y/n]: ").to_lowercase() == "n" {
        return;
    }

    match deploy_site(&name, &dir, port, &lang, inner) {
        Ok(actual) => println!("\n  ✓ {} deployed: http://{}:{}", name, host_ip(), actual),
        Err(e) => println!("\n  ✗ {}", e),
    }
}

// ═══════════════════════════════════════════
//  CLI
// ═══════════════════════════════════════════

pub fn get_container_status(name: &str) -> String {
    cmd_output_timeout("docker", &["inspect", "-f", "{{.State.Status}}", name], 3)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "stopped".to_string())
}

pub fn usage() {
    print!(
        r#"deploy-stack — Universal site deployer

Usage:
  deploy                                  Авто-деплой текущего репозитория (CI) / интерактивно
  deploy --port N[:M] [--name N] [--type T]  Авто-деплой с явными параметрами
  deploy add <name> <dir> [--port N[:M]]  Добавить сайт (авто-определение языка)
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

Порт: внешний и внутренний — через двоеточие:
  PORT=8282:8080     # внешний 8282, внутренний 8080 (.deploy / env / --port)
  --port 8282:8080   # то же флагом
  PORT=8095          # только внешний (внутренний — дефолт языка)

Zero-config: в workflow достаточно label (runs-on) и порта:
  jobs:
    deploy:
      runs-on: web
      env: {{ PORT: "8095:80" }}
      steps:
        - uses: actions/checkout@v4
        - run: deploy

Поддерживаемые языки: PHP, Python, Django, Flask, FastAPI, Node.js, TypeScript,
Bun, Deno, Go, Rust, Java/Kotlin, .NET/C#, Ruby, Elixir, Haskell, Lua, Zig,
Nim, Swift, C/C++, и любой статический сайт.
"#
    );
}
