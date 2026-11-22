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

/// Runs the core with `args` and `--json`: its reply, or its {code, message} error.
fn core_json(args: &[&str]) -> Result<Value, Value> {
    let out = core_command()
        .arg("--json")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| json!({ "code": "unavailable", "message": e.to_string() }))?;
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    if out.status.success() {
        Ok(v)
    } else {
        Err(v.get("error").cloned().unwrap_or_else(|| json!({ "code": "error", "message": "the core failed" })))
    }
}

/** Is the OCR component (models) installed? */
#[tauri::command(async)]
fn ocr_status() -> Result<Value, Value> {
    core_json(&["component", "list"])
}

/** Downloads and checks the OCR component (the core does it; only health-connect opens connections). */
#[tauri::command(async)]
fn ocr_install(matching: bool) -> Result<Value, Value> {
    let mut args = vec!["component", "install", "ocr"];
    if !matching {
        args.push("--no-matching");
    }
    core_json(&args)
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

/// The menu bar's words, per interface language (the UI's own text is in gui/src/locales).
fn words(lang: &str) -> impl Fn(&str) -> String + '_ {
    move |en: &str| {
        let (hans, hant) = match en {
            "Health" => ("健康", "健康"),
            "About Health" => ("关于健康", "關於健康"),
            "Services" => ("服务", "服務"),
            "Hide Health" => ("隐藏健康", "隱藏健康"),
            "Hide Others" => ("隐藏其他", "隱藏其他"),
            "Show All" => ("全部显示", "全部顯示"),
            "Quit Health" => ("退出健康", "結束健康"),
            "Edit" => ("编辑", "編輯"),
            "Undo" => ("撤销", "還原"),
            "Redo" => ("重做", "重做"),
            "Cut" => ("剪切", "剪下"),
            "Copy" => ("拷贝", "拷貝"),
            "Paste" => ("粘贴", "貼上"),
            "Select All" => ("全选", "全選"),
            "View" => ("显示", "顯示方式"),
            "Enter Full Screen" => ("进入全屏幕", "進入全螢幕"),
            "Go" => ("前往", "前往"),
            "Readings" => ("读数", "讀數"),
            "Data sources" => ("数据来源", "數據來源"),
            "Profile" => ("个人资料", "個人資料"),
            "Backup" => ("备份", "備份"),
            "Agent access" => ("Agent 访问", "Agent 訪問"),
            "Health account" => ("健康账户", "健康賬戶"),
            "Back" => ("返回", "返回"),
            "Find a measure" => ("查找指标", "查找指標"),
            "Import a file…" => ("导入文件…", "導入文件…"),
            "Window" => ("窗口", "視窗"),
            "Minimize" => ("最小化", "縮到最小"),
            "Zoom" => ("缩放", "縮放"),
            "Close Window" => ("关闭窗口", "關閉視窗"),
            _ => (en, en),
        };
        match lang {
            "zh-Hans" => hans.to_string(),
            "zh-Hant" => hant.to_string(),
            _ => en.to_string(),
        }
    }
}

