// sync — копирование исходников в /var/www/<name>: rsync или copyTree-фоллбек.
// SYNC_EXCLUDE из .deploy добавляет собственные исключения поверх базовых.
use crate::config::load_deploy_config;
use crate::util::go_match;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// syncExcludeTokens — токены SYNC_EXCLUDE как есть (разделены пробелами).
pub fn sync_exclude_tokens(cfg: &BTreeMap<String, String>) -> Vec<String> {
    let v = cfg.get("SYNC_EXCLUDE").map(|s| s.trim()).unwrap_or("");
    if v.is_empty() {
        return Vec::new();
    }
    v.split_whitespace().map(|s| s.to_string()).collect()
}

/// rsyncArgs — аргументы rsync: базовый список + доп. токены, затем пути.
pub fn rsync_args(src: &str, dst: &str, extra: &[String]) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-a".into(),
        "--delete".into(),
        "--exclude=.git".into(),
        "--exclude=*.db".into(),
        "--exclude=*.sqlite".into(),
        "--exclude=*.sqlite3".into(),
        "--exclude=*.db-wal".into(),
        "--exclude=*.db-shm".into(),
        "--exclude=*.sqlite-wal".into(),
        "--exclude=*.sqlite-shm".into(),
        "--exclude=.deploy.Dockerfile".into(),
        "--exclude=target/".into(),
        "--exclude=.env".into(),
        "--exclude=*.env".into(),
        "--exclude=.env.*".into(),
        "--exclude=data/".into(),
        "--exclude=uploads/".into(),
        "--exclude=upload/".into(),
        "--exclude=.ssh/".into(),
        "--exclude=cache/".into(),
        // Ведущий / = только корень репозитория (иначе съедало вложенные шаблоны).
        "--exclude=/privacy.html".into(),
        "--exclude=/terms.html".into(),
        "--exclude=/site_data.js".into(),
        "--exclude=/site_config.json".into(),
        "--exclude=/access_requests.json".into(),
        "--exclude=goserver/".into(),
    ];
    for t in extra {
        args.push(format!("--exclude={}", t));
    }
    args.push(format!("{}/", src));
    args.push(format!("{}/", dst));
    args
}

