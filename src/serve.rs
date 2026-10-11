// serve — HTTP web-server: точный порт Go serve.go + new port syntax.
use crate::auth::Auth;
use crate::cluster::{civil_from_unix, Cluster, ServerReply, StreamWriter};
use crate::compose::{
    container_host_port, free_port, list_services, runner_names, service_name, PortSpec,
};
use crate::config::{deploy_home, get_site_custom, runner_config_file, save_site_custom, SiteCustom};
use crate::deploy::{deploy_site, get_container_status, remove_site};
use crate::lang::{detect_lang, LangInfo};
use crate::util::{cmd_combined, cmd_output, cmd_output_timeout, run_capture, run_cmd, stream_exec};
use crate::VERSION;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tiny_http::{Header, HTTPVersion, Method, Request, Server};

// ─────────────────────────── helpers ───────────────────────────

fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        _ => "",
    }
}

/// escape_html — глобальная замена символов как в Go encoding/json (SetEscapeHTML(true)).
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            _ => out.push(c),
        }
    }
    out
}

/// go_json — сериализация Value + глобальное HTML-экранирование + завершающий \n (Encoder).
fn go_json(v: &Value) -> Vec<u8> {
    let mut s = serde_json::to_string(v).unwrap_or_else(|_| "null".to_string());
    s = escape_html(&s);
    s.push('\n');
    s.into_bytes()
}

/// go_pretty — MarshalIndent (2 пробела) + HTML-экранирование, БЕЗ завершающего \n.
fn go_pretty(v: &Value) -> Vec<u8> {
    let s = serde_json::to_string_pretty(v).unwrap_or_else(|_| "null".to_string());
    escape_html(&s).into_bytes()
}

