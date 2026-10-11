// compose — манипуляции с docker-compose (строковые, byte-for-byte с Go),
// имена сервисов, порты (вкл. новый синтаксис PORT=8282:8080).
use crate::config::{clean_path, compose_path, env_block, get_site_custom, site_volumes, site_volumes_block};
use crate::lang::{inner_port, LangInfo};
use crate::util::{cmd_output, cmd_output_timeout};
use std::collections::BTreeSet;
use std::path::Path;

/// PortSpec — разобранный порт: внешний (host) и опциональный внутренний (container).
/// Поддерживаемые формы: "8282" (только внешний), "8282:8080" (внешний:внутренний),
/// "0:8080" (внешний авто = 0). Невалидная строка → (0, None) → freePort.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortSpec {
    pub external: u16,
    pub internal: Option<u16>,
}

impl PortSpec {
    /// parse — разбор строки вида "8282" или "8282:8080".
    pub fn parse(s: &str) -> PortSpec {
        let s = s.trim();
        if s.is_empty() {
            return PortSpec { external: 0, internal: None };
        }
        if let Some((ext, int)) = s.split_once(':') {
            let ext = ext.trim().parse::<u16>().unwrap_or(0);
            let int = int.trim().parse::<u16>().ok().filter(|p| *p > 0);
            return PortSpec { external: ext, internal: int };
        }
        PortSpec {
            external: s.parse::<u16>().unwrap_or(0),
            internal: None,
        }
    }
    /// resolved_inner — внутренний порт: явный или дефолт языка.
    pub fn resolved_inner(&self, lang: &LangInfo) -> u16 {
        self.internal.unwrap_or_else(|| inner_port(lang))
    }
}

/// serviceName — валидное имя docker-сервиса: [a-z0-9_.-], без ведущих -._.
pub fn service_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let mut s = String::new();
    let mut last_dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '.' || c == '-' {
            s.push(c);
            last_dash = c == '-';
        } else {
            if !last_dash {
                s.push('-');
            }
            last_dash = true;
        }
    }
    let s = s.trim_start_matches(|c| c == '-' || c == '.' || c == '_');
    let s = s.trim_matches(|c| c == '-' || c == '.');
    if s.is_empty() {
        "site".to_string()
    } else {
        s.to_string()
    }
}

pub fn default_compose() -> String {
    "services: {}\nvolumes: {}\n".to_string()
}

pub fn read_compose() -> String {
    let p = compose_path();
    match std::fs::read_to_string(&p) {
        Ok(data) if !data.trim().is_empty() => data,
        _ => default_compose(),
    }
}

pub fn write_compose(c: &str) {
    let p = compose_path();
    let _ = std::fs::create_dir_all(Path::new(&p).parent().unwrap_or(Path::new(".")));
    let _ = std::fs::write(&p, c);
}

pub fn read_runners_compose() -> String {
    let p = crate::config::runners_compose_path();
    match std::fs::read_to_string(&p) {
        Ok(data) if !data.trim().is_empty() => data,
        _ => default_compose(),
    }
}

pub fn write_runners_compose(c: &str) {
    let p = crate::config::runners_compose_path();
    let _ = std::fs::create_dir_all(Path::new(&p).parent().unwrap_or(Path::new(".")));
    let _ = std::fs::write(&p, c);
}

/// insertService — вставка блока сервиса перед volumes:/в конец services.
pub fn insert_service(content: &str, block: &str) -> String {
    let block = block.trim_matches('\n');
    if content.contains("services: {}") {
        return content.replacen("services: {}", &format!("services:\n{}", block), 1);
    }
    let lines: Vec<&str> = content.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let unindented = !line.starts_with(' ') && !line.starts_with('\t');
        if unindented && (trimmed == "volumes:" || trimmed == "volumes: {}") {
            let mut out = lines[..i].join("\n");
            out.push('\n');
            out.push_str(block);
            out.push('\n');
            out.push_str(&lines[i..].join("\n"));
            return out;
        }
    }
    format!("{}\n{}\n", content.trim_end_matches('\n'), block)
}

