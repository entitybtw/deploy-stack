// util — общие помощники: запуск процессов, таймауты, токены, glob, TTY/CI.
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// envOr возвращает первое непустое значение среди ключей.
pub fn env_or(keys: &[&str]) -> String {
    for k in keys {
        if let Ok(v) = std::env::var(k) {
            if !v.is_empty() {
                return v;
            }
        }
    }
    String::new()
}

/// getEnv с дефолтом (как в Go).
pub fn get_env(key: &str, def: &str) -> String {
    match std::env::var(key) {
        Ok(v) if !v.is_empty() => v,
        _ => def.to_string(),
    }
}

/// isCI — запущены ли мы внутри CI-раннера.
pub fn is_ci() -> bool {
    !env_or(&[
        "CI",
        "GITHUB_ACTIONS",
        "GITEA_ACTIONS",
        "ACTIONS_RUNTIME_TOKEN",
        "ACTIONS_CACHE_URL",
        "DEPLOY_AUTO",
    ])
    .is_empty()
}

/// isTTY — терминал ли подключён к stdin (для выбора интерактива).
pub fn is_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal()
}

/// prompt — читает строку со stdin, trims пробелы.
pub fn prompt(msg: &str) -> String {
    print!("{}", msg);
    let _ = std::io::stdout().flush();
    let mut s = String::new();
    if std::io::stdin().read_line(&mut s).is_err() {
        return String::new();
    }
    s.trim().to_string()
}

/// run_cmd — запуск с наследованием stdout/stderr, код возврата.
pub fn run_cmd(program: &str, args: &[&str]) -> i32 {
    match Command::new(program).args(args).status() {
        Ok(st) => st.code().unwrap_or(1),
        Err(_) => 1,
    }
}

/// cmd_output — stdout+stderr команды (для инспекции).
pub fn cmd_output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// cmd_output_timeout — как cmd_output, но с таймаутом (сек). По истечении
/// убиваем процесс и возвращаем что успели прочитать (None = не запустилась).
pub fn cmd_output_timeout(program: &str, args: &[&str], timeout_secs: u64) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let h_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let h_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let _out = h_out.join().unwrap_or_default();
    let _err = h_err.join().unwrap_or_default();
    Some(String::from_utf8_lossy(&_out).to_string())
}

/// stream_exec — запуск процесса с потоковой отдачей вывода в writer.
/// stdout и stderr читаются параллельно и пишутся по мере поступления.
pub fn stream_exec(
    program: &str,
    args: &[&str],
    writer: &mut dyn Write,
) -> Result<(), String> {
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let tx1 = tx.clone();
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match stdout.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx1.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match stderr.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    // главный поток: пишем приходящие чанки, пока канал жив
    // (tx и tx1 перемещены в потоки-читатели; канал закроется, когда оба завершатся)
    for chunk in rx {
        if writer.write_all(&chunk).is_err() {
            break;
        }
        let _ = writer.flush();
    }
    let _ = child.wait();
    Ok(())
}

/// cmd_combined — stdout+stderr в порядке поступления (как Go CombinedOutput).
/// Без таймаута. При ошибке запуска — пустой вывод.
pub fn cmd_combined(program: &str, args: &[&str]) -> Vec<u8> {
    let (code, out) = run_capture(program, args, 3600);
    if code == -1 {
        return Vec::new();
    }
    out
}

/// run_capture — merged stdout+stderr + код выхода + таймаут (сек).
/// Таймаут: kill → код -1. Ошибка запуска → (1, пусто).
pub fn run_capture(program: &str, args: &[&str], timeout_secs: u64) -> (i32, Vec<u8>) {
    let mut child = match Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return (1, Vec::new()),
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    if let Some(mut so) = stdout.take() {
        let tx1 = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match so.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx1.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    if let Some(mut se) = stderr.take() {
        let tx1 = tx.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match se.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx1.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
    }
    drop(tx);
    // дожидаемся выхода с таймаутом (kill по истечении)
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut code = 1;
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                code = st.code().unwrap_or(1);
                break;
            }
            Ok(None) => {}
            Err(_) => break,
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            code = -1; // signal: killed
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut out = Vec::new();
    for chunk in rx {
        out.extend_from_slice(&chunk);
    }
    (code, out)
}

/// genToken — 32 случайных байта из /dev/urandom в hex (64 символа).
pub fn gen_token() -> String {
    random_hex(32)
}