/// sniff — определение Content-Type по первым байтам (Go net/http sniffing).
fn sniff(body: &[u8]) -> &'static str {
    if body.is_empty() {
        return "text/plain; charset=utf-8";
    }
    if body[0] == 0 {
        return "application/octet-stream";
    }
    // Пропускаем ведущие пробельные символы
    let mut i = 0;
    while i < body.len() && matches!(body[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    let rest = &body[i..];
    const HTML_SIGS: &[&[u8]] = &[
        b"<!DOCTYPE HTML",
        b"<HTML",
        b"<HEAD",
        b"<SCRIPT",
        b"<IFRAME",
        b"<H1",
        b"<DIV",
        b"<FONT",
        b"<TABLE",
        b"<A",
        b"<STYLE",
        b"<TITLE",
        b"<B",
        b"<BODY",
        b"<BR",
        b"<P",
        b"<!--",
    ];
    for sig in HTML_SIGS {
        if rest.len() >= sig.len() && rest[..sig.len()].eq_ignore_ascii_case(sig) {
            let after = &rest[sig.len()..];
            if after.is_empty() || matches!(after[0], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/') {
                return "text/html; charset=utf-8";
            }
        }
    }
    "text/plain; charset=utf-8"
}

/// go_http_date — формат Go time.RFC1123 GMT: "Mon, 02 Jan 2006 15:04:05 GMT".
fn go_http_date(unix: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_unix(unix);
    const WDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = unix.div_euclid(86400);
    // 1970-01-01 = Thursday (index 4)
    let wday = (days + 4).rem_euclid(7) as usize;
    format!(
        "{}, {:02} {} {:04} {:02}:{:02}:{:02} GMT",
        WDAYS[wday], d, MONTHS[(mo - 1) as usize], y, h, mi, s
    )
}

/// query_get — сырой разбор query-строки (без percent-decode).
fn query_get(query: &str, key: &str) -> Option<String> {
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

/// api_default_port — порт по типу (как defaultPorts в Go).
fn api_default_port(typ: &str) -> u16 {
    match typ {
        "static" | "php" | "go" => 8080,
        "python" => 8000,
        "node" => 3000,
        _ => 0,
    }
}

fn argv0_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// get_container_image — образ контейнера (локальный, stdout-only).
fn get_container_image(name: &str) -> String {
    cmd_output_timeout("docker", &["inspect", "-f", "{{.Config.Image}}", name], 3)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// parse_port_field — поле port: число → (n, None); строка → PortSpec::parse.
fn parse_port_field(v: Option<&Value>) -> (u16, Option<u16>) {
    match v {
        Some(Value::Number(n)) => (n.as_u64().unwrap_or(0) as u16, None),
        Some(Value::String(s)) => {
            let ps = PortSpec::parse(s);
            (ps.external, ps.internal)
        }
        _ => (0, None),
    }
}

/// runner_labels — labels из runner-config.yaml (все строки → join ", ").
fn runner_labels(name: &str) -> String {
    let path = runner_config_file(name);
    let Ok(data) = std::fs::read_to_string(&path) else {
        return String::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&data) else {
        return String::new();
    };
    if let Some(arr) = v.get("labels").and_then(|x| x.as_array()) {
        let strs: Vec<&str> = arr.iter().filter_map(|x| x.as_str()).collect();
        if strs.len() == arr.len() && !strs.is_empty() {
            return strs.join(", ");
        }
    }
    String::new()
}

// ─────────────────────────── RespWriter ───────────────────────────

/// Frame — способ фреймирования тела ответа (как в Go net/http).
enum Frame {
    Identity(u64), // Content-Length: N + тело
    Chunked,       // Transfer-Encoding: chunked
    NoCL,          // без Content-Length и TE (close-delimited / HEAD)
}

pub struct RespWriter {
    out: Box<dyn Write + Send>,
    status: u16,
    headers: Vec<(String, String)>,
    buf: Vec<u8>,
    head_written: bool,
    streaming: bool,  // flush() уже вызывался
    chunked: bool,    // потоковая отдача через chunked (http11)
    frozen: bool,     // set_status/set_header больше не применяются (WriteHeader зафиксирован)
    is_head: bool,
    wants_close: bool,     // http11 + Connection: close
    wants_keepalive: bool, // http10 + Connection: keep-alive
    http11: bool,
    date: String,
}

impl RespWriter {
    fn new(
        out: Box<dyn Write + Send>,
        is_head: bool,
        http11: bool,
        wants_close: bool,
        wants_keepalive: bool,
    ) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        RespWriter {
            out,
            status: 200,
            headers: Vec::new(),
            buf: Vec::new(),
            head_written: false,
            streaming: false,
            chunked: false,
            frozen: false,
            is_head,
            wants_close,
            wants_keepalive,
            http11,
            date: go_http_date(now),
        }
    }

    fn set_status(&mut self, code: u16) {
        if !self.head_written && !self.frozen {
            self.status = code;
            self.frozen = true; // WriteHeader зафиксирован
        }
    }

    fn put_header(&mut self, k: &str, v: &str) {
        if self.head_written || self.frozen {
            return;
        }
        // case-insensitive replace (как Header().Set в Go)
        for h in self.headers.iter_mut() {
            if h.0.eq_ignore_ascii_case(k) {
                h.1 = v.to_string();
                return;
            }
        }
        self.headers.push((k.to_string(), v.to_string()));
    }

    /// has_explicit_ct — есть ли явный Content-Type среди заголовков.
    fn has_explicit_ct(&self) -> bool {
        self.headers.iter().any(|h| h.0.eq_ignore_ascii_case("content-type"))
    }

    /// write_head — пишет статус-строку + заголовки по заданному фрейму.
    fn write_head(&mut self, frame: Frame) {
        if self.head_written {
            return;
        }
        let mut head = Vec::with_capacity(256);
        // статус-строка
        head.extend_from_slice(
            format!(
                "HTTP/1.{} {} {}\r\n",
                if self.http11 { 1 } else { 0 },
                self.status,
                reason_phrase(self.status)
            )
            .as_bytes(),
        );
        // явные заголовки, отсортированные по алфавиту (Go WriteSubset)
        let mut sorted = self.headers.clone();
        sorted.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        for (k, v) in &sorted {
            head.extend_from_slice(format!("{}: {}\r\n", k, v).as_bytes());
        }
        // Date
        head.extend_from_slice(format!("Date: {}\r\n", self.date).as_bytes());
        // Content-Length (только для Identity)
        if let Frame::Identity(cl) = frame {
            head.extend_from_slice(format!("Content-Length: {}\r\n", cl).as_bytes());
        }
        // sniffed Content-Type (только если нет явного и тело не пустое)
        if !self.has_explicit_ct() && !self.buf.is_empty() {
            let ct = sniff(&self.buf);
            head.extend_from_slice(format!("Content-Type: {}\r\n", ct).as_bytes());
        }
        // Connection
        if self.http11 && self.wants_close {
            head.extend_from_slice(b"Connection: close\r\n");
        } else if !self.http11
            && self.wants_keepalive
            && matches!(frame, Frame::Identity(_))
        {
            head.extend_from_slice(b"Connection: keep-alive\r\n");
        }
        // Transfer-Encoding (только для Chunked)
        if matches!(frame, Frame::Chunked) {
            head.extend_from_slice(b"Transfer-Encoding: chunked\r\n");
        }
        head.extend_from_slice(b"\r\n");
        let _ = self.out.write_all(&head);
        self.head_written = true;
    }

    /// do_flush — первый flush пишет head + буфер; далее chunk/raw.
    fn do_flush(&mut self) {
        if !self.head_written {
            if self.http11 {
                self.write_head(Frame::Chunked);
                if !self.buf.is_empty() {
                    self.write_chunk();
                }
                self.chunked = true;
            } else {
                // http10: close-delimited, без CL
                self.write_head(Frame::NoCL);
                if !self.buf.is_empty() {
                    let _ = self.out.write_all(&self.buf);
                    self.buf.clear();
                }
            }
            self.streaming = true;
        } else if !self.buf.is_empty() {
            if self.chunked {
                self.write_chunk();
            } else {
                let _ = self.out.write_all(&self.buf);
                self.buf.clear();
            }
        }
        let _ = self.out.flush();
    }

    fn write_chunk(&mut self) {
        let len = self.buf.len();
        let _ = self.out.write_all(format!("{:x}\r\n", len).as_bytes());
        let _ = self.out.write_all(&self.buf);
        let _ = self.out.write_all(b"\r\n");
        self.buf.clear();
    }

    /// finish — финализация ответа: head (если ещё не написан) + тело / terminating chunk.
    fn finish(mut self) {
        if self.head_written {
            if self.chunked {
                if !self.buf.is_empty() {
                    self.write_chunk();
                }
                let _ = self.out.write_all(b"0\r\n\r\n");
            } else if !self.buf.is_empty() {
                // http10 close-delimited: дописываем остаток
                let _ = self.out.write_all(&self.buf);
                self.buf.clear();
            }
        } else {
            let n = self.buf.len();
            if self.is_head {
                // HEAD: пустое → NoCL; 1..=2048 → Identity (тело подавлено); >2048 → NoCL
                if (1..=2048).contains(&n) {
                    self.write_head(Frame::Identity(n as u64));
                } else {
                    self.write_head(Frame::NoCL);
                }
            } else {
                if n == 0 {
                    self.write_head(Frame::Identity(0));
                } else if n <= 2048 {
                    self.write_head(Frame::Identity(n as u64));
                    let _ = self.out.write_all(&self.buf);
                    self.buf.clear();
                } else if self.http11 {
                    self.write_head(Frame::Chunked);
                    self.write_chunk();
                    let _ = self.out.write_all(b"0\r\n\r\n");
                } else {
                    self.write_head(Frame::NoCL);
                    let _ = self.out.write_all(&self.buf);
                    self.buf.clear();
                }
            }
        }
        let _ = self.out.flush();
        // drop → освобождение writer'а (pipelining)
    }
}

impl Write for RespWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.frozen = true; // неявный WriteHeader(200)
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.do_flush();
        Ok(())
    }
}

