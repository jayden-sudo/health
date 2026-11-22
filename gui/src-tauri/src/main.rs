//! Desktop shell (plan §6): starts the bundled `health serve`, restarts it if it dies, and relays the UI's calls.
//! The UI never sees the owner token and never makes a network request; it only calls the commands here.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod clients;

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

#[derive(Clone)]
struct Conn {
    port: u16,
    token: String,
}

#[derive(Default)]
struct Shell {
    conn: Mutex<Option<Conn>>,
    child: Mutex<Option<Child>>,
    quitting: AtomicBool,
    /// Why the core is not up (shown by the UI), e.g. "no_profile: …"
    last_error: Mutex<Option<String>>,
}

/// The core binary: next to this executable when bundled (Tauri `externalBin`); HEALTH_BIN in debug builds only.
fn core_bin() -> PathBuf {
    if cfg!(debug_assertions)
        && let Some(p) = std::env::var_os("HEALTH_BIN")
    {
        return p.into();
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.with_file_name(if cfg!(windows) { "health.exe" } else { "health" })
}

fn core_command() -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(core_bin());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console window behind the app
    }
    c
}

/// One request to serve on 127.0.0.1. HTTP/1.0, so the reply always comes with a Content-Length (never chunked).
fn http(c: &Conn, method: &str, path: &str, body: &str) -> Result<(u16, Value), String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], c.port).into(), Duration::from_secs(5)).map_err(|e| e.to_string())?;
    // a wedged core must not hang the UI or the supervisor: short for the handshake, generous for ops (imports)
    let wait = if path == "/v1/meta" { 10 } else { 900 };
    s.set_read_timeout(Some(Duration::from_secs(wait))).map_err(|e| e.to_string())?;
    let req = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        c.port,
        c.token,
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    s.read_to_end(&mut raw).map_err(|e| e.to_string())?;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("bad response from the core")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head.split_whitespace().nth(1).and_then(|s| s.parse().ok()).ok_or("bad status from the core")?;
    let body = &raw[split + 4..];
    let v = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(body).map_err(|e| format!("unreadable reply from the core: {e}"))?
    };
    Ok((status, v))
}

fn unavailable(shell: &Shell) -> Value {
    let why = shell.last_error.lock().unwrap().clone();
    json!({ "code": "unavailable", "message": why.unwrap_or_else(|| "the health core is starting".into()) })
}

#[tauri::command(async)]
fn op(shell: tauri::State<'_, Arc<Shell>>, name: String, req: Value) -> Result<Value, Value> {
    let conn = shell.conn.lock().unwrap().clone().ok_or_else(|| unavailable(&shell))?;
    if !name.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '_') {
        return Err(json!({ "code": "no_op", "message": name }));
    }
    let (status, body) =
        http(&conn, "POST", &format!("/v1/op/{name}"), &req.to_string()).map_err(|e| json!({ "code": "unavailable", "message": e }))?;
    if status == 200 {
        Ok(body["value"].clone())
    } else {
        // always an {code, message} the UI can show, even when the reply has no error body
        Err(body
            .get("error")
            .filter(|e| e.is_object())
            .cloned()
            .unwrap_or_else(|| json!({ "code": "error", "message": format!("HTTP {status}") })))
    }
}

#[tauri::command(async)]
fn unlock(shell: tauri::State<'_, Arc<Shell>>, passphrase: String) -> Result<Value, Value> {
    let conn = shell.conn.lock().unwrap().clone().ok_or_else(|| unavailable(&shell))?;
    let (status, body) = http(&conn, "POST", "/v1/unlock", &json!({ "passphrase": passphrase }).to_string())
        .map_err(|e| json!({ "code": "unavailable", "message": e }))?;
    if status == 200 { Ok(body) } else { Err(body["error"].clone()) }
}

/// First launch: create the profile (new key in the keychain, empty encrypted database). The supervisor then starts serve.
#[tauri::command(async)]
fn init_profile() -> Result<(), Value> {
    let out = core_command().arg("init").output().map_err(|e| json!({ "code": "init_failed", "message": e.to_string() }))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(json!({ "code": "init_failed", "message": String::from_utf8_lossy(&out.stderr).trim() }))
    }
}