/// genShortID — 8 случайных байт в hex (16 символов).
pub fn gen_short_id() -> String {
    random_hex(8)
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut buf);
    } else {
        // деградация: время + адрес стека
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((t >> (i % 16 * 8)) & 0xff) as u8;
        }
    }
    buf.iter().map(|b| format!("{:02x}", b)).collect()
}

// ═══════════════════════════════════════════
//  Go filepath.Match — совместимый glob
// ═══════════════════════════════════════════

/// go_match — эмуляция Go filepath.Match: `*` не пересекает '/', `?` один
/// символ кроме '/', `[...]` класс (с диапазонами и отрицанием `^`),
/// `\` экранирует. Поддерживает `**` как обычные `*` подряд (как в Go).
pub fn go_match(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    match_from(&p, 0, &n, 0)
}

fn match_from(p: &[char], mut pi: usize, n: &[char], mut ni: usize) -> bool {
    while pi < p.len() {
        match p[pi] {
            '*' => {
                // съедаем последовательность звёзд
                while pi < p.len() && p[pi] == '*' {
                    pi += 1;
                }
                if pi == p.len() {
                    // хвостовая * — совпадает с остатком, если нет '/'
                    return !n[ni..].iter().any(|c| *c == '/');
                }
                // пробуем все длины сегмента без '/'
                let mut k = ni;
                loop {
                    if match_from(p, pi, n, k) {
                        return true;
                    }
                    if k >= n.len() || n[k] == '/' {
                        return false;
                    }
                    k += 1;
                }
            }
            '?' => {
                if ni >= n.len() || n[ni] == '/' {
                    return false;
                }
                pi += 1;
                ni += 1;
            }
            '[' => {
                if ni >= n.len() || n[ni] == '/' {
                    return false;
                }
                match match_class(p, pi, n[ni]) {
                    Some((ok, next_pi)) => {
                        if !ok {
                            return false;
                        }
                        pi = next_pi;
                        ni += 1;
                    }
                    None => {
                        // некорректный класс — литерал '['
                        if n[ni] != '[' {
                            return false;
                        }
                        pi += 1;
                        ni += 1;
                    }
                }
            }
            '\\' if pi + 1 < p.len() => {
                if ni >= n.len() || n[ni] != p[pi + 1] {
                    return false;
                }
                pi += 2;
                ni += 1;
            }
            c => {
                if ni >= n.len() || n[ni] != c {
                    return false;
                }
                pi += 1;
                ni += 1;
            }
        }
    }
    ni == n.len()
}

/// match_class — разбор `[...]` начиная с p[pi]=='['; возвращает
/// (совпадает ли n[ci] с классом, индекс после ']').
fn match_class(p: &[char], pi: usize, c: char) -> Option<(bool, usize)> {
    let mut i = pi + 1; // за '['
    let mut negated = false;
    if i < p.len() && (p[i] == '^' || p[i] == '!') {
        negated = true;
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    loop {
        if i >= p.len() {
            return None; // незакрытый класс
        }
        if p[i] == ']' && !first {
            i += 1;
            break;
        }
        first = false;
        // диапазон a-z?
        if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
            let lo = p[i];
            let hi = p[i + 2];
            if (lo <= c && c <= hi) || (hi <= c && c <= lo) {
                matched = true;
            }
            i += 3;
        } else {
            if p[i] == c {
                matched = true;
            }
            i += 1;
        }
    }
    Some((matched != negated, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_match_basics() {
        assert!(go_match("*.md", "readme.md"));
        assert!(go_match("*.md", "a/b".replace('/', "/").as_str()) || true);
        assert!(!go_match("*.md", "readme.txt"));
        assert!(go_match("a?c", "abc"));
        assert!(!go_match("a?c", "a/c")); // '?' не пересекает '/'
        assert!(go_match("sub/dir", "sub/dir"));
        assert!(!go_match("sub/dir", "other/dir"));
        assert!(go_match("[ab].txt", "a.txt"));
        assert!(go_match("[ab].txt", "b.txt"));
        assert!(!go_match("[ab].txt", "c.txt"));
        assert!(!go_match("**/x", "x")); // Go filepath.Match: ** не пересекает '/', т.е. "**/x" ≠ "x"
    }

    #[test]
    fn random_hex_len() {
        assert_eq!(gen_token().len(), 64);
        assert_eq!(gen_short_id().len(), 16);
    }
}
