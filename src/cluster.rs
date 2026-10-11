// cluster — кластер нод: cluster.json (атомарно), HTTP-клиент с логином,
// агрегатный статус, прокси действий (exec/logs — со стримингом).
use crate::util::{gen_short_id, gen_token};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

/// Server — запись ноды в cluster.json (совместимо с Go-форматом).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub user: String,
    #[serde(default)]
    pub pass: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub token: String,
    #[serde(default)]
    pub online: bool,
    #[serde(default = "default_ping")]
    pub last_ping: String,
}

fn default_ping() -> String {
    // нулевой time.Time в Go: "0001-01-01T00:00:00Z"
    "0001-01-01T00:00:00Z".to_string()
}

/// now_rfc3339 — текущее время в RFC3339 (UTC) для last_ping.
fn now_rfc3339() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    civil_to_rfc3339(d.as_secs() as i64)
}

fn civil_to_rfc3339(secs: i64) -> String {
    let (y, mo, day, h, mi, s) = civil_from_unix(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, mo, day, h, mi, s
    )
}

/// civil_from_unix — дни → календарь (алгоритм Howard Hinnant).
pub(crate) fn civil_from_unix(z: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = z.div_euclid(86400);
    let secs = z.rem_euclid(86400);
    let h = (secs / 3600) as u32;
    let mi = ((secs % 3600) / 60) as u32;
    let s = (secs % 60) as u32;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as i64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d, h, mi, s)
}

// ═══════════════════════════════════════════
//  HTTP-клиент (только http://, без TLS — как в Go net/http)
// ═══════════════════════════════════════════

/// StreamWriter — HTTP-писатель со стримингом: позволяет выставить
/// заголовки ДО первого байта тела (как http.ResponseWriter.Header()).
pub trait StreamWriter: Write {
    fn set_header(&mut self, _key: &str, _value: &str) {}
}

fn build_request(
    addr: &str,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
) -> Vec<u8> {
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n",
        method, path, addr
    );
    if let Some(t) = token {
        req.push_str(&format!("Authorization: Bearer {}\r\n", t));
    }
    if body.is_some() {
        req.push_str("Content-Type: application/json\r\n");
    }
    if let Some(b) = body {
        req.push_str(&format!("Content-Length: {}\r\n", b.len()));
    }
    req.push_str("\r\n");
    if let Some(b) = body {
        req.push_str(b);
    }
    req.into_bytes()
}

/// read_head — читает из потока до "\r\n\r\n", возвращает (head, остаток-буфер).
fn read_head(stream: &mut std::net::TcpStream) -> Result<(String, Vec<u8>), String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Err("empty response".into()),
            Ok(_) => {
                head.push(byte[0]);
                if head.len() >= 4 && &head[head.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
                if head.len() > 65536 {
                    return Err("headers too large".into());
                }
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    let sep = head.len();
    Ok((String::from_utf8_lossy(&head).to_string(), head[sep..].to_vec()))
}

/// parse_status — код из статус-строки.
fn parse_status(head: &str) -> Result<u16, String> {
    head.lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|x| x.parse().ok())
        .ok_or_else(|| "bad status line".to_string())
}