/// removeService — вырезает блок сервиса по имени (+ суффиксы языков).
pub fn remove_service(content: &str, name: &str) -> String {
    let mut candidates: BTreeSet<String> = BTreeSet::new();
    candidates.insert(name.to_string());
    for suffix in [
        "-php", "-node", "-go", "-python", "-rust", "-jvm", "-dotnet", "-ruby", "-bun", "-deno",
        "-docker",
    ] {
        candidates.insert(format!("{}{}", name, suffix));
    }

    let mut out: Vec<String> = Vec::new();
    let mut in_services = false;
    let mut skip = false;
    for line in content.split('\n') {
        let ind = line.len() - line.trim_start_matches(|c| c == ' ' || c == '\t').len();
        let trimmed = line.trim();
        let is_blank = trimmed.is_empty();

        if ind == 0 {
            in_services = trimmed.starts_with("services");
            skip = false;
            out.push(line.to_string());
            continue;
        }

        if !in_services || is_blank {
            if is_blank && skip {
                continue;
            }
            out.push(line.to_string());
            continue;
        }

        if ind <= 2 {
            let key = trimmed.strip_suffix(':').unwrap_or(trimmed);
            if candidates.contains(key) {
                skip = true;
                continue;
            }
            skip = false;
        }

        if !skip {
            out.push(line.to_string());
        }
    }
    let cleaned = out.join("\n");
    cleaned.replace("services:\n\n  ", "services:\n  ")
}

/// listServices — имена сервисов из compose (уровень 2 пробела).
pub fn list_services() -> Vec<String> {
    let c = read_compose();
    let mut out: Vec<String> = Vec::new();
    for line in c.split('\n') {
        // ^  ([a-zA-Z0-9_.-]+):
        if !line.starts_with("  ") || line.starts_with("   ") {
            continue;
        }
        let rest = &line[2..];
        let mut name = String::new();
        for ch in rest.chars() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '.' || ch == '-' {
                name.push(ch);
            } else {
                break;
            }
        }
        if name.is_empty() || !rest[name.len()..].starts_with(':') {
            continue;
        }
        if matches!(name.as_str(), "services" | "volumes" | "networks") {
            continue;
        }
        out.push(name);
    }
    out
}

/// siteComposeInfo — dir и host-порт сервиса в compose (для apply кастомизации).
/// Дополнительно возвращает внутренний порт (из ports-строки), чтобы redeploy
/// сохранял маппинг.
pub fn site_compose_info(safe: &str) -> (String, u16, Option<u16>, bool) {
    let c = read_compose();
    let mut in_svc = false;
    let mut dir = String::new();
    let mut port: u16 = 0;
    let mut inner: Option<u16> = None;
    for line in c.split('\n') {
        let ind = line.len() - line.trim_start_matches(|c| c == ' ' || c == '\t').len();
        let trimmed = line.trim();
        if ind == 0 {
            in_svc = trimmed.starts_with("services");
            continue;
        }
        if !in_svc {
            continue;
        }
        if ind <= 2 {
            let key = trimmed.strip_suffix(':').unwrap_or(trimmed);
            if key == safe {
                in_svc = true;
            } else if trimmed.ends_with(':') && key != safe {
                if !dir.is_empty() || port != 0 {
                    break;
                }
                in_svc = false;
                continue;
            }
            continue;
        }
        if let Some(ctx) = trimmed.strip_prefix("context:") {
            dir = ctx.trim().to_string();
        }
        // ports: ["0.0.0.0:8084:80"] или ["8084:80"] — берём внешний и внутренний
        if port == 0 {
            if let Some((ext, int)) = parse_ports_line(trimmed) {
                if ext > 0 {
                    port = ext;
                    inner = int;
                }
            }
        }
        if dir.is_empty() && trimmed.contains(":/usr/share/nginx/html") {
            let src = trimmed.strip_prefix("- ").unwrap_or(trimmed);
            let src = src
                .strip_suffix(":/usr/share/nginx/html:ro")
                .or_else(|| src.strip_suffix(":/usr/share/nginx/html"))
                .unwrap_or(src);
            if src.starts_with('/') {
                dir = src.to_string();
            }
        }
        if dir.is_empty() && trimmed.contains(":/var/www/html") {
            let src = trimmed.strip_prefix("- ").unwrap_or(trimmed);
            let src = src
                .strip_suffix(":/var/www/html")
                .or_else(|| src.strip_suffix(":/var/www/html:ro"))
                .unwrap_or(src);
            if src.starts_with('/') {
                dir = src.to_string();
            }
        }
    }
    if dir.is_empty() {
        // fallback: docker inspect mounts
        let tpl = "{{range .Mounts}}{{if eq .Destination \"/var/www/html\"}}{{.Source}}{{end}}{{if eq .Destination \"/usr/share/nginx/html\"}}{{.Source}}{{end}}{{end}}";
        if let Some(out) = cmd_output("docker", &["inspect", "-f", tpl, safe]) {
            let t = out.trim().to_string();
            if !t.is_empty() {
                dir = t;
            }
        }
    }
    if port == 0 {
        port = container_host_port(safe);
    }
    let ok = !dir.is_empty();
    (dir, port, inner, ok)
}

