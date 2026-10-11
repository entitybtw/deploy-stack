// lang — универсальное определение языка/фреймворка проекта.
use std::path::Path;

/// LangInfo — результат детекта: имя, шаблон, дефолтный порт, доп. расширения (php).
#[derive(Clone, Debug)]
pub struct LangInfo {
    pub name: String,
    pub template: String,
    pub port: u16,
    pub extensions: Vec<String>,
}

impl LangInfo {
    pub fn new(name: &str, template: &str, port: u16, extensions: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            template: template.to_string(),
            port,
            extensions,
        }
    }
    /// from_type — принудительный тип из --type/TYPE.
    pub fn from_type(t: &str) -> Option<Self> {
        let port = match t {
            "static" | "php" | "go" => 8080,
            "python" => 8000,
            "node" => 3000,
            _ => return None,
        };
        Some(Self::new(t, t, port, Vec::new()))
    }
}

const SKIP_DIRS: &[&str] = &[
    "vendor",
    "node_modules",
    ".git",
    "__pycache__",
    ".venv",
    "dist",
    "build",
];

/// detectLang — обходит каталог и определяет язык по манифестам/расширениям.
pub fn detect_lang(dir: &str) -> LangInfo {
    let mut files: Vec<String> = Vec::new();
    walk_files(dir, &mut files, 0);

    let has = |ext: &str| files.iter().any(|f| f.ends_with(ext));
    let contains = |name: &str| files.iter().any(|f| f == name);

    if contains("Cargo.toml") {
        return LangInfo::new("rust", "rust", 8080, vec![]);
    }
    if contains("go.mod") {
        return LangInfo::new("go", "go", 8080, vec![]);
    }
    if contains("pom.xml") || contains("build.gradle") || contains("build.gradle.kts") {
        return LangInfo::new("jvm", "jvm", 8080, vec![]);
    }
    if contains("Gemfile") || has(".rb") {
        return LangInfo::new("ruby", "ruby", 3000, vec![]);
    }
    if contains("mix.exs") {
        return LangInfo::new("elixir", "elixir", 4000, vec![]);
    }
    if contains("package.json") {
        if contains("bun.lockb") || contains("bun.lock") {
            return LangInfo::new("bun", "bun", 3000, vec![]);
        }
        if has(".ts") || has(".tsx") {
            return LangInfo::new("node-ts", "node", 3000, vec![]);
        }
        return LangInfo::new("node", "node", 3000, vec![]);
    }
    if contains("deno.json") || contains("deno.jsonc") {
        return LangInfo::new("deno", "deno", 8000, vec![]);
    }
    if has(".py")
        || contains("requirements.txt")
        || contains("pyproject.toml")
        || contains("Pipfile")
        || contains("poetry.lock")
    {
        let mut framework = "python";
        if has(".py") {
            let content = read_file_first_n(dir, 50);
            if content.contains("django") || contains("manage.py") {
                framework = "django";
            }
            if content.contains("flask") {
                framework = "flask";
            }
            if content.contains("fastapi") {
                framework = "fastapi";
            }
        }
        return LangInfo::new(framework, framework, 8000, vec![]);
    }
    if has(".php") || contains("composer.json") || contains("index.php") {
        let mut exts = vec![
            "curl".to_string(),
            "mbstring".to_string(),
            "opcache".to_string(),
            "pdo_sqlite".to_string(),
        ];
        if has(".php") {
            let content = read_file_first_n(dir, 100);
            if content.contains("gd") || content.contains("imagecreate") {
                exts.push("gd".to_string());
            }
            if content.contains("redis") {
                exts.push("redis".to_string());
            }
        }
        return LangInfo::new("php", "php", 8080, exts);
    }
    if contains("stack.yaml") || has(".hs") {
        return LangInfo::new("haskell", "haskell", 8080, vec![]);
    }
    if has(".lua") {
        return LangInfo::new("lua", "lua", 8080, vec![]);
    }
    if contains("build.zig") {
        return LangInfo::new("zig", "zig", 8080, vec![]);
    }
    if has(".nim") {
        return LangInfo::new("nim", "nim", 8080, vec![]);
    }
    if contains("Package.swift") {
        return LangInfo::new("swift", "swift", 8080, vec![]);
    }
    if has(".c") || has(".cpp") || contains("CMakeLists.txt") {
        return LangInfo::new("c", "c", 8080, vec![]);
    }
    LangInfo::new("static", "static", 8080, vec![])
}

/// walk_files — рекурсивный обход имён файлов (с пропуском служебных каталогов).
fn walk_files(dir: &str, out: &mut Vec<String>, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_files(&e.path().to_string_lossy(), out, depth + 1);
        } else {
            out.push(name);
        }
    }
}

/// readFileFirstN — читает содержимое первых N файлов .py/.php/.js/.ts.
fn read_file_first_n(dir: &str, n: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    collect_sources(dir, n, &mut parts, 0);
    parts.join("\n")
}

fn collect_sources(dir: &str, n: usize, out: &mut Vec<String>, depth: usize) {
    if depth > 12 || out.len() >= n {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        if out.len() >= n {
            return;
        }
        let Ok(ft) = e.file_type() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            collect_sources(&e.path().to_string_lossy(), n, out, depth + 1);
        } else if name.ends_with(".py")
            || name.ends_with(".php")
            || name.ends_with(".js")
            || name.ends_with(".ts")
        {
            if let Ok(data) = std::fs::read_to_string(e.path()) {
                out.push(data);
            }
        }
    }
}

/// detectDB — ищет непустой .db/.sqlite/.sqlite3 (для интерактива).
pub fn detect_db(dir: &str) -> String {
    let mut found = String::new();
    walk_db(dir, &mut found, 0);
    found
}

fn walk_db(dir: &str, found: &mut String, depth: usize) {
    if depth > 12 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(md) = e.metadata() else { continue };
        let name = e.file_name().to_string_lossy().to_string();
        if md.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_db(&e.path().to_string_lossy(), found, depth + 1);
        } else if (name.ends_with(".db") || name.ends_with(".sqlite") || name.ends_with(".sqlite3"))
            && md.len() > 0
        {
            *found = e.path().to_string_lossy().to_string();
        }
    }
}

/// innerPort — порт внутри контейнера, на котором слушает приложение.
pub fn inner_port(lang: &LangInfo) -> u16 {
    match lang.name.as_str() {
        "static" | "php" => 80,
        "python" | "django" | "flask" | "fastapi" | "deno" => 8000,
        "node" | "node-ts" | "bun" | "ruby" => 3000,
        "elixir" => 4000,
        _ => 8080,
    }
}

/// path_exists — existence helper для одного файла.
pub fn file_exists(p: &str) -> bool {
    Path::new(p).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inner_port_defaults() {
        assert_eq!(inner_port(&LangInfo::new("static", "static", 8080, vec![])), 80);
        assert_eq!(inner_port(&LangInfo::new("php", "php", 8080, vec![])), 80);
        assert_eq!(inner_port(&LangInfo::new("node", "node", 3000, vec![])), 3000);
        assert_eq!(inner_port(&LangInfo::new("rust", "rust", 8080, vec![])), 8080);
        assert_eq!(inner_port(&LangInfo::new("python", "python", 8000, vec![])), 8000);
    }

    #[test]
    fn from_type_known() {
        assert!(LangInfo::from_type("static").is_some());
        assert!(LangInfo::from_type("php").is_some());
        assert!(LangInfo::from_type("unknown").is_none());
    }
}