impl StreamWriter for RespWriter {
    fn set_header(&mut self, k: &str, v: &str) {
        self.put_header(k, v);
    }
}

// ─────────────────────────── response helpers ───────────────────────────

/// http_error — как http.Error: явный CT + nosniff ДО статуса, тело "msg\n".
fn http_error(w: &mut RespWriter, code: u16, msg: &str) {
    w.put_header("Content-Type", "text/plain; charset=utf-8");
    w.put_header("X-Content-Type-Options", "nosniff");
    w.set_status(code);
    let _ = w.write_all(format!("{}\n", msg).as_bytes());
}

/// json_err — как jsonErr: WriteHeader СНАЧАЛА (freeze) → заголовок игнорируется → sniffed CT.
fn json_err(w: &mut RespWriter, code: u16, msg: &str) {
    w.set_status(code);
    let _ = w.write_all(&go_json(&json!({ "error": msg })));
}

/// json_resp — как jsonResp: явный CT + Encoder + завершающий \n.
fn json_resp(w: &mut RespWriter, v: &Value) {
    w.put_header("Content-Type", "application/json");
    let _ = w.write_all(&go_json(v));
}

fn auth_check(w: &mut RespWriter, auth: &Auth, headers: &[Header]) -> bool {
    let token = extract_token(headers);
    if auth.valid(&token) {
        true
    } else {
        w.set_status(401);
        let _ = w.write_all(&go_json(&json!({ "error": "unauthorized" })));
        false
    }
}

/// extract_token — cookie "dt=" или Authorization "Bearer ".
fn extract_token(headers: &[Header]) -> String {
    for h in headers {
        if h.field.equiv("cookie") {
            for part in h.value.as_str().split(';') {
                let part = part.trim();
                if let Some(v) = part.strip_prefix("dt=") {
                    return v.to_string();
                }
            }
        }
    }
    for h in headers {
        if h.field.equiv("authorization") {
            let v = h.value.as_str();
            if v.len() > 7 && v.starts_with("Bearer ") {
                return v[7..].to_string();
            }
        }
    }
    String::new()
}

// ─────────────────────────── handlers ───────────────────────────

fn h_login(w: &mut RespWriter, method: &str, body: &[u8], auth: &Auth) {
    if method != "POST" {
        http_error(w, 405, "POST required");
        return;
    }
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let user = v.get("username").and_then(|x| x.as_str()).unwrap_or("");
    let pass = v.get("password").and_then(|x| x.as_str()).unwrap_or("");
    let token = auth.login(user, pass);
    if token.is_empty() {
        w.set_status(401);
        let _ = w.write_all(&go_json(&json!({ "error": "invalid credentials" })));
        return;
    }
    w.put_header(
        "Set-Cookie",
        &format!("dt={}; Path=/; Max-Age=86400; HttpOnly", token),
    );
    let _ = w.write_all(&go_json(&json!({ "status": "ok", "token": token })));
}

fn h_logout(w: &mut RespWriter, auth: &Auth, headers: &[Header]) {
    let token = extract_token(headers);
    if !token.is_empty() {
        auth.logout(&token);
    }
    w.put_header("Set-Cookie", "dt=; Path=/; Max-Age=0; HttpOnly");
    let _ = w.write_all(&go_json(&json!({ "status": "ok" })));
}

fn h_health(w: &mut RespWriter) {
    w.put_header("Content-Type", "application/json");
    let _ = w.write_all(b"{\"status\":\"ok\"}");
}