/// parse_ports_line — разбор содержимого ports-строки compose:
/// "0.0.0.0:8282:8080" → (8282, Some(8080)); "8282:80" → (8282, Some(80)).
fn parse_ports_line(trimmed: &str) -> Option<(u16, Option<u16>)> {
    let start = trimmed.find('"')?;
    let end = trimmed[start + 1..].find('"')? + start + 1;
    let spec = &trimmed[start + 1..end];
    let parts: Vec<&str> = spec.split(':').collect();
    match parts.len() {
        2 => {
            let ext = parts[0].parse().ok()?;
            let int = parts[1].parse().ok()?;
            Some((ext, Some(int)))
        }
        3 => {
            let ext = parts[1].parse().ok()?;
            let int = parts[2].parse().ok()?;
            Some((ext, Some(int)))
        }
        _ => None,
    }
}

// ═══════════════════════════════════════════
//  Ports
// ═══════════════════════════════════════════

/// dockerHostPorts — реально опубликованные хост-порты (docker ps -a).
pub fn docker_host_ports() -> BTreeSet<u16> {
    let mut ports = BTreeSet::new();
    let Some(out) = cmd_output("docker", &["ps", "-a", "--format", "{{.Ports}}"]) else {
        return ports;
    };
    // :(\d+)->
    let b = out.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b':' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && b[j..].starts_with(b"->") {
                if let Ok(p) = out[i + 1..j].parse::<u16>() {
                    ports.insert(p);
                }
            }
        }
        i += 1;
    }
    ports
}

/// usedPorts — порты из docker ps + compose-файла.
pub fn used_ports() -> BTreeSet<u16> {
    let mut ports = docker_host_ports();
    if let Ok(data) = std::fs::read_to_string(compose_path()) {
        // (\d+):\d+"
        let b = data.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_digit()) {
                let mut j = i;
                while j < b.len() && b[j].is_ascii_digit() {
                    j += 1;
                }
                if j < b.len() && b[j] == b':' {
                    let mut k = j + 1;
                    while k < b.len() && b[k].is_ascii_digit() {
                        k += 1;
                    }
                    if k < b.len() && b[k] == b'"' {
                        if let Ok(p) = data[i..j].parse::<u16>() {
                            ports.insert(p);
                        }
                    }
                }
            }
            i += 1;
        }
    }
    ports
}

/// freePort — первый свободный порт начиная с start.
pub fn free_port(start: u16) -> u16 {
    let used = used_ports();
    let mut p = start;
    while used.contains(&p) {
        p += 1;
    }
    p
}

/// containerHostPort — фактический host-порт запущенного сервиса (0 если нет).
pub fn container_host_port(name: &str) -> u16 {
    let tpl = "{{range $p, $conf := .NetworkSettings.Ports}}{{range $conf}}{{.HostPort}} {{end}}{{end}}";
    let Some(out) = cmd_output("docker", &["inspect", "-f", tpl, name]) else {
        return 0;
    };
    for f in out.split_whitespace() {
        if let Ok(p) = f.parse::<u16>() {
            if p > 0 {
                return p;
            }
        }
    }
    0
}

/// runnerNames — реальные контейнеры раннеров (docker ps).
pub fn runner_names() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Some(raw) = cmd_output("docker", &["ps", "-a", "--format", "{{.Names}}"]) else {
        return out;
    };
    for n in raw.split_whitespace() {
        if n == "runner" || n.starts_with("runner-") {
            out.push(n.to_string());
        }
    }
    out
}

// ═══════════════════════════════════════════
//  Service block (генерация compose-секции)
// ═══════════════════════════════════════════