/// Where the core binary lives (for the MCP config an agent needs).
/** Agents with a one-click connect that look installed here. */
#[tauri::command]
fn agent_clients() -> Value {
    Value::Array(clients::Roots::os().map(|r| clients::detect(&r)).unwrap_or_default())
}

/** Writes this app's MCP entry (with the grant's token) into an agent's config; returns the file. */
#[tauri::command]
async fn connect_agent(id: String, token: String) -> Result<String, String> {
    let r = clients::Roots::os().ok_or("no home folder")?;
    clients::connect(&r, &id, &core_bin().to_string_lossy(), &token).map(|p| p.display().to_string())
}

#[tauri::command]
fn core_path() -> String {
    core_bin().display().to_string()
}

/// Re-emits serve's SSE events to the UI until the connection drops.
fn relay_events(app: &AppHandle, c: &Conn) {
    let Ok(mut s) = TcpStream::connect(("127.0.0.1", c.port)) else { return };
    let _ = write!(s, "GET /v1/events HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\n\r\n", c.port, c.token);
    for line in BufReader::new(s).lines() {
        let Ok(line) = line else { return };
        if let Some(ev) = line.strip_prefix("event: ") {
            let _ = app.emit(&format!("health:{ev}"), ());
        }
    }
}

fn status(app: &AppHandle, shell: &Shell, up: bool, extra: Value) {
    let mut v = json!({ "up": up });
    if let (Value::Object(o), Value::Object(e)) = (&mut v, extra) {
        o.extend(e);
    }
    if !up {
        v["error"] = json!(shell.last_error.lock().unwrap().clone());
    }
    let _ = app.emit("health:status", v);
}