/// syncDir — копирует исходники в dst (rsync, иначе copyTree).
pub fn sync_dir(src: &str, dst: &str) -> Result<(), String> {
    let _ = std::fs::create_dir_all(dst);
    let cfg = load_deploy_config(src);
    let extra = sync_exclude_tokens(&cfg);
    if which("rsync").is_some() {
        let args = rsync_args(src, dst, &extra);
        let argrefs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let out = std::process::Command::new("rsync")
            .args(&argrefs)
            .output()
            .map_err(|e| format!("rsync: {}", e))?;
        if !out.status.success() {
            return Err(format!(
                "rsync: exit {:?}: {}",
                out.status.code(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        return Ok(());
    }
    copy_tree(src, dst, &extra)
}

/// which — путь к исполняемому файлу в PATH.
fn which(prog: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        let p = Path::new(dir).join(prog);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// matchSyncExclude — эмуляция rsync --exclude для copyTree:
/// токен с завершающим "/" матчит только директории; без внутреннего "/" —
/// basename на любой глубине; со внутренним "/" — от корня репозитория.
pub fn match_sync_exclude(rel: &str, is_dir: bool, tokens: &[String]) -> bool {
    if tokens.is_empty() {
        return false;
    }
    let rel = rel.replace('\\', "/");
    for tok in tokens {
        let dir_only = tok.ends_with('/');
        let pat = tok.strip_suffix('/').unwrap_or(tok);
        if pat.is_empty() || (dir_only && !is_dir) {
            continue;
        }
        if pat.contains('/') {
            // anchored: матчим относительно корня репозитория
            let pat = pat.strip_prefix('/').unwrap_or(pat);
            if go_match(pat, &rel) {
                return true;
            }
            continue;
        }
        let base = rel.rsplit('/').next().unwrap_or(&rel);
        if go_match(pat, base) {
            return true;
        }
    }
    false
}

/// copyTree — фоллбек syncDir без rsync. extra — токены SYNC_EXCLUDE.
pub fn copy_tree(src: &str, dst: &str, extra: &[String]) -> Result<(), String> {
    fn persistent(rel: &str) -> bool {
        let base = rel.rsplit('/').next().unwrap_or(rel);
        match base {
            ".env" | ".git" | ".deploy.Dockerfile" | "cache" | "access_requests.json"
            | "site_data.js" | "site_config.json" | "privacy.html" | "terms.html"
            | "goserver" | "target" => return true,
            _ => {}
        }
        if base.ends_with(".db")
            || base.ends_with(".sqlite")
            || base.ends_with(".sqlite3")
            || base.ends_with(".db-wal")
            || base.ends_with(".db-shm")
            || base.ends_with(".sqlite-wal")
            || base.ends_with(".sqlite-shm")
            || base.ends_with(".env")
        {
            return true;
        }
        rel == "data"
            || rel.starts_with("data/")
            || rel == "uploads"
            || rel.starts_with("uploads/")
            || rel == "upload"
            || rel.starts_with("upload/")
            || rel == ".ssh"
            || rel.starts_with(".ssh/")
    }
    copy_walk(Path::new(src), Path::new(src), Path::new(dst), extra, &persistent)
}

fn copy_walk(
    root: &Path,
    p: &Path,
    dst: &Path,
    extra: &[String],
    persistent: &dyn Fn(&str) -> bool,
) -> Result<(), String> {
    let rel_raw = p.strip_prefix(root).unwrap_or(p);
    let rel = rel_raw.to_string_lossy().replace('\\', "/");
    if rel == "." || rel.is_empty() {
        // сам корень: обойдём детей
        if let Ok(rd) = std::fs::read_dir(p) {
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                copy_walk(root, &e.path(), dst, extra, persistent)?;
            }
        }
        return Ok(());
    }
    let Ok(ft) = std::fs::symlink_metadata(p).map(|m| m.file_type()) else {
        return Ok(());
    };
    let is_dir = ft.is_dir();
    if persistent(&rel) || match_sync_exclude(&rel, is_dir, extra) {
        return Ok(());
    }
    let out = dst.join(&rel);
    if is_dir {
        let _ = std::fs::create_dir_all(&out);
        if let Ok(rd) = std::fs::read_dir(p) {
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                copy_walk(root, &e.path(), dst, extra, persistent)?;
            }
        }
    } else {
        let _ = std::fs::create_dir_all(out.parent().unwrap_or(dst));
        if let Ok(data) = std::fs::read(p) {
            let _ = std::fs::write(&out, data);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tempdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "ds-test-{}-{}-{}",
            tag,
            std::process::id(),
            crate::util::gen_short_id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn write_tree(root: &Path, files: &[(&str, &str)]) {
        for (rel, content) in files {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
    }

    fn assert_exists(root: &Path, rel: &str, want: bool) {
        let exist = root.join(rel).exists();
        assert_eq!(exist, want, "{}: exists={} want={}", rel, exist, want);
    }

    // ── SYNC_EXCLUDE: разбор .deploy ──────────────────────────────

    #[test]
    fn sync_exclude_tokens_basic() {
        let want: Vec<String> = ["src/", "Cargo.toml", "Cargo.lock", "Dockerfile", "*.md", ".forgejo/"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "SYNC_EXCLUDE".to_string(),
            "src/ Cargo.toml Cargo.lock Dockerfile *.md .forgejo/".to_string(),
        );
        assert_eq!(sync_exclude_tokens(&cfg), want);
    }

    #[test]
    fn sync_exclude_tokens_no_transformations() {
        // Токены уходят как есть: без смены регистра, без срезания / и глобов.
        let want: Vec<String> = ["SRC/", "README.MD", "**/x", "[ab].txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "SYNC_EXCLUDE".to_string(),
            "SRC/ README.MD **/x [ab].txt".to_string(),
        );
        assert_eq!(sync_exclude_tokens(&cfg), want);
    }

    #[test]
    fn sync_exclude_tokens_missing_or_empty() {
        let mut cfg = BTreeMap::new();
        cfg.insert("NAME".to_string(), "x".to_string());
        assert!(sync_exclude_tokens(&cfg).is_empty(), "нет ключа → пусто");
        let mut cfg2 = BTreeMap::new();
        cfg2.insert("SYNC_EXCLUDE".to_string(), "   ".to_string());
        assert!(sync_exclude_tokens(&cfg2).is_empty(), "пустое значение → пусто");
    }

    #[test]
    fn load_deploy_config_sync_exclude() {
        let dir = tempdir("cfg");
        fs::write(
            dir.join(".deploy"),
            "NAME=foo\nPORT=8084\nSYNC_EXCLUDE=src/ Cargo.toml *.md .forgejo/\n",
        )
        .unwrap();
        let cfg = load_deploy_config(&dir.to_string_lossy());
        let want: Vec<String> = ["src/", "Cargo.toml", "*.md", ".forgejo/"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(sync_exclude_tokens(&cfg), want);
        assert_eq!(cfg.get("NAME").map(|s| s.as_str()), Some("foo"));
        assert_eq!(cfg.get("PORT").map(|s| s.as_str()), Some("8084"));
        let _ = fs::remove_dir_all(&dir);
    }

    // ── rsync-аргументы ───────────────────────────────────────────

    #[test]
    fn rsync_args_base_unchanged() {
        let want: Vec<String> = [
            "-a", "--delete",
            "--exclude=.git", "--exclude=*.db", "--exclude=*.sqlite", "--exclude=*.sqlite3",
            "--exclude=*.db-wal", "--exclude=*.db-shm",
            "--exclude=*.sqlite-wal", "--exclude=*.sqlite-shm",
            "--exclude=.deploy.Dockerfile",
            "--exclude=target/",
            "--exclude=.env", "--exclude=*.env", "--exclude=.env.*",
            "--exclude=data/", "--exclude=uploads/", "--exclude=upload/", "--exclude=.ssh/",
            "--exclude=cache/",
            "--exclude=/privacy.html", "--exclude=/terms.html",
            "--exclude=/site_data.js", "--exclude=/site_config.json",
            "--exclude=/access_requests.json",
            "--exclude=goserver/",
            "/repo/", "/var/www/site/",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(rsync_args("/repo", "/var/www/site", &[]), want);
    }

    #[test]
    fn rsync_args_extra_excludes() {
        let extra: Vec<String> = ["src/", "Cargo.toml", "Cargo.lock", "Dockerfile", "*.md", ".forgejo/"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let got = rsync_args("/repo", "/var/www/site", &extra);
        for tok in &extra {
            assert!(
                got.iter().any(|a| a == &format!("--exclude={}", tok)),
                "нет --exclude={} в {:?}",
                tok,
                got
            );
        }
        let base = rsync_args("/repo", "/var/www/site", &[]);
        assert_eq!(got.len(), base.len() + extra.len());
        let first_extra = got
            .iter()
            .position(|a| extra.iter().any(|t| a == &format!("--exclude={}", t)))
            .unwrap();
        let last_base = got.iter().position(|a| a == "--exclude=goserver/").unwrap();
        assert!(first_extra > last_base, "extra не после базового списка");
        assert_eq!(got[got.len() - 2], "/repo/");
        assert_eq!(got[got.len() - 1], "/var/www/site/");
    }

    #[test]
    fn rsync_args_no_delete_excluded() {
        for args in [
            rsync_args("/repo", "/dst", &[]),
            rsync_args(
                "/repo",
                "/dst",
                &["src/".to_string(), "*.md".to_string()],
            ),
        ] {
            for a in &args {
                assert!(!a.contains("delete-excluded"), "не должно быть: {}", a);
            }
        }
    }

    // ── matchSyncExclude ──────────────────────────────────────────

    #[test]
    fn match_sync_exclude_cases() {
        let t = |s: &[&str]| -> Vec<String> { s.iter().map(|x| x.to_string()).collect() };
        // dirOnly — только директории, на любой глубине
        assert!(match_sync_exclude("src", true, &t(&["src/"])));
        assert!(!match_sync_exclude("src", false, &t(&["src/"])));
        assert!(match_sync_exclude("a/b/src", true, &t(&["src/"])));
        // без внутреннего / — basename на любой глубине
        assert!(match_sync_exclude("Cargo.toml", false, &t(&["Cargo.toml"])));
        assert!(match_sync_exclude("sub/Cargo.toml", false, &t(&["Cargo.toml"])));
        assert!(!match_sync_exclude("Cargo.lock.bak", false, &t(&["Cargo.toml"])));
        // глоб
        assert!(match_sync_exclude("docs/readme.md", false, &t(&["*.md"])));
        assert!(match_sync_exclude("README.md", false, &t(&["*.md"])));
        assert!(!match_sync_exclude("docs/readme.txt", false, &t(&["*.md"])));
        // с внутренним / — от корня репозитория
        assert!(match_sync_exclude("sub/dir", true, &t(&["sub/dir"])));
        assert!(!match_sync_exclude("other/dir", true, &t(&["sub/dir"])));
        assert!(match_sync_exclude("sub/dir", true, &t(&["sub/dir/"])));
        assert!(!match_sync_exclude("sub/dir", false, &t(&["sub/dir/"])));
        assert!(!match_sync_exclude("sub", true, &t(&["sub/dir"])));
        // ведущий / = корень
        assert!(match_sync_exclude("x.txt", false, &t(&["/x.txt"])));
        assert!(!match_sync_exclude("a/x.txt", false, &t(&["/x.txt"])));
        // пусто
        assert!(!match_sync_exclude("src", true, &[]));
    }

    // ── copyTree с SYNC_EXCLUDE ───────────────────────────────────

    #[test]
    fn copy_tree_sync_exclude() {
        let src = tempdir("ct-src");
        let dst = tempdir("ct-dst");
        write_tree(
            &src,
            &[
                ("keep.txt", "ok"),
                ("skip.md", "no"),
                ("Cargo.toml", "no"),
                ("Cargo.lock", "no"),
                ("Dockerfile", "no"),
                ("src/main.rs", "no"),
                ("deep/nested/x.md", "no"),
                ("deep/keep.txt", "ok"),
                (".forgejo/ci.yml", "no"),
                (".env", "base-excluded"),
                (".git/config", "base-excluded"),
            ],
        );
        let mut cfg = BTreeMap::new();
        cfg.insert(
            "SYNC_EXCLUDE".to_string(),
            "src/ Cargo.toml Cargo.lock Dockerfile *.md .forgejo/".to_string(),
        );
        let extra = sync_exclude_tokens(&cfg);
        copy_tree(
            &src.to_string_lossy(),
            &dst.to_string_lossy(),
            &extra,
        )
        .unwrap();

        assert_exists(&dst, "keep.txt", true);
        assert_exists(&dst, "deep/keep.txt", true);
        assert_exists(&dst, "skip.md", false);
        assert_exists(&dst, "Cargo.toml", false);
        assert_exists(&dst, "Cargo.lock", false);
        assert_exists(&dst, "Dockerfile", false);
        assert_exists(&dst, "src", false);
        assert_exists(&dst, "deep/nested/x.md", false);
        assert_exists(&dst, ".forgejo", false);
        // базовые исключения по-прежнему работают
        assert_exists(&dst, ".env", false);
        assert_exists(&dst, ".git", false);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn copy_tree_no_extra_unchanged() {
        let src = tempdir("ct2-src");
        let dst = tempdir("ct2-dst");
        write_tree(
            &src,
            &[
                ("keep.txt", "ok"),
                ("note.md", "copied: без SYNC_EXCLUDE ничего не фильтруется сверх базы"),
                (".env", "base-excluded"),
            ],
        );
        copy_tree(&src.to_string_lossy(), &dst.to_string_lossy(), &[]).unwrap();
        assert_exists(&dst, "keep.txt", true);
        assert_exists(&dst, "note.md", true);
        assert_exists(&dst, ".env", false);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    // ── syncDir end-to-end ────────────────────────────────────────

    #[test]
    fn sync_dir_reads_deploy_exclude() {
        let src = tempdir("sd-src");
        let dst = std::env::temp_dir().join(format!("sd-www-{}", crate::util::gen_short_id()));
        write_tree(
            &src,
            &[
                (".deploy", "SYNC_EXCLUDE=src/ Cargo.toml *.md .forgejo/\n"),
                ("index.html", "<h1>ok</h1>"),
                ("readme.md", "no"),
                ("Cargo.toml", "no"),
                ("src/lib.rs", "no"),
                ("docs/a.md", "no"),
                ("docs/api.txt", "ok"),
                (".env", "base-excluded"),
            ],
        );
        sync_dir(&src.to_string_lossy(), &dst.to_string_lossy()).unwrap();

        assert_exists(&dst, "index.html", true);
        assert_exists(&dst, "docs/api.txt", true);
        assert_exists(&dst, "readme.md", false);
        assert_exists(&dst, "Cargo.toml", false);
        assert_exists(&dst, "src", false);
        assert_exists(&dst, "docs/a.md", false);
        assert_exists(&dst, ".env", false);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn sync_dir_without_deploy_exclude() {
        let src = tempdir("sd2-src");
        let dst = std::env::temp_dir().join(format!("sd2-www-{}", crate::util::gen_short_id()));
        write_tree(
            &src,
            &[
                (".deploy", "NAME=plain\n"),
                ("index.html", "<h1>ok</h1>"),
                ("readme.md", "copied"),
                ("Cargo.toml", "copied"),
                (".env", "base-excluded"),
            ],
        );
        sync_dir(&src.to_string_lossy(), &dst.to_string_lossy()).unwrap();

        assert_exists(&dst, "index.html", true);
        assert_exists(&dst, "readme.md", true);
        assert_exists(&dst, "Cargo.toml", true);
        assert_exists(&dst, ".env", false);
        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }
}