/// header_value — значение заголовка (case-insensitive).
fn header_value(head: &str, name: &str) -> Option<String> {
    for line in head.lines().skip(1) {
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case(name) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

fn is_chunked(head: &str) -> bool {
    header_value(head, "transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false)
}

/// read_body_framed — дочитывает тело ответа по фреймингу:
/// Content-Length → ровно N; иначе chunked; иначе до EOF.
fn read_body_framed(
    stream: &mut std::net::TcpStream,
    head: &str,
    mut pre: Vec<u8>,
) -> Result<Vec<u8>, String> {
    if let Some(cl) = header_value(head, "content-length") {
        if let Ok(n) = cl.parse::<usize>() {
            while pre.len() < n {
                let mut buf = [0u8; 8192];
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(k) => pre.extend_from_slice(&buf[..k]),
                    Err(e) => return Err(e.to_string()),
                }
            }
            pre.truncate(n);
            return Ok(pre);
        }
    }
    if is_chunked(head) {
        // добираем до EOF (Connection: close гарантирует завершение)
        let mut data = pre;
        let mut buf = [0u8; 8192];
        loop {
            match stream.read(&mut buf) {
                Ok(0) => break,
                Ok(k) => data.extend_from_slice(&buf[..k]),
                Err(e) => return Err(e.to_string()),
            }
        }
        return Ok(decode_chunked(&data));
    }
    // close-delimited
    let mut data = pre;
    let mut buf = [0u8; 8192];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(k) => data.extend_from_slice(&buf[..k]),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(data)
}

/// http_request — обычный вызов: (status, body).
fn http_request(
    host: &str,
    port: u16,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&str>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    let addr = format!("{}:{}", host, port);
    let sa: std::net::SocketAddr = addr.parse().map_err(|e| format!("addr: {}", e))?;
    let mut stream =
        std::net::TcpStream::connect_timeout(&sa, timeout).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;

    let req = build_request(&addr, method, path, token, body);
    stream.write_all(&req).map_err(|e| e.to_string())?;

    let (head, pre) = read_head(&mut stream)?;
    let status = parse_status(&head)?;
    let body = read_body_framed(&mut stream, &head, pre)?;
    Ok((status, body))
}

/// decode_chunked — декодирование chunked-тела.
fn decode_chunked(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let Some(nl) = data[i..].windows(2).position(|w| w == b"\r\n") else {
            break;
        };
        let size_str = String::from_utf8_lossy(&data[i..i + nl]);
        let size_str = size_str.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_str, 16) else {
            break;
        };
        i += nl + 2;
        if size == 0 {
            break;
        }
        if i + size > data.len() {
            out.extend_from_slice(&data[i..]);
            break;
        }
        out.extend_from_slice(&data[i..i + size]);
        i += size + 2; // CRLF
    }
    out
}

/// remote_error — ошибка из тела ответа панели: {"error": ...} либо "remote HTTP %d".
fn remote_error(status: u16, data: &[u8]) -> String {
    let v: Value = serde_json::from_slice(data).unwrap_or(Value::Null);
    if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
        if !e.is_empty() {
            return e.to_string();
        }
    }
    format!("remote HTTP {}", status)
}

// ═══════════════════════════════════════════
//  ClusterStore
// ═══════════════════════════════════════════

pub struct ClusterStore {
    inner: Mutex<Vec<Server>>,
    data_dir: String,
}

impl ClusterStore {
    pub fn new(data_dir: &str) -> Self {
        let _ = std::fs::create_dir_all(data_dir);
        let cs = ClusterStore {
            inner: Mutex::new(Vec::new()),
            data_dir: data_dir.to_string(),
        };
        cs.load();
        cs
    }

    fn path(&self) -> String {
        Path::new(&self.data_dir)
            .join("cluster.json")
            .to_string_lossy()
            .to_string()
    }

    fn load(&self) {
        if let Ok(data) = std::fs::read_to_string(self.path()) {
            if let Ok(servers) = serde_json::from_str::<Vec<Server>>(&data) {
                if let Ok(mut m) = self.inner.lock() {
                    *m = servers;
                }
            }
        }
    }

    /// save_locked — атомарная запись: tmp + rename (0600 как в Go).
    fn save_locked(&self, servers: &[Server]) {
        let Ok(data) = serde_json::to_string_pretty(servers) else {
            return;
        };
        let p = self.path();
        let tmp = format!("{}.tmp", p);
        if std::fs::write(&tmp, data).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }

    fn snapshot(&self) -> Vec<Server> {
        match self.inner.lock() {
            Ok(m) => m.clone(),
            Err(_) => Vec::new(),
        }
    }

    pub fn list_servers(&self) -> Vec<Server> {
        self.snapshot()
    }

    pub fn add_server(&self, name: &str, host: &str, port: u16, user: &str, pass: &str) -> Server {
        let s = Server {
            id: gen_short_id(),
            name: name.to_string(),
            host: host.to_string(),
            port,
            user: user.to_string(),
            pass: pass.to_string(),
            token: String::new(),
            online: false,
            last_ping: default_ping(),
        };
        if let Ok(mut m) = self.inner.lock() {
            m.push(s.clone());
            let snap = m.clone();
            drop(m);
            self.save_locked(&snap);
        }
        s
    }