fn h_status(w: &mut RespWriter, cluster: &Cluster) {
    let containers = list_containers(false).len();
    let sites = list_services();
    let runners = runner_names();
    let mut m = serde_json::Map::new();
    m.insert("containers".into(), json!(containers));
    m.insert("runners".into(), json!(runners));
    m.insert("sites".into(), json!(sites));
    m.insert("version".into(), json!(crate::VERSION));
    let _ = w.write_all(&go_json(&Value::Object(m)));
}

fn h_containers(w: &mut RespWriter, method: &str, query: &str) {
    if method == "GET" {
        let show_all = query_get(query, "show_all").as_deref() == Some("true");
        let list = list_containers(show_all);
        json_resp(w, &json!(list));
    }
}

fn h_containers_sub(
    w: &mut RespWriter,
    method: &str,
    query: &str,
    body: &[u8],
    name: &str,
) {
    if name.is_empty() {
        json_err(w, 400, "name required");
        return;
    }
    if let Some(cname) = name.strip_suffix("/logs") {
        let tail = query_get(query, "tail")
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "200".to_string());
        w.put_header("Content-Type", "text/plain");
        let out = cmd_combined("docker", &["logs", "--tail", &tail, cname]);
        let _ = w.write_all(&out);
        return;
    }
    if let Some(cname) = name.strip_suffix("/exec") {
        if method == "POST" {
            let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            let cmd = v.get("cmd").and_then(|x| x.as_str()).unwrap_or("");
            if cmd.is_empty() {
                json_err(w, 400, "cmd required");
                return;
            }
            w.put_header("Content-Type", "text/plain");
            w.put_header("X-Accel-Buffering", "no");
            if let Err(e) = stream_exec("docker", &["exec", cname, "sh", "-c", cmd], w) {
                let _ = w.write_all(format!("Error: {}", e).as_bytes());
            }
            return;
        }
    }
    if let Some(cname) = name.strip_suffix("/inspect") {
        let out = cmd_output("docker", &["inspect", cname]).unwrap_or_default();
        w.put_header("Content-Type", "application/json");
        let stripped = if out.len() >= 8 { out[8..].to_string() } else { out.clone() };
        let _ = w.write_all(stripped.as_bytes());
        return;
    }
    if let Some(cname) = name.strip_suffix("/start") {
        run_cmd("docker", &["start", cname]);
        json_resp(w, &json!({ "status": "started" }));
        return;
    }
    if let Some(cname) = name.strip_suffix("/stop") {
        run_cmd("docker", &["stop", cname]);
        json_resp(w, &json!({ "status": "stopped" }));
        return;
    }
    if let Some(cname) = name.strip_suffix("/restart") {
        run_cmd("docker", &["restart", cname]);
        json_resp(w, &json!({ "status": "restarted" }));
        return;
    }
    if let Some(cname) = name.strip_suffix("/remove") {
        if method == "DELETE" {
            run_cmd("docker", &["rm", "-f", cname]);
            json_resp(w, &json!({ "status": "removed" }));
            return;
        }
    }
    if name.ends_with("/terminal") {
        http_error(w, 400, "Use WebSocket for terminal");
        return;
    }
    if let Some(cname) = name.strip_suffix("/edit") {
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let (port, inner) = parse_port_field(v.get("port"));
        let host_path = v.get("hostPath").and_then(|x| x.as_str()).unwrap_or("");
        let host_ip = v.get("hostIP").and_then(|x| x.as_str()).unwrap_or("");
        let typ = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
        match recreate_container(cname, port, host_path, host_ip, typ, inner) {
            Ok(()) => json_resp(w, &json!({ "status": "updated", "name": cname })),
            Err(e) => json_err(w, 400, &e),
        }
        return;
    }
    json_err(w, 404, "unknown action");
}

fn h_sites(w: &mut RespWriter, method: &str, body: &[u8]) {
    if method == "GET" {
        let names = list_services();
        let mut out = Vec::with_capacity(names.len());
        for name in &names {
            out.push(json!({
                "name": name,
                "status": get_container_status(name),
                "port": container_host_port(name),
                "image": get_container_image(name),
            }));
        }
        json_resp(w, &json!(out));
        return;
    }
    if method == "POST" {
        let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
        let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if name.is_empty() {
            json_err(w, 400, "name required");
            return;
        }
        let dir = v.get("dir").and_then(|x| x.as_str()).unwrap_or("");
        let dir = if dir.is_empty() {
            format!("/var/www/{}", name)
        } else {
            dir.to_string()
        };
        let mut lang = detect_lang(&dir);
        if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
            if !t.is_empty() {
                lang = LangInfo::new(t, t, api_default_port(t), Vec::new());
            }
        }
        let (mut port, mut inner) = parse_port_field(v.get("port"));
        if port == 0 {
            port = free_port(lang.port);
        }
        match deploy_site(name, &dir, port, &lang, inner) {
            Ok(actual) => {
                let _ = w.write_all(&go_json(&json!({
                    "status": "deployed", "name": name, "port": actual
                })));
            }
            Err(e) => json_err(w, 400, &e),
        }
    }
}

