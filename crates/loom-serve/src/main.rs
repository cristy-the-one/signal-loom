//! Localhost preview of the Signal Loom indexer.
//!
//! The desktop app talks to the same crate through Tauri. This process exists
//! so `npm run dev:preview` can exercise the UI in a browser. It binds to
//! 127.0.0.1 only and is not a network service.

use loom_core::{
    IndexControl, IndexStatus, MathChannel, Query, Session, StepDir, ThresholdTrigger,
};
use serde::Deserialize;
use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
    let hub = Hub {
        session: Arc::new(Mutex::new(Session::new())),
        control: Arc::new(IndexControl::default()),
        phase: Arc::new(Mutex::new(Phase::Idle)),
    };
    for mut request in server.incoming_requests() {
        let response = dispatch(&hub, &mut request);
        let _ = request.respond(response);
    }
}

struct Hub {
    session: Arc<Mutex<Session>>,
    control: Arc<IndexControl>,
    phase: Arc<Mutex<Phase>>,
}

#[derive(Clone)]
enum Phase {
    Idle,
    Running,
    Ready,
    Failed(String),
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

fn dispatch(hub: &Hub, request: &mut Request) -> Response<std::io::Cursor<Vec<u8>>> {
    // The UI reaches this process through the Vite proxy, so every legitimate
    // request is same-origin. Refuse other sites and DNS-rebound hostnames.
    if !header(request, "host").is_some_and(|host| is_loopback_authority(&host))
        || header(request, "origin").is_some_and(|origin| !is_loopback_origin(&origin))
    {
        return error(403, "only the local Signal Loom UI may call this engine");
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
        let mut session = hub.session.lock().map_err(|err| err.to_string())?;
        match (method, path.as_str()) {
            (Method::Get, "/api/health") => Ok(br#"{"ok":true}"#.to_vec()),
            (Method::Get, "/api/summary") => json(&lift(session.summary())?),
            (Method::Post, "/api/open-sample") => json(&lift(session.open_sample())?),
            (Method::Post, "/api/open-path") => {
                let req: PathBody = parse_json(&body)?;
                json(&lift(session.open_path(PathBuf::from(req.path).as_path()))?)
            }
            (Method::Post, "/api/begin-open") => {
                let req: PathBody = parse_json(&body)?;
                drop(session);
                begin(hub, move |session, control| {
                    session
                        .open_path_controlled(PathBuf::from(req.path).as_path(), Some(control))
                        .map(|_| ())
                })
            }
            (Method::Post, "/api/begin-map") => {
                let req: PathBody = parse_json(&body)?;
                drop(session);
                begin(hub, move |session, control| {
                    session
                        .open_map_path_controlled(PathBuf::from(req.path).as_path(), Some(control))
                        .map(|_| ())
                })
            }
            (Method::Post, "/api/begin-add-map") => {
                let req: AddMapBody = parse_json(&body)?;
                drop(session);
                begin(hub, move |session, control| {
                    session
                        .add_map_path_controlled(
                            PathBuf::from(req.path).as_path(),
                            req.channel,
                            Some(control),
                        )
                        .map(|_| ())
                })
            }
            (Method::Get, "/api/progress") | (Method::Post, "/api/progress") => {
                drop(session);
                json(&progress(hub, true)?)
            }
            (Method::Post, "/api/cancel") => {
                hub.control.request_cancel();
                drop(session);
                json(&progress(hub, false)?)
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
            (Method::Post, "/api/add-map") => {
                let req: AddJsonBody = parse_json(&body)?;
                json(&lift(session.add_map_json(&req.json, req.channel))?)
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
            (Method::Post, "/api/bus") => {
                let req: ExportBody = parse_json(&body)?;
                json(&lift(session.bus_load(req.t0_us, req.t1_us))?)
            }
            (Method::Post, "/api/capture") => {
                let req: CaptureBody = parse_json(&body)?;
                json(&lift(
                    session.capture_socketcan(&req.iface, req.duration_ms),
                )?)
            }
            (Method::Post, "/api/stats") => {
                let req: StatsBody = parse_json(&body)?;
                json(&lift(session.stats(&req.name, req.t0_us, req.t1_us))?)
            }
            (Method::Post, "/api/export-csv") => {
                let req: ExportBody = parse_json(&body)?;
                let text = lift(session.export_csv(&req.names, req.t0_us, req.t1_us))?;
                json(&serde_json::json!({ "text": text }))
            }
            (Method::Post, "/api/export-slog") => {
                let req: ExportBody = parse_json(&body)?;
                let text = lift(session.export_slog(req.t0_us, req.t1_us))?;
                json(&serde_json::json!({ "text": text }))
            }
            (Method::Post, "/api/math") => {
                let req: MathBody = parse_json(&body)?;
                json(&lift(session.set_math(req.channels))?)
            }
            (Method::Post, "/api/triggers") => {
                let req: TriggerBody = parse_json(&body)?;
                json(&lift(session.set_triggers(req.triggers))?)
            }
            (Method::Post, "/api/timeout-factor") => {
                let req: TimeoutBody = parse_json(&body)?;
                json(&lift(session.set_timeout_factor(req.factor))?)
            }
            (Method::Post, "/api/compare-path") => {
                let req: PathBody = parse_json(&body)?;
                json(&lift(
                    session.open_compare_path(PathBuf::from(req.path).as_path()),
                )?)
            }
            (Method::Post, "/api/compare-bytes") => {
                json(&lift(session.open_compare_bytes(body.clone()))?)
            }
            (Method::Post, "/api/compare-offset") => {
                let req: OffsetBody = parse_json(&body)?;
                session.set_compare_offset(req.offset_us);
                json(&lift(session.summary())?)
            }
            (Method::Post, "/api/compare-clear") => {
                session.clear_compare();
                json(&lift(session.summary())?)
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
struct AddMapBody {
    path: String,
    #[serde(default)]
    channel: u8,
}

#[derive(Deserialize)]
struct AddJsonBody {
    json: String,
    #[serde(default)]
    channel: u8,
}

fn begin(
    hub: &Hub,
    work: impl FnOnce(&mut Session, &IndexControl) -> loom_core::Result<()> + Send + 'static,
) -> Result<Vec<u8>, String> {
    {
        let mut phase = hub.phase.lock().map_err(|err| err.to_string())?;
        if matches!(*phase, Phase::Running) {
            return Err("an index is already running".into());
        }
        hub.control.reset(0);
        *phase = Phase::Running;
    }
    let session = Arc::clone(&hub.session);
    let control = Arc::clone(&hub.control);
    let phase = Arc::clone(&hub.phase);
    std::thread::spawn(move || {
        let result = {
            let mut session = match session.lock() {
                Ok(guard) => guard,
                Err(err) => {
                    if let Ok(mut phase) = phase.lock() {
                        *phase = Phase::Failed(err.to_string());
                    }
                    return;
                }
            };
            work(&mut session, &control)
        };
        if let Ok(mut phase) = phase.lock() {
            *phase = match result {
                Ok(()) => Phase::Ready,
                Err(err) => Phase::Failed(err.to_string()),
            };
        }
    });
    json(&serde_json::json!({ "started": true }))
}

/// `take` hands a finished result to the caller and resets to idle. Cancel
/// only peeks, so a result that lands as Cancel is clicked still reaches the poll.
fn progress(hub: &Hub, take: bool) -> Result<IndexStatus, String> {
    let mut phase = hub.phase.lock().map_err(|err| err.to_string())?;
    let (bytes_done, bytes_total, frames, skipped) = hub.control.snapshot();
    let mut status = IndexStatus {
        running: false,
        done: false,
        idle: false,
        bytes_done,
        bytes_total,
        frames,
        skipped,
        summary: None,
        error: None,
    };
    let current = if take {
        std::mem::replace(&mut *phase, Phase::Idle)
    } else {
        phase.clone()
    };
    match current {
        Phase::Idle => status.idle = true,
        Phase::Running => {
            *phase = Phase::Running;
            status.running = true;
        }
        Phase::Ready => match hub.session.lock() {
            Ok(session) => match session.summary() {
                Ok(summary) => {
                    status.done = true;
                    status.bytes_done = summary.bytes;
                    status.frames = summary.frame_count;
                    status.skipped = summary.skipped_records;
                    status.summary = Some(summary);
                }
                Err(err) => {
                    status.done = true;
                    status.error = Some(err.to_string());
                }
            },
            Err(err) => {
                *phase = Phase::Ready;
                return Err(err.to_string());
            }
        },
        Phase::Failed(message) => {
            status.done = true;
            status.error = Some(message);
        }
    }
    Ok(status)
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
#[serde(rename_all = "camelCase")]
struct StatsBody {
    name: String,
    #[serde(deserialize_with = "loom_core::deserialize_us")]
    t0_us: u64,
    #[serde(deserialize_with = "loom_core::deserialize_us")]
    t1_us: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExportBody {
    #[serde(default)]
    names: Vec<String>,
    #[serde(deserialize_with = "loom_core::deserialize_us")]
    t0_us: u64,
    #[serde(deserialize_with = "loom_core::deserialize_us")]
    t1_us: u64,
}

#[derive(Deserialize)]
struct MathBody {
    channels: Vec<MathChannel>,
}

#[derive(Deserialize)]
struct TriggerBody {
    triggers: Vec<ThresholdTrigger>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CaptureBody {
    iface: String,
    duration_ms: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OffsetBody {
    offset_us: i64,
}

#[derive(Deserialize)]
struct TimeoutBody {
    factor: f64,
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

fn is_loopback_authority(authority: &str) -> bool {
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => authority,
    };
    matches!(
        host.to_ascii_lowercase().as_str(),
        "127.0.0.1" | "localhost" | "[::1]"
    )
}

fn is_loopback_origin(origin: &str) -> bool {
    origin
        .strip_prefix("http://")
        .is_some_and(is_loopback_authority)
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
    response
}

fn header_line(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("header")
}