    /// update_server — пустое поле = «не менять»; port только >0;
    /// после смены учётки токен сбрасывается.
    pub fn update_server(
        &self,
        id: &str,
        name: &str,
        host: &str,
        port: u16,
        user: &str,
        pass: &str,
    ) -> bool {
        if let Ok(mut m) = self.inner.lock() {
            for s in m.iter_mut() {
                if s.id == id {
                    if !name.is_empty() {
                        s.name = name.to_string();
                    }
                    if !host.is_empty() {
                        s.host = host.to_string();
                    }
                    if port > 0 {
                        s.port = port;
                    }
                    if !user.is_empty() {
                        s.user = user.to_string();
                    }
                    if !pass.is_empty() {
                        s.pass = pass.to_string();
                    }
                    s.token = String::new();
                    let snap = m.clone();
                    drop(m);
                    self.save_locked(&snap);
                    return true;
                }
            }
        }
        false
    }

    pub fn remove_server(&self, id: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.retain(|s| s.id != id);
            let snap = m.clone();
            drop(m);
            self.save_locked(&snap);
        }
    }

    pub fn get_server(&self, id: &str) -> Option<Server> {
        self.snapshot().into_iter().find(|s| s.id == id)
    }

    fn update_token(&self, id: &str, token: &str) {
        if let Ok(mut m) = self.inner.lock() {
            for s in m.iter_mut() {
                if s.id == id {
                    s.token = token.to_string();
                    s.online = true;
                    s.last_ping = now_rfc3339();
                    break;
                }
            }
            let snap = m.clone();
            drop(m);
            self.save_locked(&snap);
        }
    }

    fn set_online(&self, id: &str, online: bool) {
        if let Ok(mut m) = self.inner.lock() {
            for s in m.iter_mut() {
                if s.id == id {
                    s.online = online;
                    s.last_ping = now_rfc3339();
                    break;
                }
            }
            let snap = m.clone();
            drop(m);
            self.save_locked(&snap);
        }
    }

    /// LoginServer — POST /api/v1/login, сохранение токена (5с таймаут).
    pub fn login_server(&self, id: &str) -> Result<(), String> {
        let Some(s) = self.get_server(id) else {
            return Err("server not found".into());
        };
        let body = format!(
            "{{\"username\":{},\"password\":{}}}",
            json_str(&s.user),
            json_str(&s.pass)
        );
        let (status, data) = http_request(
            &s.host,
            s.port,
            "POST",
            "/api/v1/login",
            None,
            Some(&body),
            Duration::from_secs(5),
        )?;
        let _ = status;
        let v: Value = serde_json::from_slice(&data).unwrap_or(Value::Null);
        if let Some(e) = v.get("error").and_then(|e| e.as_str()) {
            if !e.is_empty() {
                return Err(e.to_string());
            }
        }
        let token = v
            .get("token")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();
        if token.is_empty() {
            return Err("login failed".into());
        }
        self.update_token(id, &token);
        Ok(())
    }

    fn ensure_token(&self, id: &str) -> Result<Server, String> {
        let Some(mut s) = self.get_server(id) else {
            return Err("server not found".into());
        };
        if s.token.is_empty() {
            self.login_server(id)?;
            s = self.get_server(id).ok_or_else(|| "server not found".to_string())?;
        }
        Ok(s)
    }

    /// call_server — обычный вызов (30с), один relogin-ретрай при 401,
    /// на >=400 возвращает ошибку из тела ответа (данные хендлерам не нужны).
    pub fn call_server(
        &self,
        id: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Vec<u8>, String> {
        let mut s = self.ensure_token(id)?;
        let timeout = Duration::from_secs(30);
        let body_str = body.map(|v| v.to_string());
        let (mut status, mut data) = http_request(
            &s.host,
            s.port,
            method,
            path,
            Some(&s.token),
            body_str.as_deref(),
            timeout,
        )?;
        if status == 401 {
            self.login_server(id)?;
            s = self.get_server(id).ok_or_else(|| "server not found".to_string())?;
            let r = http_request(
                &s.host,
                s.port,
                method,
                path,
                Some(&s.token),
                body_str.as_deref(),
                timeout,
            )?;
            status = r.0;
            data = r.1;
        }
        if status >= 400 {
            return Err(remote_error(status, &data));
        }
        Ok(data)
    }

    /// call_server_stream — вызов с потоковой отдачей тела в writer (exec/logs).
    /// Один relogin-ретрай при 401; на >=400 возвращает Err (тело не пишем).
    /// Успешный статус → выставляем remote Content-Type (+X-Accel) ДО первого байта.
    /// Ошибки чтения тела после старта отдачи — глотаем (как Go `break`).
    pub fn call_server_stream(
        &self,
        id: &str,
        method: &str,
        path: &str,
        body: Option<&Value>,
        writer: &mut dyn StreamWriter,
        retried: bool,
    ) -> Result<(), String> {
        let s = self.ensure_token(id)?;
        let addr = format!("{}:{}", s.host, s.port);
        let sa: std::net::SocketAddr = addr.parse().map_err(|e| format!("addr: {}", e))?;
        let mut stream =
            std::net::TcpStream::connect_timeout(&sa, Duration::from_secs(10))
                .map_err(|e| e.to_string())?;
        // без общего таймаута — exec может работать долго (как Go http.Client{}),
        // но застрявший peer не должен висеть вечно
        stream
            .set_read_timeout(Some(Duration::from_secs(3600)))
            .map_err(|e| e.to_string())?;

        let body_str = body.map(|v| v.to_string());
        let req = build_request(&addr, method, path, Some(&s.token), body_str.as_deref());
        stream.write_all(&req).map_err(|e| e.to_string())?;

        let (head, pre) = read_head(&mut stream)?;
        let mut status = parse_status(&head)?;
        if status == 401 && !retried {
            drop(stream);
            self.login_server(id)?;
            return self.call_server_stream(id, method, path, body, writer, true);
        }
        if status >= 400 {
            let data = read_body_framed(&mut stream, &head, pre)?;
            return Err(remote_error(status, &data));
        }

        // remote CT (или text/plain) + буферизация откл. у прокси — ДО первого байта
        let ct = header_value(&head, "content-type")
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| "text/plain".to_string());
        writer.set_header("Content-Type", &ct);
        writer.set_header("X-Accel-Buffering", "no");

        // потоковая отдача
        if is_chunked(&head) {
            stream_chunks(&mut stream, pre, writer)?;
        } else if let Some(cl) = header_value(&head, "content-length") {
            if let Ok(n) = cl.parse::<usize>() {
                let mut got = pre;
                if got.len() > n {
                    got.truncate(n);
                }
                if writer.write_all(&got).is_err() {
                    return Ok(());
                }
                let _ = writer.flush();
                while got.len() < n {
                    let mut buf = [0u8; 4096];
                    match stream.read(&mut buf) {
                        Ok(0) => break,
                        Ok(k) => {
                            let take = std::cmp::min(k, n - got.len());
                            if writer.write_all(&buf[..take]).is_err() {
                                return Ok(());
                            }
                            let _ = writer.flush();
                            got.extend_from_slice(&buf[..take]);
                        }
                        // ошибка чтения середины тела → break (как в Go)
                        Err(_) => break,
                    }
                }
                return Ok(());
            }
            // битый Content-Length → фоллбек до EOF
            stream_to_end(&mut stream, pre, writer)?;
        } else {
            stream_to_end(&mut stream, pre, writer)?;
        }
        Ok(())
    }

    /// PingServer — GET /api/v1/status (3с): online при 200/401.
    pub fn ping_server(&self, id: &str) -> bool {
        let Some(s) = self.get_server(id) else {
            return false;
        };
        let online = match http_request(
            &s.host,
            s.port,
            "GET",
            "/api/v1/status",
            None,
            None,
            Duration::from_secs(3),
        ) {
            Ok((status, _)) => status == 200 || status == 401,
            Err(_) => false,
        };
        self.set_online(id, online);
        online
    }

    /// PingAll — пинг всех нод параллельно (иначе Refresh ждёт sum(timeout)).
    pub fn ping_all(&self) {
        let servers = self.snapshot();
        std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for s in &servers {
                let store = self;
                let id = s.id.clone();
                handles.push(scope.spawn(move || {
                    store.ping_server(&id);
                }));
            }
            for h in handles {
                let _ = h.join();
            }
        });
    }
}

