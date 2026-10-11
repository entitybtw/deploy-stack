// config — пути, каталоги, site-custom.json, .deploy-конфиг, тома.
use crate::compose::service_name;
use crate::util::{env_or, get_env};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// deployHome — рабочий каталог менеджера: sites/, templates/, static/.
/// По умолчанию — каталог исполняемого файла; в контейнере DEPLOY_HOME.
pub fn deploy_home() -> String {
    let h = std::env::var("DEPLOY_HOME").unwrap_or_default();
    if !h.is_empty() {
        return clean_path(&h);
    }
    // Go: filepath.Clean(filepath.Dir(os.Args[0])) — точный паритет.
    if let Some(a0) = std::env::args().next() {
        let p = Path::new(&a0);
        let d = p.parent().unwrap_or(Path::new(""));
        let d = d.to_string_lossy();
        if !d.is_empty() {
            return clean_path(&d);
        }
        return ".".to_string();
    }
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("./deploy"));
    clean_path(exe.parent().unwrap_or(Path::new(".")).to_str().unwrap_or("."))
}

/// clean_path — аналог filepath.Clean для абсолютных/относительных путей.
pub fn clean_path(p: &str) -> String {
    let absolute = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for part in p.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            x => out.push(x),
        }
    }
    let joined = out.join("/");
    if absolute {
        format!("/{}", joined)
    } else if joined.is_empty() {
        ".".to_string()
    } else {
        joined
    }
}

/// composePath — файл compose сервисов сайтов. Fallback, если родительский
/// каталог смонтирован (split-brain иначе: панель и раннер правят разные файлы).
pub fn compose_path() -> String {
    let p = std::env::var("DEPLOY_SITES").unwrap_or_default();
    if p.is_empty() {
        return Path::new(&deploy_home())
            .join("sites")
            .join("docker-compose.yml")
            .to_string_lossy()
            .to_string();
    }
    let p = clean_path(&p);
    if let Some(parent) = Path::new(&p).parent() {
        if parent.is_dir() {
            return p;
        }
    }
    let fallback = Path::new(&deploy_home())
        .join("sites")
        .join("docker-compose.yml")
        .to_string_lossy()
        .to_string();
    if clean_path(&fallback) == p {
        return p;
    }
    let _ = std::fs::create_dir_all(Path::new(&fallback).parent().unwrap_or(Path::new(".")));
    fallback
}

/// runnersComposePath — файл compose CI-раннеров.
pub fn runners_compose_path() -> String {
    let p = std::env::var("DEPLOY_RUNNERS").unwrap_or_default();
    if !p.is_empty() {
        return clean_path(&p);
    }
    Path::new(&deploy_home())
        .join("runners")
        .join("docker-compose.yml")
        .to_string_lossy()
        .to_string()
}

/// templatesSearch — кандидаты каталогов шаблонов (парity с Go-версией:
/// symlink-снятый exe, типовые места установки).
pub fn templates_search() -> Vec<String> {
    let mut cands: Vec<String> = Vec::new();
    let mut add = |p: String, cands: &mut Vec<String>| {
        let p = clean_path(&p);
        if !cands.contains(&p) {
            cands.push(p);
        }
    };
    add(
        Path::new(&deploy_home())
            .join("templates")
            .to_string_lossy()
            .to_string(),
        &mut cands,
    );
    if let Ok(exe) = std::env::current_exe() {
        // current_exe уже снят symlink (Linux /proc/self/exe)
        if let Some(dir) = exe.parent() {
            add(dir.join("templates").to_string_lossy().to_string(), &mut cands);
        }
    }
    if let Some(a0) = std::env::args().next() {
        if let Some(dir) = Path::new(&a0).parent() {
            add(dir.join("templates").to_string_lossy().to_string(), &mut cands);
        }
    }
    add(
        "/usr/local/share/deploy-stack/templates".to_string(),
        &mut cands,
    );
    add("/root/deploy-stack/templates".to_string(), &mut cands);
    if let Ok(h) = std::env::var("DEPLOY_HOME") {
        if !h.is_empty() {
            add(Path::new(&h).join("templates").to_string_lossy().to_string(), &mut cands);
        }
    }
    cands
}

/// resolveTemplatesDir — первая существующая templates-папка.
pub fn resolve_templates_dir() -> String {
    let cands = templates_search();
    for d in &cands {
        if Path::new(d).is_dir() {
            return d.clone();
        }
    }
    cands
        .into_iter()
        .next()
        .unwrap_or_else(|| "/usr/local/share/deploy-stack/templates".to_string())
}

/// runnerDataExists — есть ли зарегистрированный раннер в каталоге.
fn runner_data_exists(root: &str) -> bool {
    for p in [
        Path::new(root).join(".runner"),
        Path::new(root).join("data").join(".runner"),
        Path::new(root).join("runner").join(".runner"),
    ] {
        if p.exists() {
            return true;
        }
    }
    false
}

