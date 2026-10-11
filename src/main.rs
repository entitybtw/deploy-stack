// deploy-stack — Universal site deployer (Rust rewrite of Go v1.5.3).
// CLI-диспетчер: точный порт Go main() + новый синтаксис порта N[:M].
mod auth;
mod cluster;
mod compose;
mod config;
mod deploy;
mod lang;
mod serve;
mod sync;
mod util;

use crate::compose::{container_host_port, free_port, list_services, runner_names, service_name, PortSpec};
use crate::config::{default_port, host_ip, parse_flags, templates};
use crate::deploy::{
    add_runner, auto_deploy, deploy_site, docker_compose, get_container_status, interactive,
    remove_all_sites, remove_site, usage,
};
use crate::lang::{detect_lang, LangInfo};
use crate::util::{is_ci, is_tty, run_cmd};

pub const VERSION: &str = "2.0.0";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() {
        if is_ci() || !is_tty() {
            auto_deploy(&[]);
        } else {
            interactive();
        }
        return;
    }

    match args[0].as_str() {
        "add" => {
            if args.len() < 3 {
                println!("Usage: deploy add <name> <dir> [--port N[:M]] [--type T]");
                return;
            }
            let name = &args[1];
            let dir = &args[2];
            let opts = parse_flags(&args[3..]);
            let mut lang = detect_lang(dir);
            if let Some(t) = opts.get("type") {
                if !t.is_empty() && templates().iter().any(|(k, _)| k == t) {
                    lang = LangInfo::new(t, t, default_port(t), Vec::new());
                }
            }
            // --port N[:M] — внешний[:внутренний]; пусто/0 → авто.
            let mut port: u16 = 0;
            let mut inner: Option<u16> = None;
            if let Some(p) = opts.get("port") {
                let spec = PortSpec::parse(p);
                port = spec.external;
                inner = spec.internal;
            }
            if port == 0 {
                port = free_port(lang.port);
            }
            match deploy_site(name, dir, port, &lang, inner) {
                Ok(actual) => println!("✓ {} deployed: http://{}:{}", name, host_ip(), actual),
                Err(e) => {
                    println!("✗ {}", e);
                    std::process::exit(1);
                }
            }
        }

        "rm" => {
            if args.len() < 2 {
                println!("Usage: deploy rm <name>");
                return;
            }
            let n = service_name(&args[1]);
            remove_site(&n);
            println!("✓ {} removed", n);
        }

        "list" => {
            for s in list_services() {
                let st = get_container_status(&s);
                let p = container_host_port(&s);
                println!("  {:<24} port={:<6} {}", s, p, st);
            }
        }

        "runners" => {
            for r in runner_names() {
                println!("  {:<24} {}", r, get_container_status(&r));
            }
        }

        "add-runner" => {
            if args.len() < 3 {
                println!("Usage: deploy add-runner <name> <token> [--url URL]");
                return;
            }
            let mut url = "http://10.0.0.1:3000".to_string();
            for (i, a) in args.iter().enumerate() {
                if a == "--url" && i + 1 < args.len() {
                    url = args[i + 1].clone();
                }
            }
            add_runner(&args[1], &args[2], &url);
        }

        "rm-runner" => {
            let n = args.get(1).cloned().unwrap_or_default();
            if n.is_empty() {
                for r in runner_names() {
                    run_cmd("docker", &["rm", "-f", &r]);
                }
                crate::compose::write_runners_compose(&crate::compose::default_compose());
            } else {
                let svc = service_name(&format!("runner-{}", n));
                docker_compose_file_runner("down", &svc);
                let content = crate::compose::remove_service(&crate::compose::read_runners_compose(), &svc);
                crate::compose::write_runners_compose(&content);
            }
            println!("✓ Runner removed");
        }

        "logs" => {
            if args.len() < 2 {
                println!("Usage: deploy logs <name> [--tail N]");
                return;
            }
            let mut tail = "200".to_string();
            for (i, a) in args.iter().enumerate() {
                if a == "--tail" && i + 1 < args.len() {
                    tail = args[i + 1].clone();
                }
            }
            let svc = service_name(&args[1]);
            run_cmd("docker", &["logs", "--tail", &tail, &svc]);
        }

        "restart" => {
            if args.len() < 2 {
                println!("Usage: deploy restart <name>");
                return;
            }
            let svc = service_name(&args[1]);
            run_cmd("docker", &["restart", &svc]);
        }

        "status" => {
            docker_compose(&["ps"]);
        }

        "up" => {
            if args.len() > 1 {
                let svc = service_name(&args[1]);
                docker_compose(&["up", "-d", &svc]);
            } else {
                docker_compose(&["up", "-d"]);
            }
        }

        "down" => {
            remove_all_sites();
        }

        "deploy" | "auto" => {
            auto_deploy(&args[1..]);
        }

        "serve" => {
            let mut port: u16 = 3000;
            let mut i = 0;
            while i < args.len() {
                let a = &args[i];
                if a == "--port" && i + 1 < args.len() {
                    port = args[i + 1].parse().unwrap_or(0);
                    i += 2;
                    continue;
                }
                if let Some(v) = a.strip_prefix("--port=") {
                    port = v.parse().unwrap_or(0);
                }
                i += 1;
            }
            serve::start_web_server(port);
        }

        "help" | "-h" | "--help" => usage(),

        other => {
            if other.starts_with('-') {
                auto_deploy(&args);
            } else {
                usage();
            }
        }
    }
}

/// dockerComposeFile(runnersComposePath(), "down", svc) — лёгкая обёртка.
fn docker_compose_file_runner(action: &str, svc: &str) {
    let path = crate::config::runners_compose_path();
    crate::deploy::docker_compose_file(&path, &[action, svc]);
}