fn h_sites_sub(w: &mut RespWriter, method: &str, body: &[u8], name: &str) {
    if let Some(sname) = name.strip_suffix("/remove") {
        if method == "DELETE" {
            let safe = service_name(sname);
            remove_site(&safe);
            let _ = save_site_custom(&safe, SiteCustom::default());
            json_resp(w, &json!({ "status": "removed" }));
            return;
        }
    }
    if let Some(sname) = name.strip_suffix("/custom") {
        let safe = service_name(sname);
        match method {
            "GET" => {
                json_resp(w, &serde_json::to_value(get_site_custom(&safe)).unwrap_or(Value::Null));
                return;
            }
            "POST" | "PUT" => {
                #[derive(serde::Deserialize)]
                struct CustomReq {
                    #[serde(default)]
                    tools: Option<Vec<String>>,
                    #[serde(default)]
                    env: Option<BTreeMap<String, String>>,
                    #[serde(default)]
                    volumes: Option<Vec<String>>,
                    #[serde(default)]
                    apply: Option<bool>,
                }
                let req: CustomReq = match serde_json::from_slice(body) {
                    Ok(r) => r,
                    Err(_) => {
                        json_err(w, 400, "invalid json");
                        return;
                    }
                };
                let tools: Vec<String> = req
                    .tools
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
                let volumes: Vec<String> = req
                    .volumes
                    .unwrap_or_default()
                    .into_iter()
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect();
                let mut env = BTreeMap::new();
                for (k, val) in req.env.unwrap_or_default() {
                    let k = k.trim();
                    if !k.is_empty() {
                        env.insert(k.to_string(), val);
                    }
                }
                if let Err(e) = save_site_custom(&safe, SiteCustom { tools, env, volumes }) {
                    json_err(w, 500, &e);
                    return;
                }
                let mut resp = serde_json::Map::new();
                resp.insert("status".into(), json!("saved"));
                resp.insert("name".into(), json!(sname));
                if req.apply.unwrap_or(false) {
                    let (dir, port, inner, ok) = site_compose_info(&safe);
                    if !ok {
                        json_err(w, 400, "site not found in compose — save only");
                        return;
                    }
                    let lang = detect_lang(&dir);
                    match deploy_site(sname, &dir, port, &lang, inner) {
                        Ok(actual) => {
                            resp.insert("status".into(), json!("applied"));
                            resp.insert("port".into(), json!(actual));
                        }
                        Err(e) => {
                            json_err(w, 400, &e);
                            return;
                        }
                    }
                }
                json_resp(w, &Value::Object(resp));
                return;
            }
            _ => {
                json_err(w, 405, "method not allowed");
                return;
            }
        }
    }
    json_err(w, 404, "unknown");
}

fn h_runners_list(w: &mut RespWriter, method: &str) {
    if method != "GET" {
        return;
    }
    let names = runner_names();
    let mut out = Vec::with_capacity(names.len());
    for name in &names {
        let status = get_container_status(name);
        let image = cmd_output("docker", &["inspect", "-f", "{{.Config.Image}}", name])
            .map(|s| s.trim().to_string())
            .unwrap_or_default();
        let labels = runner_labels(name);
        out.push(json!({ "name": name, "status": status, "image": image, "labels": labels }));
    }
    json_resp(w, &json!(out));
}

fn h_runners_add(w: &mut RespWriter, body: &[u8]) {
    #[derive(serde::Deserialize)]
    struct AddReq {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        url: Option<String>,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        labels: Option<String>,
        #[serde(default)]
        #[allow(dead_code)]
        capacity: Option<i64>,
        #[serde(default)]
        #[allow(dead_code)]
        platform: Option<String>,
    }
    let req: AddReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(_) => {
            json_err(w, 400, "invalid json");
            return;
        }
    };
    let name = req.name.unwrap_or_default();
    let token = req.token.unwrap_or_default();
    let mut url = req.url.unwrap_or_default();
    if url.is_empty() {
        url = "http://10.0.0.1:3000".to_string();
    }
    let _ = crate::deploy::add_runner(&name, &token, &url);
    json_resp(w, &json!({ "status": "created", "name": name }));
}

fn h_runners_sub(w: &mut RespWriter, method: &str, body: &[u8], name: &str) {
    if let Some(cname) = name.strip_suffix("/logs") {
        w.put_header("Content-Type", "text/plain");
        let out = cmd_combined("docker", &["logs", "--tail", "200", cname]);
        let _ = w.write_all(&out);
        return;
    }
    if let Some(cname) = name.strip_suffix("/restart") {
        run_cmd("docker", &["restart", cname]);
        json_resp(w, &json!({ "status": "restarted" }));
        return;
    }
    if let Some(cname) = name.strip_suffix("/stop") {
        run_cmd("docker", &["stop", cname]);
        json_resp(w, &json!({ "status": "stopped" }));
        return;
    }
    if let Some(cname) = name.strip_suffix("/start") {
        run_cmd("docker", &["start", cname]);
        json_resp(w, &json!({ "status": "started" }));
        return;
    }
    if let Some(cname) = name.strip_suffix("/edit") {
        if method == "POST" {
            #[derive(serde::Deserialize)]
            struct EditReq {
                #[serde(default)]
                labels: Option<String>,
            }
            let req: EditReq = match serde_json::from_slice(body) {
                Ok(r) => r,
                Err(_) => {
                    json_err(w, 400, "invalid json");
                    return;
                }
            };
            let cfg = runner_config_file(cname);
            let Ok(data) = std::fs::read_to_string(&cfg) else {
                json_err(w, 404, "runner config not found");
                return;
            };
            let mut rf = serde_json::from_str::<Value>(&data).unwrap_or_else(|_| json!({}));
            if !rf.is_object() {
                rf = json!({});
            }
            let labels: Vec<String> = req
                .labels
                .unwrap_or_default()
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            rf["labels"] = if labels.is_empty() {
                Value::Null
            } else {
                json!(labels)
            };
            let _ = std::fs::write(&cfg, go_pretty(&rf));
            run_cmd("docker", &["restart", cname]);
            json_resp(w, &json!({ "status": "updated" }));
            return;
        }
    }
    if method == "DELETE" {
        run_cmd("docker", &["rm", "-f", name]);
        json_resp(w, &json!({ "status": "removed" }));
        return;
    }
    json_resp(w, &json!({ "name": name }));
}