/// Starts serve and keeps it running; restarts with backoff (1 s doubling to 30 s) if it exits.
fn supervise(app: AppHandle, shell: Arc<Shell>) {
    let mut backoff = 1;
    while !shell.quitting.load(Ordering::SeqCst) {
        let spawned = core_command()
            .args(["serve", "--exit-with-parent"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let started = std::time::Instant::now();
        let mut child = match spawned {
            Ok(c) => c,
            Err(e) => {
                *shell.last_error.lock().unwrap() = Some(format!("cannot start the health core: {e}"));
                status(&app, &shell, false, json!({}));
                std::thread::sleep(Duration::from_secs(backoff));
                backoff = (backoff * 2).min(30);
                continue;
            }
        };
        *shell.last_error.lock().unwrap() = None; // only this child's errors count
        // the core's last error line, for the UI ("no_profile: …", "running: …")
        if let Some(err) = child.stderr.take() {
            let s = shell.clone();
            std::thread::spawn(move || {
                for l in BufReader::new(err).lines().map_while(Result::ok) {
                    let l = l.trim_start_matches("error: ").to_string();
                    if !l.is_empty() {
                        *s.last_error.lock().unwrap() = Some(l);
                    }
                }
            });
        }
        let mut line = String::new();
        let ready = child.stdout.take().map(|o| BufReader::new(o).read_line(&mut line).is_ok_and(|n| n > 0)).unwrap_or(false);
        let info: Value = serde_json::from_str(&line).unwrap_or(Value::Null);
        if let (true, Some(port), Some(token)) = (ready, info["port"].as_u64(), info["token"].as_str()) {
            let conn = Conn { port: port as u16, token: token.into() };
            // handshake: refuse a core that speaks another API major version (plan §7)
            let meta = http(&conn, "GET", "/v1/meta", "");
            let mismatch = matches!(&meta, Ok((_, m)) if m["api"] != 1);
            if let Ok((_, m)) = &meta
                && m["api"] == 1
            {
                *shell.last_error.lock().unwrap() = None;
                *shell.conn.lock().unwrap() = Some(conn.clone());
                status(&app, &shell, true, json!({ "locked": info["locked"] }));
                let (a, c) = (app.clone(), conn);
                std::thread::spawn(move || relay_events(&a, &c));
            } else {
                *shell.last_error.lock().unwrap() = Some(if mismatch {
                    "the bundled core speaks another API version; reinstall Health".into()
                } else {
                    "cannot reach the health core; retrying".into()
                });
                status(&app, &shell, false, json!({}));
                let _ = child.kill();
            }
        } else {
            // no handshake line: a core that will never answer; restart it rather than wait forever
            let _ = child.kill();
        }
        *shell.child.lock().unwrap() = Some(child);
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let mut g = shell.child.lock().unwrap();
            match g.as_mut().map(|c| c.try_wait()) {
                Some(Ok(None)) => continue,
                _ => {
                    g.take();
                    break;
                }
            }
        }
        let ran = started.elapsed(); // how long this core lived, before any waiting below
        *shell.conn.lock().unwrap() = None;
        std::thread::sleep(Duration::from_millis(200)); // let the stderr reader catch the last line
        let err = shell.last_error.lock().unwrap().clone().unwrap_or_default();
        let need_init = err.starts_with("no_profile");
        status(&app, &shell, false, json!({ "need_init": need_init }));
        // a core that ran a good while gets a fresh start; one that dies at once backs off further each time
        if ran >= Duration::from_secs(30) {
            backoff = 1;
        }
        // a missing profile is fixed by the person, so check back quickly once they create it
        std::thread::sleep(Duration::from_secs(if need_init { 1 } else { backoff }));
        if !need_init && ran < Duration::from_secs(30) {
            backoff = (backoff * 2).min(30);
        }
    }
}

/// An empty unified NSToolbar: the window gets the macOS toolbar-window shape (larger corners, traffic lights
/// centred in a taller title area). The title bar stays transparent over the content (tauri.conf.json).
#[cfg(target_os = "macos")]
fn native_toolbar(w: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSToolbar, NSWindow, NSWindowToolbarStyle};
    let (Ok(ptr), Some(mtm)) = (w.ns_window(), objc2_foundation::MainThreadMarker::new()) else { return };
    // SAFETY: tauri hands out the live NSWindow of this window; setup runs on the main thread (checked above).
    let win: &NSWindow = unsafe { &*(ptr as *const NSWindow) };
    let toolbar = NSToolbar::new(mtm);
    win.setToolbar(Some(&toolbar));
    win.setToolbarStyle(NSWindowToolbarStyle::Unified);
}