/// buildServiceBlock — генерация YAML-блока сервиса сайта.
/// inner_override — внутренний порт из PortSpec (если задан явно).
pub fn build_service_block(
    name: &str,
    dir: &str,
    port: u16,
    lang: &LangInfo,
    inner_override: Option<u16>,
) -> String {
    let safe = service_name(name);
    let bind = {
        let b = std::env::var("DEPLOY_BIND").unwrap_or_default();
        if b.is_empty() {
            "0.0.0.0".to_string()
        } else {
            b
        }
    };
    let own_dockerfile = Path::new(dir).join("Dockerfile").exists();
    let custom = get_site_custom(&safe);
    let extra_vols = site_volumes(&safe);
    let inner = inner_override.unwrap_or_else(|| inner_port(lang));

    match lang.name.as_str() {
        "static" => {
            let base_vols = vec![
                format!("{}:/usr/share/nginx/html:ro", dir),
                format!("{}/.deploy.nginx.conf:/etc/nginx/conf.d/default.conf:ro", dir),
            ];
            let vol_block = site_volumes_block(&base_vols, &extra_vols);
            if !custom.tools.is_empty() {
                let env = env_block(&safe, &[]);
                return format!(
                    "  {s}:\n    build:\n      context: {d}\n      dockerfile: .deploy.Dockerfile\n    container_name: {s}\n    restart: unless-stopped\n    ports: [\"{b}:{p}:{i}\"]\n{e}{v}    labels:\n      - \"deploy-stack.site={n}\"\n",
                    s = safe, d = dir, b = bind, p = port, i = inner, e = env, v = vol_block, n = name
                );
            }
            format!(
                "  {s}:\n    image: nginx:alpine\n    container_name: {s}\n    restart: unless-stopped\n    ports: [\"{b}:{p}:{i}\"]\n{e}{v}    labels:\n      - \"deploy-stack.site={n}\"\n",
                s = safe, b = bind, p = port, i = inner, e = env_block(&safe, &[]), v = vol_block, n = name
            )
        }
        "php" => {
            let vol_block = site_volumes_block(
                &[format!("{}:/var/www/html", dir), format!("{}:/var/www/{}", dir, name)],
                &extra_vols,
            );
            format!(
                "  {s}:\n    build:\n      context: {d}\n      dockerfile: .deploy.Dockerfile\n    container_name: {s}\n    restart: unless-stopped\n    ports: [\"{b}:{p}:{i}\"]\n{e}{v}    labels:\n      - \"deploy-stack.site={n}\"\n",
                s = safe, d = dir, b = bind, p = port, i = inner, e = env_block(&safe, &[]), v = vol_block, n = name
            )
        }
        "rust" => {
            let df = if own_dockerfile && custom.tools.is_empty() {
                "Dockerfile"
            } else {
                ".deploy.Dockerfile"
            };
            let vol_block =
                site_volumes_block(&[format!("{}:/var/www/{}", dir, name)], &extra_vols);
            let env = env_block(
                &safe,
                &[
                    ("PORT", inner.to_string()),
                    ("UST_ROOT", format!("/var/www/{}", name)),
                    ("UST_DB", format!("/var/www/{}/database.sqlite", name)),
                    ("UST_UPLOAD", format!("/var/www/{}/upload", name)),
                    ("UST_TEMPLATES", format!("/var/www/{}/templates", name)),
                ],
            );
            format!(
                "  {s}:\n    build:\n      context: {d}\n      dockerfile: {df}\n    container_name: {s}\n    restart: unless-stopped\n    ports: [\"{b}:{p}:{i}\"]\n{e}{v}    labels:\n      - \"deploy-stack.site={n}\"\n",
                s = safe, d = dir, b = bind, p = port, i = inner, e = env, v = vol_block, n = name
            )
        }
        _ => {
            let df = if own_dockerfile && custom.tools.is_empty() {
                "Dockerfile"
            } else {
                ".deploy.Dockerfile"
            };
            let vol_block = site_volumes_block(&[], &extra_vols);
            let env = env_block(&safe, &[("PORT", inner.to_string())]);
            format!(
                "  {s}:\n    build:\n      context: {d}\n      dockerfile: {df}\n    container_name: {s}\n    restart: unless-stopped\n    ports: [\"{b}:{p}:{i}\"]\n{e}{v}    labels:\n      - \"deploy-stack.site={n}\"\n",
                s = safe, d = dir, b = bind, p = port, i = inner, e = env, v = vol_block, n = name
            )
        }
    }
}