fn h_servers(w: &mut RespWriter, method: &str, body: &[u8], cluster: &Cluster) {
    let body_str = if matches!(method, "POST" | "PUT" | "PATCH") {
        Some(String::from_utf8_lossy(body).to_string())
    } else {
        None
    };
    let (code, v) = cluster.handle_servers(method, body_str.as_deref());
    if code == 200 && v.is_null() {
        return; // пустой 200
    }
    if code == 200 {
        json_resp(w, &v);
    } else {
        w.set_status(code);
        let _ = w.write_all(&go_json(&v));
    }
}

fn h_servers_sub(
    w: &mut RespWriter,
    method: &str,
    body: &[u8],
    query: &str,
    name: &str,
    cluster: &Cluster,
) {
    let body_str = if matches!(method, "POST" | "PUT" | "PATCH") {
        Some(String::from_utf8_lossy(body).to_string())
    } else {
        None
    };
    let sw: &mut dyn StreamWriter = w;
    let reply = cluster.handle_server(method, name, body_str.as_deref(), query, Some(sw));
    match reply {
        ServerReply::Streamed => {}
        ServerReply::RawJson(data) => {
            w.put_header("Content-Type", "application/json");
            let _ = w.write_all(&data);
        }
        ServerReply::Json(200, v) => json_resp(w, &v),
        ServerReply::Json(code, v) => {
            w.set_status(code);
            let _ = w.write_all(&go_json(&v));
        }
    }
}

fn h_cluster_status(w: &mut RespWriter, method: &str, cluster: &Cluster) {
    if method != "GET" {
        json_err(w, 405, "GET required");
        return;
    }
    json_resp(w, &cluster.handle_status());
}

fn h_deploy(w: &mut RespWriter, method: &str, body: &[u8]) {
    if method != "POST" {
        json_err(w, 405, "POST required");
        return;
    }
    let v: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let mut name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let dir = v.get("dir").and_then(|x| x.as_str()).unwrap_or("");
    let dir = if dir.is_empty() {
        if name.is_empty() {
            json_err(w, 400, "name or dir required");
            return;
        }
        format!("/var/www/{}", name)
    } else {
        dir.to_string()
    };
    let mut lang = detect_lang(&dir);
    if let Some(t) = v.get("type").and_then(|x| x.as_str()) {
        if !t.is_empty() {
            lang = LangInfo::new(t, t, api_default_port(t), Vec::new());
        }
    }
    if name.is_empty() {
        name = Path::new(&dir)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.clone());
    }
    let (mut port, mut inner) = parse_port_field(v.get("port"));
    if port == 0 {
        port = free_port(lang.port);
    }
    match deploy_site(&name, &dir, port, &lang, inner) {
        Ok(actual) => json_resp(
            w,
            &json!({
                "status": "deployed", "name": name, "port": actual, "type": lang.name
            }),
        ),
        Err(e) => json_err(w, 400, &e),
    }
}

// ─────────────────────────── list_containers / recreate ───────────────────────────