/// The macOS menu bar in `lang`: the standard menus, plus "Go" with the sections' shortcuts.
#[cfg(target_os = "macos")]
fn menu(app: &AppHandle, lang: &str) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem as P, Submenu};
    let w = words(lang);
    let item = |id: &str, label: &str, key: &str| MenuItem::with_id(app, format!("ui:{id}"), w(label), true, Some(key));
    let health = Submenu::with_items(
        app,
        w("Health"),
        true,
        &[
            &P::about(app, Some(&w("About Health")), Some(AboutMetadata::default()))?,
            &P::separator(app)?,
            &P::services(app, Some(&w("Services")))?,
            &P::separator(app)?,
            &P::hide(app, Some(&w("Hide Health")))?,
            &P::hide_others(app, Some(&w("Hide Others")))?,
            &P::show_all(app, Some(&w("Show All")))?,
            &P::separator(app)?,
            &P::quit(app, Some(&w("Quit Health")))?,
        ],
    )?;
    let edit = Submenu::with_items(
        app,
        w("Edit"),
        true,
        &[
            &P::undo(app, Some(&w("Undo")))?,
            &P::redo(app, Some(&w("Redo")))?,
            &P::separator(app)?,
            &P::cut(app, Some(&w("Cut")))?,
            &P::copy(app, Some(&w("Copy")))?,
            &P::paste(app, Some(&w("Paste")))?,
            &P::select_all(app, Some(&w("Select All")))?,
        ],
    )?;
    let view = Submenu::with_items(app, w("View"), true, &[&P::fullscreen(app, Some(&w("Enter Full Screen")))?])?;
    let go = Submenu::with_items(
        app,
        w("Go"),
        true,
        &[
            &item("readings", "Readings", "CmdOrCtrl+1")?,
            &item("data", "Data sources", "CmdOrCtrl+2")?,
            &item("backup", "Backup", "CmdOrCtrl+3")?,
            &item("profile", "Profile", "CmdOrCtrl+4")?,
            &item("account", "Health account", "CmdOrCtrl+5")?,
            &item("agents", "Agent access", "CmdOrCtrl+6")?,
            &P::separator(app)?,
            &item("back", "Back", "CmdOrCtrl+[")?,
            &item("find", "Find a measure", "CmdOrCtrl+F")?,
            &item("import", "Import a file…", "CmdOrCtrl+O")?,
        ],
    )?;
    let window = Submenu::with_items(
        app,
        w("Window"),
        true,
        &[
            &P::minimize(app, Some(&w("Minimize")))?,
            &P::maximize(app, Some(&w("Zoom")))?,
            &P::separator(app)?,
            &P::close_window(app, Some(&w("Close Window")))?,
        ],
    )?;
    Menu::with_items(app, &[&health, &edit, &view, &go, &window])
}

/// Web pages the UI links to; nothing else is opened.
const LINKS: &[&str] = &["https://zuchiangmai.com/health", "https://developer-dashboard.whoop.com/"];

/// Opens one of `LINKS` in the default browser.
#[tauri::command]
fn open_link(url: String) -> Result<(), String> {
    if !LINKS.contains(&url.as_str()) {
        return Err("not an allowed link".into());
    }
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(&url).status();
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", &url]).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(&url).status();
    r.map(|_| ()).map_err(|e| e.to_string())
}

/// The UI's language changed (or the page loaded): the menu bar and the window title follow it.
#[tauri::command]
fn set_language(app: AppHandle, lang: String) -> Result<(), String> {
    let lang = match lang.as_str() {
        "zh-Hans" | "zh-Hant" => lang,
        _ => "en".to_string(),
    };
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_title(&words(&lang)("Health"));
    }
    #[cfg(target_os = "macos")]
    app.set_menu(menu(&app, &lang).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(())
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
        .invoke_handler(tauri::generate_handler![
            op,
            unlock,
            init_profile,
            core_path,
            agent_clients,
            connect_agent,
            ocr_status,
            ocr_install,
            set_language,
            open_link
        ])
        // Health is an app you open and close, not a background service: no tray icon, and closing the window
        // quits (Tauri's default). Agents don't need it running: `health mcp` opens the store itself.
        .on_menu_event(|app, e| {
            if let Some(id) = e.id().as_ref().strip_prefix("ui:") {
                let _ = app.emit("menu", id);
            }
        })
        .setup(|app| {
            // macOS: the standard menus plus "Go", whose shortcuts the UI shows (in the UI's language, which
            // set_language sends once the page has loaded). Elsewhere the window has no menu bar and the UI
            // handles the same keys itself.
            #[cfg(target_os = "macos")]
            {
                if let Some(w) = app.get_webview_window("main") {
                    native_toolbar(&w);
                }
                app.set_menu(menu(app.handle(), "en")?)?;
            }
            let (handle, s) = (app.handle().clone(), app.state::<Arc<Shell>>().inner().clone());
            std::thread::spawn(move || supervise(handle, s));
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri app");
    app.run(move |_, e| {
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
