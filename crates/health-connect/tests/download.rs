//! The download path against a local HTTP server: the pinned file arrives, anything else is discarded.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;

/// Serves `body` (with `status`) to every request; returns the base URL.
fn serve(status: &'static str, body: Vec<u8>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 2 {
                line.clear();
            }
            let mut s = s;
            let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    format!("http://127.0.0.1:{port}/file")
}

fn sha(b: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(b))
}

#[test]
fn the_pinned_file_arrives_and_anything_else_is_discarded() {
    let dir = tempfile::tempdir().unwrap();
    let body = b"model bytes".to_vec();
    let dest = dir.path().join("m.onnx");
    let url = serve("200 OK", body.clone());
    let mut seen = 0;
    health_connect::download(&url, &dest, &sha(&body), body.len() as u64, &mut |n, _| seen = n).unwrap();
    assert_eq!((std::fs::read(&dest).unwrap(), seen), (body.clone(), body.len() as u64));

    // a different file of the same size: refused, nothing left behind
    let bad = dir.path().join("bad.onnx");
    let e = health_connect::download(&serve("200 OK", b"model bytez".to_vec()), &bad, &sha(&body), body.len() as u64, &mut |_, _| {})
        .unwrap_err();
    assert!(matches!(e, health_connect::Error::Integrity(_)), "{e}");
    assert!(!bad.exists() && !bad.with_extension("part").exists());
    // larger than pinned: refused
    let e = health_connect::download(&serve("200 OK", vec![1; 64]), &bad, &sha(&body), body.len() as u64, &mut |_, _| {}).unwrap_err();
    assert!(matches!(e, health_connect::Error::Integrity(_) | health_connect::Error::Network(_)), "{e}");
    assert!(!bad.exists());
    // not found: a network error
    let e = health_connect::download(&serve("404 Not Found", vec![]), &bad, &sha(&body), body.len() as u64, &mut |_, _| {}).unwrap_err();
    assert!(matches!(e, health_connect::Error::Network(_)), "{e}");
}

/// Serves one fixed reply (status, body) and records each request line and body.
fn serve_json(status: &'static str, body: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut len = 0;
            let mut line = String::new();
            while r.read_line(&mut line).unwrap_or(0) > 2 {
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                line.clear();
            }
            let mut b = vec![0; len];
            std::io::Read::read_exact(&mut r, &mut b).unwrap();
            log.lock().unwrap().push(format!("{}{}", first.trim(), String::from_utf8(b).unwrap()));
            let mut s = s;
            let _ = write!(
                s,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nLocation: http://127.0.0.1:1/\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

#[test]
fn the_backend_answers_with_its_object_or_a_code() {
    let (base, seen) = serve_json("200 OK", r#"{"code":"160010","until":1}"#);
    let b = health_connect::Backend::new(&base);
    assert_eq!(b.post("/h/1/self", r#"{"jws":"x"}"#).unwrap()["code"], "160010");
    assert_eq!(seen.lock().unwrap()[0], r#"POST /h/1/self HTTP/1.1{"jws":"x"}"#);
    let (base, _) = serve_json("404 Not Found", r#"{"err":"unknown"}"#);
    assert_eq!(health_connect::Backend::new(&base).get("/h/1/link/X").unwrap_err(), "unknown");
    let (base, _) = serve_json("500 Internal Server Error", r#"{"err":"made-up"}"#);
    assert_eq!(health_connect::Backend::new(&base).get("/").unwrap_err(), "server");
    let (base, _) = serve_json("200 OK", "<html>");
    assert_eq!(health_connect::Backend::new(&base).get("/").unwrap_err(), "server");
    // a redirect is never followed
    let (base, _) = serve_json("302 Found", "{}");
    assert_eq!(health_connect::Backend::new(&base).get("/").unwrap_err(), "server");
    assert_eq!(health_connect::Backend::new("http://127.0.0.1:1").get("/").unwrap_err(), "network");
}