fn list_containers(show_all: bool) -> Vec<Value> {
    let mut containers: Vec<Value> = Vec::new();
    let infra = [
        "runner",
        "docker_dind",
        "deploy-web",
        "deploy-stack",
        "deploy-stack-runner",
    ];
    let (code, out) = run_capture(
        "docker",
        &["ps", "-a", "--format", "{{.Names}}|{{.Status}}|{{.Image}}"],
        15,
    );
    if code != 0 {
        return containers;
    }
    let text = String::from_utf8_lossy(&out);
    let trimmed = text.trim();
    let mut all: Vec<(String, String, String)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in trimmed.split('\n') {
        let parts: Vec<&str> = line.splitn(3, '|').collect();
        let name = parts[0];
        let status = parts.get(1).copied().unwrap_or("");
        let image = parts.get(2).copied().unwrap_or("");
        if seen.contains(name) {
            continue;
        }
        seen.insert(name);
        all.push((name.to_string(), status.to_string(), image.to_string()));
    }
    // Batch: монты
    let mut mount_map = std::collections::HashSet::new();
    if !all.is_empty() {
        let names: Vec<&str> = all.iter().map(|(n, _, _)| n.as_str()).collect();
        let mut args = vec!["inspect", "--format"];
        args.push("{{.Name}}|{{index .Config.Labels \"deploy-stack.site\"}}|{{range .Mounts}}{{.Source}}:{{.Destination}}|{{end}}");
        args.extend_from_slice(&names);
        let (_, mount_out) = run_capture("docker", &args, 10);
        let mtext = String::from_utf8_lossy(&mount_out);
        for line in mtext.trim().split('\n') {
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() < 3 {
                continue;
            }
            let mname = parts[0].trim_start_matches('/');
            let label = parts[1].trim();
            let mounts = parts[2];
            if !label.is_empty() || mounts.contains("/var/www") {
                mount_map.insert(mname.to_string());
            }
        }
    }
    // Порты по-контейнерно
    for (name, status, image) in all {
        let managed = mount_map.contains(&name);
        if !show_all && (infra.contains(&name.as_str()) || name.starts_with("runner-") || !managed) {
            continue;
        }
        let (_, port_out) = run_capture(
            "docker",
            &[
                "inspect",
                "--format",
                "{{range $p, $conf := .NetworkSettings.Ports}}{{with $conf}}{{range .}}{{.HostPort}}->{{$p}} {{end}}{{end}}{{end}}",
                &name,
            ],
            3,
        );
        let ptext = String::from_utf8_lossy(&port_out);
        let mut seen_ports = std::collections::HashSet::new();
        let mut ports = Vec::new();
        for field in ptext.split_whitespace() {
            if seen_ports.insert(field.to_string()) {
                ports.push(field.to_string());
            }
        }
        let mut item = serde_json::Map::new();
        item.insert("name".into(), json!(name));
        item.insert("status".into(), json!(status));
        item.insert("image".into(), json!(image));
        item.insert("ports".into(), json!(ports.join(" ")));
        item.insert("managed".into(), json!(managed));
        containers.push(Value::Object(item));
    }
    containers
}