fn main() {
    let shell = Arc::new(Shell::default());
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // size and place are remembered; visibility is not (a window hidden by ⌘W must come back on launch)
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(tauri_plugin_window_state::StateFlags::all() & !tauri_plugin_window_state::StateFlags::VISIBLE)
                .build(),
        )
        .manage(shell.clone())
        .invoke_handler(tauri::generate_handler![op, unlock, init_profile, core_path, agent_clients, connect_agent])
        // closing the window keeps Health running so agents can still reach it, where it can be brought back: the
        // dock on macOS, the tray on Windows. Linux desktops may have no tray host, so closing quits there.
        .on_window_event(|w, e| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = e
                && cfg!(any(target_os = "macos", windows))
            {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .on_menu_event(|app, e| {
            // the app menu's own items; the tray menu handles its own
            if let Some(id) = e.id().as_ref().strip_prefix("ui:") {
                let _ = app.emit("menu", id);
            }
        })
        .setup(|app| {
            // macOS: the standard menus plus "Go", whose shortcuts the UI shows. Elsewhere the window has no menu
            // bar and the UI handles the same keys itself.
            #[cfg(target_os = "macos")]
            {
                let item = |id: &str, label: &str, key: &str| MenuItem::with_id(app, format!("ui:{id}"), label, true, Some(key));
                let go = tauri::menu::Submenu::with_items(
                    app,
                    "Go",
                    true,
                    &[
                        &item("readings", "Readings", "CmdOrCtrl+1")?,
                        &item("add", "Add data", "CmdOrCtrl+2")?,
                        &item("sources", "Sources", "CmdOrCtrl+3")?,
                        &item("backup", "Backup", "CmdOrCtrl+4")?,
                        &item("agents", "Agent access", "CmdOrCtrl+5")?,
                        &tauri::menu::PredefinedMenuItem::separator(app)?,
                        &item("back", "Back", "CmdOrCtrl+[")?,
                        &item("find", "Find a measure", "CmdOrCtrl+F")?,
                        &item("import", "Import a file…", "CmdOrCtrl+O")?,
                    ],
                )?;
                if let Some(w) = app.get_webview_window("main") {
                    native_toolbar(&w);
                }
                let menu = Menu::default(app.handle())?;
                let at = menu.items()?.len().saturating_sub(2); // before Window and Help
                menu.insert(&go, at)?;
                app.set_menu(menu)?;
            }
            let show = MenuItem::with_id(app, "show", "Open Health", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            TrayIconBuilder::new()
                .icon(app.default_window_icon().cloned().expect("bundled icon"))
                .tooltip("Health")
                .menu(&menu)
                .on_menu_event(|app, e| match e.id().as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            let (handle, s) = (app.handle().clone(), app.state::<Arc<Shell>>().inner().clone());
            std::thread::spawn(move || supervise(handle, s));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri app");
    app.run(move |app, e| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen { .. } = e
            && let Some(w) = app.get_webview_window("main")
        {
            let _ = w.show();
            let _ = w.set_focus();
        }
        #[cfg(not(target_os = "macos"))]
        let _ = app;
        if let RunEvent::Exit = e {
            shell.quitting.store(true, Ordering::SeqCst);
            if let Some(mut c) = shell.child.lock().unwrap().take() {
                let _ = c.kill();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real serve, a reply far over tiny_http's chunking threshold: the shell must read it whole.
    #[test]
    fn large_replies_come_through_whole() {
        let core = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/health");
        if !core.exists() {
            eprintln!("skipped: build the core first (cargo build -p health)");
            return;
        }
        let dir = std::env::temp_dir().join(format!("health-shell-test-{}", std::process::id()));
        let run = |args: &[&str]| {
            Command::new(&core)
                .args(args)
                .env("HEALTH_DATA_DIR", dir.join("data"))
                .env("HEALTH_ROOT_KEY_FILE", dir.join("k"))
                .output()
                .unwrap()
        };
        assert!(run(&["init"]).status.success());
        let mut serve = Command::new(&core)
            .args(["serve", "--exit-with-parent"])
            .env("HEALTH_DATA_DIR", dir.join("data"))
            .env("HEALTH_ROOT_KEY_FILE", dir.join("k"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(serve.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let info: Value = serde_json::from_str(&line).unwrap();
        let conn = Conn { port: info["port"].as_u64().unwrap() as u16, token: info["token"].as_str().unwrap().into() };
        let (status, body) = http(&conn, "POST", "/v1/op/metric.search", r#"{"q":"a","limit":1000}"#).unwrap();
        assert_eq!(status, 200);
        assert!(body.to_string().len() > 64 * 1024, "the reply must be well past tiny_http's 32 KiB chunking threshold");
        // a second serve on the same profile says why it can't start, which the shell shows
        let second = run(&["serve"]);
        assert!(String::from_utf8_lossy(&second.stderr).contains("running"), "{second:?}");
        drop(serve.stdin.take()); // --exit-with-parent: closing stdin stops the core
        assert!(serve.wait().unwrap().success());
        let _ = std::fs::remove_dir_all(dir);
    }

    /// First launch: with no profile, serve exits saying `no_profile`, which the supervisor turns into need-init.
    #[test]
    fn first_launch_reports_no_profile() {
        let core = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/health");
        if !core.exists() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("health-shell-first-{}", std::process::id()));
        let out = Command::new(&core)
            .arg("serve")
            .env("HEALTH_DATA_DIR", dir.join("data"))
            .env("HEALTH_ROOT_KEY_FILE", dir.join("k"))
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().last().unwrap_or("").trim_start_matches("error: ");
        assert!(!out.status.success() && last.starts_with("no_profile"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