/// clean_path_pub — экспорт для deploy.rs.
pub fn clean(p: &str) -> String {
    clean_path(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_name_normalizes() {
        assert_eq!(service_name("My Site!"), "my-site");
        assert_eq!(service_name("-weird-"), "weird");
        assert_eq!(service_name("..."), "site");
        assert_eq!(service_name("ok.name_1"), "ok.name_1");
    }

    #[test]
    fn port_spec_forms() {
        assert_eq!(
            PortSpec::parse("8282"),
            PortSpec { external: 8282, internal: None }
        );
        assert_eq!(
            PortSpec::parse("8282:8080"),
            PortSpec { external: 8282, internal: Some(8080) }
        );
        assert_eq!(
            PortSpec::parse(" 8095:80 "),
            PortSpec { external: 8095, internal: Some(80) }
        );
        assert_eq!(PortSpec::parse(""), PortSpec { external: 0, internal: None });
        assert_eq!(PortSpec::parse("junk"), PortSpec { external: 0, internal: None });
        assert_eq!(PortSpec::parse("8282:junk"), PortSpec { external: 8282, internal: None });
    }

    #[test]
    fn port_spec_resolved_inner() {
        let lang = LangInfo::new("node", "node", 3000, vec![]);
        assert_eq!(PortSpec::parse("8282").resolved_inner(&lang), 3000);
        assert_eq!(PortSpec::parse("8282:8080").resolved_inner(&lang), 8080);
    }

    #[test]
    fn insert_and_remove_service() {
        let base = default_compose();
        let block = "  site1:\n    image: nginx:alpine\n    container_name: site1\n";
        let c = insert_service(&base, block);
        assert!(c.contains("  site1:"));
        let services = {
            // list_services читает из compose_path — проверяем на строке напрямую
            let mut out = Vec::new();
            for line in c.split('\n') {
                if line.starts_with("  ") && !line.starts_with("   ") && line.contains(':') {
                    let name: String = line[2..]
                        .chars()
                        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '.' || *c == '-')
                        .collect();
                    if !name.is_empty() {
                        out.push(name);
                    }
                }
            }
            out
        };
        assert!(services.contains(&"site1".to_string()));
        let removed = remove_service(&c, "site1");
        assert!(!removed.contains("site1:"));
    }

    #[test]
    fn remove_service_with_suffix() {
        // remove_service("panel") удаляет и panel, и panel-php (суффикс-кандидат),
        // но не трогает несвязанные сервисы.
        let c = "services:\n  panel:\n    image: x\n  panel-php:\n    image: y\n  other:\n    image: z\nvolumes: {}\n";
        let out = remove_service(c, "panel");
        assert!(!out.contains("panel-php"));
        assert!(!out.contains("panel:"));
        assert!(!out.contains("image: x"));
        assert!(!out.contains("image: y"));
        assert!(out.contains("    image: z"));
    }

    #[test]
    fn build_service_block_inner_override() {
        let lang = LangInfo::new("node", "node", 3000, vec![]);
        let b = build_service_block("demo", "/tmp/demo", 8282, &lang, Some(8080));
        assert!(b.contains("ports: [\"0.0.0.0:8282:8080\"]"));
        assert!(b.contains("PORT=8080"));
        // без override — дефолт языка
        let b2 = build_service_block("demo", "/tmp/demo", 8282, &lang, None);
        assert!(b2.contains("ports: [\"0.0.0.0:8282:3000\"]"));
        assert!(b2.contains("PORT=3000"));
        // static: внутренний 80
        let st = LangInfo::new("static", "static", 8080, vec![]);
        let b3 = build_service_block("demo", "/tmp/demo", 8095, &st, None);
        assert!(b3.contains("ports: [\"0.0.0.0:8095:80\"]"));
        assert!(b3.contains("image: nginx:alpine"));
    }

    #[test]
    fn parse_ports_line_forms() {
        assert_eq!(
            parse_ports_line("ports: [\"0.0.0.0:8282:8080\"]"),
            Some((8282, Some(8080)))
        );
        assert_eq!(parse_ports_line("ports: [\"8282:80\"]"), Some((8282, Some(80))));
        assert_eq!(parse_ports_line("image: nginx"), None);
    }
}