fn recreate_container(
    name: &str,
    new_port: u16,
    host_path: &str,
    host_ip: &str,
    _typ: &str,
    inner_override: Option<u16>,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let rem = || -> u64 {
        deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_secs()
    };
    let get = |sep: &str| -> String {
        cmd_output_timeout("docker", &["inspect", "-f", sep, name], rem())
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let image = get("{{.Config.Image}}");
    if image.is_empty() {
        return Err("контейнер не найден".to_string());
    }
    // внутренний порт: явный override (например 8282:8080) либо авто по ExposedPorts
    let mut inner = "80".to_string();
    if let Some(io) = inner_override {
        inner = io.to_string();
    } else {
        let exposed = get("{{range $p, $_ := .Config.ExposedPorts}}{{$p}} {{end}}");
        for e in exposed.split_whitespace() {
            if e.starts_with("80") {
                inner = "80".to_string();
                break;
            }
            let p = e.split('/').next().unwrap_or("");
            if !p.is_empty() {
                inner = p.to_string();
                break;
            }
        }
    }
    // bind-маунты
    let mounts_json = get("{{json .Mounts}}");
    let mounts: Vec<serde_json::Value> =
        serde_json::from_str(&mounts_json).unwrap_or_default();
    let mut binds: Vec<(String, String, bool)> = Vec::new();
    for m in &mounts {
        if m.get("Type").and_then(|x| x.as_str()) != Some("bind") {
            continue;
        }
        let mut src = m.get("Source").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let dst = m.get("Destination").and_then(|x| x.as_str()).unwrap_or("").to_string();
        let rw = m.get("RW").and_then(|x| x.as_bool()).unwrap_or(true);
        if !host_path.is_empty() && (dst == "/usr/share/nginx/html" || dst == "/var/www/html") {
            src = host_path.to_string();
        }
        binds.push((src, dst, rw));
    }
    // внешний порт
    let mut host_port = new_port;
    if host_port == 0 {
        let pb = get("{{json .NetworkSettings.Ports}}");
        let pm: BTreeMap<String, Vec<serde_json::Value>> =
            serde_json::from_str(&pb).unwrap_or_default();
        for arr in pm.values() {
            if let Some(first) = arr.first() {
                let hp = first.get("HostPort").and_then(|x| x.as_str()).unwrap_or("");
                let digits: String = hp.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
                host_port = digits.parse().unwrap_or(0);
                break;
            }
        }
        if host_port == 0 {
            host_port = 8080;
        }
    }
    let host_ip = if host_ip.is_empty() { "0.0.0.0" } else { host_ip };
    if binds.is_empty() {
        return Err("не удалось определить каталог-источник контейнера".to_string());
    }
    for (src, _, _) in &binds {
        if src.is_empty() {
            return Err("не удалось определить каталог-источник контейнера".to_string());
        }
    }
    run_cmd("docker", &["rm", "-f", name]);
    let mut args = vec![
        "run".to_string(),
        "-d".to_string(),
        "--name".to_string(),
        name.to_string(),
        "--restart".to_string(),
        "unless-stopped".to_string(),
        "-p".to_string(),
        format!("{}:{}:{}", host_ip, host_port, inner),
    ];
    for (src, dst, rw) in &binds {
        let ro = if *rw { "" } else { ":ro" };
        args.push("-v".to_string());
        args.push(format!("{}:{}{}", src, dst, ro));
    }
    args.push(image.clone());
    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let (code, out) = run_capture("docker", &arg_refs, rem());
    if code != 0 {
        let desc = if code == -1 {
            "signal: killed".to_string()
        } else {
            format!("exit status {}", code)
        };
        let out_str = String::from_utf8_lossy(&out);
        return Err(format!(
            "не удалось пересоздать: {}: {}",
            desc,
            out_str.trim()
        ));
    }
    Ok(())
}

// ─────────────────────────── static / routing ───────────────────────────

fn serve_static_file(w: &mut RespWriter, name: &str) {
    let paths = [
        Path::new(&deploy_home()).join("static").join(name),
        Path::new("/usr/local/share/deploy-stack/static").join(name),
        argv0_dir().join("static").join(name),
        Path::new("/root/deploy-stack/static").join(name),
    ];
    for p in &paths {
        if let Ok(data) = std::fs::read(p) {
            let ct = if name.ends_with(".html") {
                "text/html; charset=utf-8"
            } else {
                "text/plain"
            };
            w.put_header("Content-Type", ct);
            let _ = w.write_all(&data);
            return;
        }
    }
    http_error(w, 404, &format!("{} not found", name));
}

use crate::compose::site_compose_info;

fn route(
    w: &mut RespWriter,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
    headers: &[Header],
    auth: &Auth,
    cluster: &Cluster,
) {
    // ── точные маршруты без auth ──
    match path {
        "/api/v1/login" => {
            h_login(w, method, body, auth);
            return;
        }
        "/api/v1/logout" => {
            h_logout(w, auth, headers);
            return;
        }
        "/health" => {
            h_health(w);
            return;
        }
        "/" => {
            serve_static_file(w, "index.html");
            return;
        }
        _ => {}
    }
    // ── точные маршруты с auth ──
    if !auth_check(w, auth, headers) {
        return;
    }
    match path {
        "/api/v1/containers" => {
            h_containers(w, method, query);
            return;
        }
        "/api/v1/sites" => {
            h_sites(w, method, body);
            return;
        }
        "/api/v1/runners" => {
            h_runners_list(w, method);
            return;
        }
        "/api/v1/runners/add" => {
            h_runners_add(w, body);
            return;
        }
        "/api/v1/servers" => {
            h_servers(w, method, body, cluster);
            return;
        }
        "/api/v1/cluster/status" => {
            h_cluster_status(w, method, cluster);
            return;
        }
        "/api/v1/status" => {
            h_status(w, cluster);
            return;
        }
        "/api/v1/deploy" => {
            h_deploy(w, method, body);
            return;
        }
        "/api/v1/docs" => {
            serve_static_file(w, "api_docs.html");
            return;
        }
        _ => {}
    }
    // ── префиксы ──
    if let Some(rest) = path.strip_prefix("/api/v1/containers/") {
        h_containers_sub(w, method, query, body, rest);
        return;
    }
    if let Some(rest) = path.strip_prefix("/api/v1/sites/") {
        h_sites_sub(w, method, body, rest);
        return;
    }
    if let Some(rest) = path.strip_prefix("/api/v1/runners/") {
        h_runners_sub(w, method, body, rest);
        return;
    }
    if let Some(rest) = path.strip_prefix("/api/v1/servers/") {
        h_servers_sub(w, method, body, query, rest, cluster);
        return;
    }
    http_error(w, 404, "404 page not found");
}

fn handle_request(mut req: Request, auth: &Auth, cluster: &Cluster) {
    let method = req.method().as_str().to_string();
    let url = req.url().to_string();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url, String::new()),
    };
    let http11 = *req.http_version() == (1, 1);
    let is_head = method == "HEAD";
    let mut wants_close = false;
    let mut wants_keepalive = false;
    for h in req.headers() {
        if h.field.equiv("connection") {
            let v = h.value.as_str();
            if v.eq_ignore_ascii_case("close") {
                wants_close = true;
            }
            if v.eq_ignore_ascii_case("keep-alive") {
                wants_keepalive = true;
            }
        }
    }
    let headers = req.headers().to_vec();
    let mut body = Vec::new();
    let _ = req.as_reader().read_to_end(&mut body);
    let writer = req.into_writer();
    let mut w = RespWriter::new(writer, is_head, http11, wants_close, wants_keepalive);
    route(&mut w, &method, &path, &query, &body, &headers, auth, cluster);
    w.finish();
}

pub fn start_web_server(port: u16) {
    let auth = Arc::new(Auth::new());
    let cluster = Arc::new(Cluster::new(&format!("{}/data", deploy_home())));
    let server = Server::http(("0.0.0.0", port)).expect("bind");
    eprintln!("deploy-stack v{} on :{}", crate::VERSION, port);
    eprintln!("Web:  http://localhost:{}", port);
    eprintln!("API:  http://localhost:{}/api/v1/", port);
    for request in server.incoming_requests() {
        let auth = auth.clone();
        let cluster = cluster.clone();
        std::thread::spawn(move || {
            handle_request(request, &auth, &cluster);
        });
    }
}