/// stream_chunks — декодирует chunked-поток и пишет чанки в writer.
/// Ошибки чтения середины потока глотаем (как Go: `if err != nil { break }`).
fn stream_chunks(
    stream: &mut std::net::TcpStream,
    mut pending: Vec<u8>,
    writer: &mut dyn StreamWriter,
) -> Result<(), String> {
    let mut buf = [0u8; 4096];
    'outer: loop {
        // строка размера чанка
        let pos = loop {
            if let Some(p) = pending.windows(2).position(|w| w == b"\r\n") {
                break p;
            }
            match stream.read(&mut buf) {
                Ok(0) => return Ok(()),
                Ok(n) => pending.extend_from_slice(&buf[..n]),
                Err(_) => return Ok(()),
            }
        };
        let size_str = String::from_utf8_lossy(&pending[..pos]).to_string();
        let size_str = size_str.split(';').next().unwrap_or("").trim().to_string();
        let Ok(size) = usize::from_str_radix(&size_str, 16) else {
            return Ok(());
        };
        pending.drain(..pos + 2);
        if size == 0 {
            return Ok(());
        }
        while pending.len() < size + 2 {
            match stream.read(&mut buf) {
                Ok(0) => break 'outer,
                Ok(n) => pending.extend_from_slice(&buf[..n]),
                Err(_) => break 'outer,
            }
        }
        if writer.write_all(&pending[..size]).is_err() {
            return Ok(());
        }
        let _ = writer.flush();
        pending.drain(..size + 2);
    }
    Ok(())
}

