//! Localhost preview of the Signal Loom indexer.
//!
//! The desktop app talks to the same crate through Tauri. This process exists
//! so `npm run dev:preview` can exercise the UI in a browser. It binds to
//! 127.0.0.1 only and is not a network service.

use loom_core::{ProjectFile, Query, Session, StepDir};
use serde::Deserialize;
use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

const MAX_BODY: usize = 32 * 1024 * 1024;

fn main() {
    let port = parse_port();
    let addr = format!("127.0.0.1:{port}");
    let server = Server::http(&addr).unwrap_or_else(|err| {
        eprintln!("loom-serve could not bind {addr}: {err}");
        std::process::exit(1);
    });
    eprintln!("signal-loom engine listening on http://{addr}");
    let session = Mutex::new(Session::new());
    for mut request in server.incoming_requests() {
        let response = dispatch(&session, &mut request);
        let _ = request.respond(response);
    }
}

fn parse_port() -> u16 {
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--port" {
            let Some(value) = args.next() else {
                eprintln!("--port needs a number");
                std::process::exit(2);
            };
            return value.parse().unwrap_or_else(|_| {
                eprintln!("bad port {value}");
                std::process::exit(2);
            });
        }
    }
    43128
}

fn dispatch(session: &Mutex<Session>, request: &mut Request) -> Response<std::io::Cursor<Vec<u8>>> {
    if request.method() == &Method::Options {
        return respond(204, b"{}".to_vec());
    }
    let path = request.url().split('?').next().unwrap_or("/").to_string();
    let method = request.method().clone();
    let filename = header(request, "x-filename");
    let body = match read_body(request) {
        Ok(body) => body,
        Err(err) => return error(400, &err),
    };
    if body.len() > MAX_BODY {
        return error(413, "upload is larger than 32MB");
    }

    let result = (|| -> Result<Vec<u8>, String> {
        let mut session = session.lock().map_err(|err| err.to_string())?;
        match (method, path.as_str()) {
            (Method::Get, "/api/health") => Ok(br#"{"ok":true}"#.to_vec()),
            (Method::Get, "/api/summary") => json(&lift(session.summary())?),
            (Method::Post, "/api/open-sample") => json(&lift(session.open_sample())?),
            (Method::Post, "/api/open-path") => {
                let req: PathBody = parse_json(&body)?;
                json(&lift(session.open_path(PathBuf::from(req.path).as_path()))?)
            }
            (Method::Post, "/api/open-bytes") => {
                let name = filename.unwrap_or_else(|| "upload.log".into());
                json(&lift(session.open_bytes(&name, body.clone()))?)
            }
            (Method::Post, "/api/open-map-path") => {
                let req: PathBody = parse_json(&body)?;
                json(&lift(
                    session.open_map_path(PathBuf::from(req.path).as_path()),
                )?)
            }
            (Method::Post, "/api/open-map") => {
                let json_text = json_text(&body)?;
                json(&lift(session.open_map_json(&json_text))?)
            }
            (Method::Post, "/api/query") => {
                let query: Query = parse_json(&body)?;
                json(&lift(session.query(&query))?)
            }
            (Method::Post, "/api/values") => {
                let req: TimeBody = parse_json(&body)?;
                json(&lift(session.values_at(req.t_us))?)
            }
            (Method::Post, "/api/step") => {
                let req: StepBody = parse_json(&body)?;
                json(&lift(session.step(req.t_us, req.direction))?)
            }
            (Method::Post, "/api/frame") => {
                let req: TimeBody = parse_json(&body)?;
                json(&lift(session.frame_at(req.t_us))?)
            }
            (Method::Post, "/api/open-project-path") => {
                let req: PathBody = parse_json(&body)?;
                json(&lift(
                    session.load_project_file(PathBuf::from(req.path).as_path()),
                )?)
            }
            (Method::Post, "/api/open-project") => {
                let req: ProjectBody = parse_json(&body)?;
                let base = req.base_dir.map(PathBuf::from);
                json(&lift(
                    session.load_project_json(&req.json, base.as_deref()),
                )?)
            }
            (Method::Post, "/api/write-project") => {
                let req: WriteProjectBody = parse_json(&body)?;
                let project = lift(ProjectFile::parse(&req.json))?;
                lift(session.write_project(PathBuf::from(req.path).as_path(), &project))?;
                Ok(br#"{"ok":true}"#.to_vec())
            }
            _ => Err(format!("no route for {path}")),
        }
    })();

    match result {
        Ok(bytes) => respond(200, bytes),
        Err(message) => {
            if message.starts_with("no route") {
                error(404, &message)
            } else {
                error(400, &message)
            }
        }
    }
}

#[derive(Deserialize)]
struct PathBody {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimeBody {
    #[serde(deserialize_with = "loom_core::deserialize_us")]
    t_us: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StepBody {
    #[serde(deserialize_with = "loom_core::deserialize_us")]
    t_us: u64,
    direction: StepDir,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProjectBody {
    json: String,
    base_dir: Option<String>,
}

#[derive(Deserialize)]
struct WriteProjectBody {
    path: String,
    json: String,
}

fn json_text(body: &[u8]) -> Result<String, String> {
    if body.first() == Some(&b'{') {
        if let Ok(wrapped) = serde_json::from_slice::<serde_json::Value>(body) {
            if let Some(text) = wrapped.get("json").and_then(|v| v.as_str()) {
                return Ok(text.to_string());
            }
        }
    }
    String::from_utf8(body.to_vec()).map_err(|_| "map is not utf-8".into())
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<T, String> {
    serde_json::from_slice(body).map_err(|err| err.to_string())
}

fn lift<T>(result: loom_core::Result<T>) -> Result<T, String> {
    result.map_err(|err| err.to_string())
}

fn json<T: serde::Serialize>(value: &T) -> Result<Vec<u8>, String> {
    serde_json::to_vec(value).map_err(|err| err.to_string())
}

fn read_body(request: &mut Request) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    request
        .as_reader()
        .take(MAX_BODY as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|err| err.to_string())?;
    Ok(buf)
}

fn header(request: &Request, name: &str) -> Option<String> {
    request.headers().iter().find_map(|header| {
        let field = header.field.as_str().as_str();
        if field.eq_ignore_ascii_case(name) {
            Some(header.value.as_str().to_string())
        } else {
            None
        }
    })
}

fn error(status: u16, message: &str) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::json!({ "error": message })
        .to_string()
        .into_bytes();
    respond(status, body)
}

fn respond(status: u16, body: Vec<u8>) -> Response<std::io::Cursor<Vec<u8>>> {
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    response.add_header(header_line("Content-Type", "application/json"));
    response.add_header(header_line("Access-Control-Allow-Origin", "*"));
    response.add_header(header_line("Access-Control-Allow-Headers", "*"));
    response.add_header(header_line(
        "Access-Control-Allow-Methods",
        "GET, POST, OPTIONS",
    ));
    response
}

fn header_line(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("header")
}