/// runnerDataCandidates — типовые каталоги данных раннеров.
fn runner_data_candidates() -> Vec<String> {
    let mut c = vec![
        "/root/forgejo-runner".to_string(),
        "/root/forgejo-runner/data".to_string(),
        "/runner".to_string(),
        "/runner/data".to_string(),
    ];
    if let Ok(h) = std::env::var("DEPLOY_HOME") {
        if !h.is_empty() {
            c.push(Path::new(&h).join("runner").to_string_lossy().to_string());
            c.push(
                Path::new(&h)
                    .join("runner")
                    .join("data")
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }
    let dh = deploy_home();
    c.push(Path::new(&dh).join("runner").to_string_lossy().to_string());
    c.push(
        Path::new(&dh)
            .join("runner")
            .join("data")
            .to_string_lossy()
            .to_string(),
    );
    c
}

/// runnerDataRoot — корень данных Forgejo-раннеров (config.yaml/.runner).
pub fn runner_data_root() -> String {
    if let Ok(h) = std::env::var("RUNNER_DATA") {
        if !h.is_empty() {
            let h = clean_path(&h);
            if runner_data_exists(&h) {
                return h;
            }
            for c in runner_data_candidates() {
                if runner_data_exists(&c) {
                    return c;
                }
            }
            return h;
        }
    }
    for c in runner_data_candidates() {
        if runner_data_exists(&c) {
            return c;
        }
    }
    "/root/forgejo-runner".to_string()
}

/// runnerConfigFile — путь .runner-файла конкретного раннера.
pub fn runner_config_file(container_name: &str) -> String {
    let mut roots: Vec<String> = Vec::new();
    if let Ok(h) = std::env::var("RUNNER_DATA") {
        if !h.is_empty() {
            roots.push(clean_path(&h));
        }
    }
    roots.extend(runner_data_candidates());
    let mut candidates: Vec<String> = Vec::new();
    for r in &roots {
        if container_name == "runner" {
            candidates.push(Path::new(r).join(".runner").to_string_lossy().to_string());
            candidates.push(
                Path::new(r)
                    .join("data")
                    .join(".runner")
                    .to_string_lossy()
                    .to_string(),
            );
            candidates.push(
                Path::new(r)
                    .join("runner")
                    .join(".runner")
                    .to_string_lossy()
                    .to_string(),
            );
        } else {
            candidates.push(
                Path::new(r)
                    .join("data")
                    .join(container_name)
                    .join(".runner")
                    .to_string_lossy()
                    .to_string(),
            );
            candidates.push(
                Path::new(r)
                    .join(container_name)
                    .join(".runner")
                    .to_string_lossy()
                    .to_string(),
            );
            candidates.push(
                Path::new(r)
                    .join("runner")
                    .join(container_name)
                    .join(".runner")
                    .to_string_lossy()
                    .to_string(),
            );
        }
    }
    for c in &candidates {
        if Path::new(c).exists() {
            return c.clone();
        }
    }
    candidates
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// extraVolumesForSite — rw-bind тома из DEPLOY_SITE_VOLUMES/site-volumes.conf.
pub fn extra_volumes_for_site(safe: &str) -> Vec<String> {
    let mut raw = env_or(&["DEPLOY_SITE_VOLUMES"]);
    if raw.is_empty() {
        let p = Path::new(&deploy_home()).join("site-volumes.conf");
        if let Ok(data) = std::fs::read_to_string(&p) {
            raw = data;
        }
    }
    let mut out: Vec<String> = Vec::new();
    for line in raw.split('\n') {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let sep = line.find(['=', '|']);
        let Some(sep) = sep else { continue };
        if sep == 0 {
            continue;
        }
        let key = service_name(line[..sep].trim());
        if key != safe {
            continue;
        }
        for vol in line[sep + 1..].split(',') {
            let vol = vol.trim();
            if !vol.is_empty() {
                out.push(vol.to_string());
            }
        }
    }
    out
}

// ═══════════════════════════════════════════
//  SiteCustom (site-custom.json)
// ═══════════════════════════════════════════

/// SiteCustom — кастомизация контейнера сайта: apk-тулы, env, доп. тома.
#[derive(Serialize, Deserialize, Default, Clone)]
pub struct SiteCustom {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
}

pub fn site_custom_path() -> String {
    Path::new(&deploy_home())
        .join("site-custom.json")
        .to_string_lossy()
        .to_string()
}

pub fn load_site_customs() -> BTreeMap<String, SiteCustom> {
    let mut out = BTreeMap::new();
    if let Ok(data) = std::fs::read_to_string(site_custom_path()) {
        if let Ok(m) = serde_json::from_str::<BTreeMap<String, SiteCustom>>(&data) {
            out = m;
        }
    }
    out
}

pub fn get_site_custom(safe: &str) -> SiteCustom {
    load_site_customs().remove(safe).unwrap_or_default()
}

pub fn save_site_custom(safe: &str, c: SiteCustom) -> Result<(), String> {
    let mut all = load_site_customs();
    if c.tools.is_empty() && c.env.is_empty() && c.volumes.is_empty() {
        all.remove(safe);
    } else {
        all.insert(safe.to_string(), c);
    }
    let data = serde_json::to_string_pretty(&all).map_err(|e| e.to_string())?;
    std::fs::write(site_custom_path(), data).map_err(|e| e.to_string())
}

/// siteVolumes — штатные тома + тома из site-custom.json.
pub fn site_volumes(safe: &str) -> Vec<String> {
    let mut out = extra_volumes_for_site(safe);
    out.extend(get_site_custom(safe).volumes);
    out
}

/// envBlock — рендер блока environment (4 пробела) или "".
/// base дописывается к пользовательским (например PORT); порядок: base, затем custom.
pub fn env_block(safe: &str, base: &[(&str, String)]) -> String {
    let custom = get_site_custom(safe).env;
    if custom.is_empty() && base.is_empty() {
        return String::new();
    }
    let mut keys: Vec<String> = base.iter().map(|(k, _)| k.to_string()).collect();
    keys.sort();
    let mut ck: Vec<String> = custom
        .keys()
        .filter(|k| !base.iter().any(|(bk, _)| bk == k))
        .cloned()
        .collect();
    ck.sort();
    let mut b = String::from("    environment:\n");
    for k in &keys {
        let v = base
            .iter()
            .find(|(bk, _)| bk == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        b.push_str(&format!("      - {}={}\n", k, v));
    }
    for k in &ck {
        b.push_str(&format!("      - {}={}\n", k, custom[k]));
    }
    b
}

/// siteVolumesBlock — рендер блока volumes (4 пробела) или "".
pub fn site_volumes_block(base: &[String], extra: &[String]) -> String {
    let mut all: Vec<String> = base.to_vec();
    all.extend_from_slice(extra);
    if all.is_empty() {
        return String::new();
    }
    let mut b = String::from("    volumes:\n");
    for v in &all {
        b.push_str(&format!("      - {}\n", v));
    }
    b
}

/// loadDeployConfig — читает .deploy.env / deploy.env / .deploy (KEY=VALUE или KEY: VALUE).
pub fn load_deploy_config(dir: &str) -> BTreeMap<String, String> {
    let mut cfg = BTreeMap::new();
    for f in [".deploy.env", "deploy.env", ".deploy"] {
        let Ok(data) = std::fs::read_to_string(Path::new(dir).join(f)) else {
            continue;
        };
        for line in data.split('\n') {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some(sep) = line.find(['=', ':']) else {
                continue;
            };
            if sep == 0 {
                continue;
            }
            let k = line[..sep].trim().to_uppercase();
            let v = line[sep + 1..]
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_string();
            cfg.insert(k, v);
        }
    }
    cfg
}

/// parseFlags — разбор флагов вида --key value / --key=value / -key.
pub fn parse_flags(args: &[String]) -> BTreeMap<String, String> {
    let mut opts = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        if !a.starts_with('-') {
            i += 1;
            continue;
        }
        let a = a.trim_start_matches('-').to_string();
        if let Some(eq) = a.find('=') {
            if eq > 0 {
                opts.insert(a[..eq].to_string(), a[eq + 1..].to_string());
                i += 1;
                continue;
            }
        }
        if i + 1 < args.len() && !args[i + 1].starts_with('-') {
            opts.insert(a, args[i + 1].clone());
            i += 2;
        } else {
            opts.insert(a, "true".to_string());
            i += 1;
        }
    }
    opts
}

/// hostIP — адрес для «deployed» URL.
pub fn host_ip() -> String {
    let v = env_or(&["DEPLOY_HOST"]);
    if v.is_empty() {
        "0.0.0.0".to_string()
    } else {
        v
    }
}

/// ciRepoName — имя репозитория из CI-переменных.
pub fn ci_repo_name() -> String {
    for k in ["GITHUB_REPOSITORY", "CI_PROJECT_PATH", "GITEA_REPOSITORY", "DEPLOY_REPO"] {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                return v.rsplit('/').next().unwrap_or(&v).to_string();
            }
        }
    }
    String::new()
}

/// templates — известные типы (для --type).
pub fn templates() -> &'static [(&'static str, &'static str)] {
    &[
        ("static", "Static HTML/CSS/JS (nginx)"),
        ("php", "PHP 8.2 (nginx + php-fpm)"),
        ("python", "Python 3.12 (gunicorn)"),
        ("node", "Node.js 20"),
        ("go", "Go (compiled binary)"),
    ]
}

/// defaultPorts — дефолтные порты по типу.
pub fn default_port(typ: &str) -> u16 {
    match typ {
        "static" | "php" | "go" => 8080,
        "python" => 8000,
        "node" => 3000,
        _ => 8080,
    }
}

pub fn env_or_empty(key: &str) -> String {
    get_env(key, "")
}