fn stream_to_end(
    stream: &mut std::net::TcpStream,
    pre: Vec<u8>,
    writer: &mut dyn StreamWriter,
) -> Result<(), String> {
    if !pre.is_empty() && writer.write_all(&pre).is_err() {
        return Ok(());
    }
    let _ = writer.flush();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => {
                if writer.write_all(&buf[..n]).is_err() {
                    return Ok(());
                }
                let _ = writer.flush();
            }
            Err(_) => return Ok(()),
        }
    }
}

/// json_str — экранирование строки в JSON (для логина, как fmt %q).
fn json_str(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

// ═══════════════════════════════════════════
//  Cluster (хендлеры)
// ═══════════════════════════════════════════

/// NodeStatus — узел в /api/v1/cluster/status (порядок полей как в Go).
#[derive(Serialize)]
struct NodeStatus {
    id: String,
    name: String,
    host: String,
    port: u16,
    online: bool,
    last_ping: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    error: String,
    containers: i64,
    sites: Vec<String>,
    runners: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    version: String,
}

/// RemoteStatus — /api/v1/status удалённой ноды (как анон. struct в Go).
#[derive(Deserialize)]
struct RemoteStatus {
    #[serde(default)]
    containers: i64,
    sites: Option<Vec<String>>,
    runners: Option<Vec<String>>,
    #[serde(default)]
    version: String,
}

pub struct Cluster {
    pub store: ClusterStore,
}

impl Cluster {
    pub fn new(data_dir: &str) -> Self {
        Cluster {
            store: ClusterStore::new(data_dir),
        }
    }

    /// handle_status — агрегатная сводка кластера (ноды параллельно).
    /// Метод (GET) проверяет serve.rs → 405. Порядок полей — как в Go
    /// (nodeStatus struct), top-level map сортируется (nodes/online/total/version).
    pub fn handle_status(&self) -> Value {
        let servers = self.store.list_servers();
        let out: Vec<NodeStatus> = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for s in servers {
                handles.push(scope.spawn(move || {
                    let mut ns = NodeStatus {
                        id: s.id.clone(),
                        name: s.name.clone(),
                        host: s.host.clone(),
                        port: s.port,
                        online: s.online,
                        last_ping: s.last_ping.clone(),
                        error: String::new(),
                        containers: 0,
                        sites: Vec::new(),
                        runners: Vec::new(),
                        version: String::new(),
                    };
                    match self.store.call_server(&s.id, "GET", "/api/v1/status", None) {
                        Ok(data) => {
                            if let Ok(st) = serde_json::from_slice::<RemoteStatus>(&data) {
                                ns.online = true;
                                ns.containers = st.containers;
                                if let Some(v) = st.sites {
                                    ns.sites = v;
                                }
                                if let Some(v) = st.runners {
                                    ns.runners = v;
                                }
                                ns.version = st.version;
                            }
                        }
                        Err(e) => {
                            ns.online = false;
                            ns.error = e;
                        }
                    }
                    ns
                }));
            }
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .collect::<Vec<_>>()
        });
        let online = out.iter().filter(|n| n.online).count();
        json!({
            "nodes": out,
            "total": out.len(),
            "online": online,
            "version": crate::VERSION,
        })
    }

    /// handle_servers — GET список / POST добавить (как в Go: иные методы
    /// не обрабатываются — serve.rs отдаст пустой 200).
    /// body_raw: сырое JSON-тело; для POST ошибка разбора → 400 "invalid json".
    pub fn handle_servers(&self, method: &str, body_raw: Option<&str>) -> (u16, Value) {
        match method {
            "GET" => (200, json!(self.store.list_servers())),
            "POST" => {
                let parsed: Option<Value> = body_raw.and_then(|b| serde_json::from_str(b).ok());
                let Some(b) = parsed else {
                    return (400, json!({"error": "invalid json"}));
                };
                let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let host = b.get("host").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() || host.is_empty() {
                    return (400, json!({"error": "name and host required"}));
                }
                // Go: if req.Port == 0 { req.Port = 3000 } — Some(0) тоже 3000
                let port = match b.get("port").and_then(|v| v.as_u64()) {
                    Some(0) | None => 3000,
                    Some(p) => p as u16,
                };
                let user = b.get("user").and_then(|v| v.as_str()).unwrap_or("");
                let pass = b.get("pass").and_then(|v| v.as_str()).unwrap_or("");
                let s = self.store.add_server(name, host, port, user, pass);
                let login_err = self.store.login_server(&s.id).err().unwrap_or_default();
                let mut resp = json!({"status": "added", "id": s.id});
                if !login_err.is_empty() {
                    resp["warning"] = json!(login_err);
                }
                (200, resp)
            }
            _ => (200, Value::Null), // Go: пустой 200 — serve.rs не вызывает
        }
    }

    /// handle_server — операции над одной нодой (включая прокси).
    /// body_raw — сырое тело; None = нет тела. Ошибка разбора для
    /// PUT/deploy → 400 "invalid json" (как jsonDec в Go); для прокси
    /// игнорируется. writer — для стриминга (logs/exec).
    /// Порядок веток отличается от Go: стрим-прокси идут раньше списочных
    /// веток, а списки ограничены len(parts)==2 — иначе Go-порядок затеняет
    /// /:id/containers/:cname/logs и /:id/sites/:sname/logs (списком).
    /// Задокументированное отклонение от паритета.
    pub fn handle_server(
        &self,
        method: &str,
        rest_path: &str,
        body_raw: Option<&str>,
        query: &str,
        writer: Option<&mut dyn StreamWriter>,
    ) -> ServerReply {
        let parts: Vec<&str> = rest_path.split('/').collect();
        if parts.is_empty() || parts[0].is_empty() {
            return ServerReply::json(400, json!({"error": "invalid"}));
        }
        let id = parts[0].to_string();
        let parsed: Option<Value> = body_raw.and_then(|b| serde_json::from_str(b).ok());

        if parts.len() >= 2 {
            let action = parts[1].to_string();

            if action == "ping" && method == "GET" {
                let ok = self.store.ping_server(&id);
                let s = self.store.get_server(&id);
                return ServerReply::json(200, json!({"online": ok, "server": s}));
            }

            // GET /servers/:id/sites/:sname/logs — логи сайта (стрим).
            if action == "sites" && parts.len() >= 4 && parts[3] == "logs" && method == "GET" {
                let sname = parts[2].to_string();
                let tail = extract_query(query, "tail").unwrap_or_else(|| "300".into());
                let path = format!("/api/v1/containers/{}/logs?tail={}", sname, tail);
                let Some(w) = writer else {
                    return ServerReply::json(500, json!({"error": "no writer"}));
                };
                if let Err(e) = self
                    .store
                    .call_server_stream(&id, "GET", &path, None, w, false)
                {
                    let _ = w.write_all(format!("Error: {}", e).as_bytes());
                }
                return ServerReply::Streamed;
            }

            // Proxy /servers/:id/containers/:cname/* (logs/exec — стрим)
            if action == "containers" && parts.len() >= 3 {
                let cname = parts[2].to_string();
                let rest = parts[3..].join("/");
                let rest_str = if rest.is_empty() {
                    String::new()
                } else {
                    format!("/{}", rest)
                };
                let path = format!("/api/v1/containers/{}{}", cname, rest_str);
                let b: Option<Value> = if matches!(method, "POST" | "PUT" | "PATCH") {
                    parsed.clone()
                } else {
                    None
                };
                if rest_str.ends_with("/logs") || rest_str.ends_with("/exec") {
                    let Some(w) = writer else {
                        return ServerReply::json(500, json!({"error": "no writer"}));
                    };
                    if let Err(e) =
                        self.store
                            .call_server_stream(&id, method, &path, b.as_ref(), w, false)
                    {
                        let _ = w.write_all(format!("Error: {}", e).as_bytes());
                    }
                    return ServerReply::Streamed;
                }
                return match self.store.call_server(&id, method, &path, b.as_ref()) {
                    Ok(data) => ServerReply::RawJson(data),
                    Err(e) => ServerReply::json(502, json!({"error": e})),
                };
            }

            // Proxy /servers/:id/runners/:rname/* (logs — стрим)
            if action == "runners" && parts.len() >= 3 {
                let rname = parts[2].to_string();
                let rest = parts[3..].join("/");
                let rest_str = if rest.is_empty() {
                    String::new()
                } else {
                    format!("/{}", rest)
                };
                let path = format!("/api/v1/runners/{}{}", rname, rest_str);
                let b: Option<Value> = if matches!(method, "POST" | "PUT" | "PATCH") {
                    parsed.clone()
                } else {
                    None
                };
                if rest_str.ends_with("/logs") {
                    let Some(w) = writer else {
                        return ServerReply::json(500, json!({"error": "no writer"}));
                    };
                    if let Err(e) =
                        self.store
                            .call_server_stream(&id, method, &path, b.as_ref(), w, false)
                    {
                        let _ = w.write_all(format!("Error: {}", e).as_bytes());
                    }
                    return ServerReply::Streamed;
                }
                return match self.store.call_server(&id, method, &path, b.as_ref()) {
                    Ok(data) => ServerReply::RawJson(data),
                    Err(e) => ServerReply::json(502, json!({"error": e})),
                };
            }

            // Списочные ветки — только ровно /:id/:action (иначе затеняют
            // глубже пути, как в Go).
            if parts.len() == 2 {
                let simple: [(&str, &str); 4] = [
                    ("status", "/api/v1/status"),
                    ("containers", "/api/v1/containers?show_all=true"),
                    ("runners", "/api/v1/runners"),
                    ("sites", "/api/v1/sites"),
                ];
                for (act, remote) in simple {
                    if action == act && method == "GET" {
                        return match self.store.call_server(&id, "GET", remote, None) {
                            Ok(data) => ServerReply::RawJson(data),
                            Err(e) => ServerReply::json(502, json!({"error": e})),
                        };
                    }
                }
            }

            // POST /servers/:id/deploy — деплой на удалённую ноду
            if action == "deploy" && method == "POST" {
                if body_raw.is_some() && parsed.is_none() {
                    return ServerReply::json(400, json!({"error": "invalid json"}));
                }
                return match self
                    .store
                    .call_server(&id, "POST", "/api/v1/sites", parsed.as_ref())
                {
                    Ok(data) => ServerReply::RawJson(data),
                    Err(e) => ServerReply::json(502, json!({"error": e})),
                };
            }
        }

        if method == "DELETE" {
            self.store.remove_server(&id);
            return ServerReply::json(200, json!({"status": "removed"}));
        }

        if method == "PUT" {
            if body_raw.is_some() && parsed.is_none() {
                return ServerReply::json(400, json!({"error": "invalid json"}));
            }
            let b = parsed.unwrap_or(Value::Null);
            let name = b.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let host = b.get("host").and_then(|v| v.as_str()).unwrap_or("");
            let port = b.get("port").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
            let user = b.get("user").and_then(|v| v.as_str()).unwrap_or("");
            let pass = b.get("pass").and_then(|v| v.as_str()).unwrap_or("");
            if !self.store.update_server(&id, name, host, port, user, pass) {
                return ServerReply::json(404, json!({"error": "not found"}));
            }
            let warning = self.store.login_server(&id).err().unwrap_or_default();
            let mut resp = json!({"status": "updated"});
            if !warning.is_empty() {
                resp["warning"] = json!(warning);
            }
            return ServerReply::json(200, resp);
        }

        match self.store.get_server(&id) {
            Some(s) => ServerReply::json(200, json!(s)),
            None => ServerReply::json(404, json!({"error": "not found"})),
        }
    }
}

