//! Высокоуровневые сценарии через настоящий бинарник.
//!
//! Покрывают то, что не ловят юнит-тесты: CLI (`validate-config`, `status`),
//! feature `api` (`serve` + REST-эндпоинты).
//!
//! Мокируются только границы: боевой Авито недоступен, поэтому полный
//! пайплайн `run` проверяется отдельным e2e против локального мока
//! (`tests/e2e_mock.rs`).

use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "api")]
use std::process::Stdio;

/// Найти собранный бинарник рядом с тестом.
fn bin() -> PathBuf {
    let mut p = std::env::current_exe().expect("current_exe");
    p.pop();
    if p.join("../avito-rs").exists() {
        p.pop();
    }
    let cand = p.join("avito-rs");
    assert!(
        cand.exists(),
        "avito-rs binary not found next to {}",
        p.display()
    );
    cand
}

/// Записать конфиг и вернуть путь.
fn write_config(dir: &Path) -> PathBuf {
    let cfg = dir.join("it.toml");
    std::fs::write(
        &cfg,
        format!(
            "interval_sec = 1\npages_per_url = 1\ndb_path = '{}'\nurls = []\n",
            dir.join("it.db").display()
        ),
    )
    .expect("write config");
    cfg
}

#[test]
fn validate_config_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let out = Command::new(bin())
        .args(["validate-config", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("config OK"), "unexpected stdout: {stdout}");
}

#[test]
fn status_ok() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let out = Command::new(bin())
        .args(["status", "--config"])
        .arg(&cfg)
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("db:"));
}

#[test]
fn run_once_empty_urls_exits_nonzero() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());
    let out = Command::new(bin())
        .args(["run", "--config"])
        .arg(&cfg)
        .arg("--once")
        .output()
        .unwrap();
    assert!(!out.status.success(), "пустой urls должен быть ошибкой");
}

#[cfg(feature = "api")]
#[test]
fn api_serve_reports_status() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = write_config(dir.path());

    // Свободный порт: занять и сразу отпустить.
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let mut child = Command::new(bin())
        .args(["serve", "--config"])
        .arg(&cfg)
        .arg("--bind")
        .arg(format!("127.0.0.1:{port}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    // Ждём подъёма до 5с.
    let client = reqwest::blocking::Client::new();
    let mut ok = false;
    for _ in 0..50 {
        if let Ok(r) = client
            .get(format!("http://127.0.0.1:{port}/status"))
            .timeout(std::time::Duration::from_millis(300))
            .send()
        {
            if r.status().is_success() {
                ok = true;
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(ok, "API не ответил на /status за 5с");
}
