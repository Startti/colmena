#![cfg(target_os = "linux")]
//! `python_executor serve` as a process, with the real binary. Its calls run
//! in the process jail, which needs root and CAP_SYS_ADMIN, so the served
//! test runs only with `COLMENA_PYEXEC_JAIL_TESTS=1`.

use colmena::dag_engine::infrastructure::python_exec::jail::DEFAULT_HIDDEN;
use colmena::dag_engine::infrastructure::python_exec::protocol::REFUSED_MESSAGE;
use serde_json::{json, Value};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// Stops the server when the test ends, however it ends.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        unsafe { libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM) };
        let _ = self.0.wait();
    }
}

/// A call with `token` in mode `none`: its status and its body.
async fn call(url: &str, token: &str, code: &str, inputs: Value) -> (u16, String) {
    let body = json!({"v": 1, "code": code, "mode": "none", "timeout_ms": 30000, "inputs": inputs});
    let req = reqwest::Client::new()
        .post(format!("{url}/v1/run"))
        .bearer_auth(token);
    let resp = req.json(&body).send().await.unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

/// A served call needs the token and runs as an unprivileged user in the
/// jail, with pandas, under the output policy. It cannot read the token file,
/// by the path the server was given or by the one that path links to, while a
/// file next to it stays readable.
#[tokio::test]
async fn a_served_call_needs_the_token_and_cannot_read_it() {
    if std::env::var("COLMENA_PYEXEC_JAIL_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set COLMENA_PYEXEC_JAIL_TESTS=1 (Linux, root, CAP_SYS_ADMIN)");
        return;
    }
    // Outside /tmp and the default hidden paths, which would hide it anyway.
    let dir = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let root = dir.path();
    let covered = |h: &&str| root.starts_with(h);
    assert!(!root.starts_with("/tmp") && !DEFAULT_HIDDEN.iter().any(covered));
    let (token, mode) = ("t".repeat(40), PermissionsExt::from_mode);
    std::fs::create_dir(root.join("..data")).unwrap();
    for (name, text) in [("..data/token", token.as_str()), ("visible", "visible")] {
        std::fs::write(root.join(name), text).unwrap();
        std::fs::set_permissions(root.join(name), mode(0o444)).unwrap();
    }
    for d in [root.to_path_buf(), root.join("..data")] {
        std::fs::set_permissions(d, mode(0o755)).unwrap();
    }
    symlink("..data/token", root.join("token")).unwrap();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut serve = Command::new(env!("CARGO_BIN_EXE_python_executor"));
    serve.args([
        "serve",
        "--listen",
        &format!("127.0.0.1:{port}"),
        "--token-file",
    ]);
    let serve = serve
        .arg(root.join("token"))
        .env("COLMENA_PYTHON_EXECUTOR_SLOTS", "1");
    let serve = serve.env("COLMENA_PYTHON_EXECUTOR_REFUSE_OUTPUT", "MARK3R.");
    let mut server = Server(serve.spawn().unwrap());
    let url = format!("http://127.0.0.1:{port}");
    let t0 = Instant::now();
    while reqwest::get(format!("{url}/readyz"))
        .await
        .map(|r| r.status().as_u16())
        .ok()
        != Some(200)
    {
        assert!(server.0.try_wait().unwrap().is_none(), "the server exited");
        assert!(t0.elapsed() < Duration::from_secs(60), "never ready");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let code =
        "import os, pandas as pd\nprint(os.getuid() != 0)\noutput = int(pd.Series([1, 2]).sum())";
    assert_eq!(
        call(&url, &token[1..], code, json!({})).await,
        (401, String::new())
    );
    let (status, text) = call(&url, &token, code, json!({})).await;
    let out: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (status, &out["output"], &out["stdout"]),
        (200, &json!(3), &json!("True\n"))
    );

    let paths = [
        root.join("token"),
        root.join("..data/token"),
        root.join("visible"),
    ];
    let code = "def read(p):\n    try:\n        return open(p).read()\n    except OSError as e:\n        return type(e).__name__\noutput = [read(p) for p in paths]";
    let (_, text) = call(&url, &token, code, json!({"paths": paths})).await;
    assert!(!text.contains(&token), "{text}");
    let out: Value = serde_json::from_str(&text).unwrap();
    let expected = json!(["PermissionError", "PermissionError", "visible"]);
    assert_eq!(out["output"], expected, "{text}");

    let (_, text) = call(&url, &token, "output = 'MARK3R.x'", json!({})).await;
    let out: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        (&out["status"], &out["message"]),
        (&json!("python_error"), &json!(REFUSED_MESSAGE))
    );
    assert_eq!(out.get("output"), None, "{text}");
    let (_, text) = call(&url, &token, "output = 'MARKER.x'", json!({})).await;
    assert_eq!(
        serde_json::from_str::<Value>(&text).unwrap()["output"],
        json!("MARKER.x")
    );
}

/// An egress target that is not `host:port` stops `serve` before anything
/// starts; it is not taken as a target that refuses connections.
#[test]
fn a_malformed_egress_target_is_refused_at_startup() {
    let mut serve = Command::new(env!("CARGO_BIN_EXE_python_executor"));
    serve.args([
        "serve",
        "--listen",
        "127.0.0.1:0",
        "--require-closed-egress",
    ]);
    let serve = serve
        .arg("127.0.0.1:443,127.0.0.1")
        .stderr(std::process::Stdio::piped());
    let mut server = Server(serve.spawn().unwrap());
    let t0 = Instant::now();
    while server.0.try_wait().unwrap().is_none() && t0.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(server.0.try_wait().unwrap().and_then(|s| s.code()), Some(2));
    let mut stderr = String::new();
    std::io::Read::read_to_string(&mut server.0.stderr.take().unwrap(), &mut stderr).unwrap();
    assert!(stderr.contains("'127.0.0.1'"), "{stderr}");
}