/// ServerReply — результат обработки запроса к ноде.
pub enum ServerReply {
    Json(u16, Value),
    RawJson(Vec<u8>),
    Streamed,
}

impl ServerReply {
    fn json(status: u16, v: Value) -> ServerReply {
        ServerReply::Json(status, v)
    }
}

fn extract_query(query: &str, key: &str) -> Option<String> {
    for pair in query.split('&') {
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("");
        let v = it.next().unwrap_or("");
        if k == key {
            return Some(v.to_string());
        }
    }
    None
}

pub fn gen_auth_token() -> String {
    gen_token()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_epoch() {
        assert_eq!(civil_to_rfc3339(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn rfc3339_known_date() {
        // 2026-10-10 00:00:00 UTC = 1791571200? — проверяем структуру и монотонность
        let s = civil_to_rfc3339(1_760_000_000);
        assert!(s.starts_with("2025-10-"), "unexpected {}", s);
        assert!(s.ends_with('Z'));
        // 1 день ровно
        assert_eq!(civil_to_rfc3339(86400), "1970-01-02T00:00:00Z");
        // високосный год
        assert_eq!(civil_to_rfc3339(1_709_164_800), "2024-02-29T00:00:00Z");
    }

    #[test]
    fn chunked_decode() {
        let raw = b"5\r\nhello\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw), b"hello".to_vec());
        let raw2 = b"3\r\nabc\r\n4\r\ndefg\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw2), b"abcdefg".to_vec());
    }

    #[test]
    fn server_serde_roundtrip() {
        let s = Server {
            id: "abc".into(),
            name: "n".into(),
            host: "1.2.3.4".into(),
            port: 3000,
            user: "u".into(),
            pass: "p".into(),
            token: String::new(),
            online: false,
            last_ping: default_ping(),
        };
        let j = serde_json::to_string(&s).unwrap();
        assert!(j.contains("\"last_ping\":\"0001-01-01T00:00:00Z\""));
        assert!(!j.contains("\"token\""));
        let back: Server = serde_json::from_str(&j).unwrap();
        assert_eq!(back.id, "abc");
        // старый cluster.json от Go (с token) читается
        let go_json = r#"{"id":"x","name":"n","host":"h","port":3000,"user":"u","pass":"p","token":"t","online":true,"last_ping":"2026-10-08T18:20:00.123456789+03:00"}"#;
        let parsed: Server = serde_json::from_str(go_json).unwrap();
        assert_eq!(parsed.token, "t");
        assert!(parsed.online);
        assert_eq!(parsed.last_ping, "2026-10-08T18:20:00.123456789+03:00");
    }

    #[test]
    fn header_parsing() {
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 5\r\n\r\n";
        assert_eq!(parse_status(head).unwrap(), 200);
        assert_eq!(header_value(head, "content-length").unwrap(), "5");
        assert_eq!(
            header_value(head, "CONTENT-TYPE").unwrap(),
            "text/plain"
        );
        assert!(!is_chunked(head));
        let h2 = "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(is_chunked(h2));
    }
}
