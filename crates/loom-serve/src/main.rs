//! Localhost preview of the Signal Loom indexer.
//!
//! The desktop app talks to the same crate through Tauri. This process exists
//! so `npm run dev:preview` can exercise the UI in a browser. It binds to
//! 127.0.0.1 only and is not a network service.

use loom_core::{Engine, Export, MathChannel, ProjectView, Query, StepDir, ThresholdTrigger};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::env;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
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
    let engine = Arc::new(Engine::new());
    for mut request in server.incoming_requests() {
        let response = dispatch(&engine, &mut request);
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

fn dispatch(engine: &Arc<Engine>, request: &mut Request) -> Response<std::io::Cursor<Vec<u8>>> {
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

    match route(engine, method, &path, filename, body) {
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

/// Answer one request. `body` is moved into the session when it is an upload.
fn route(
    engine: &Arc<Engine>,
    method: Method,
    path: &str,
    filename: Option<String>,
    body: Vec<u8>,
) -> Result<Vec<u8>, String> {
    // Job endpoints must not wait for the session lock a running job holds.
    match (&method, path) {
        (Method::Get, "/api/health") => return Ok(br#"{"ok":true}"#.to_vec()),
        (Method::Post, "/api/begin-open") => {
            let req: PathBody = parse_json(&body)?;
            return begin(engine, move |session, control| {
                session
                    .open_path_controlled(PathBuf::from(req.path).as_path(), Some(control))
                    .map(|_| ())
            });
        }
        (Method::Post, "/api/begin-map") => {
            let req: PathBody = parse_json(&body)?;
            return begin(engine, move |session, control| {
                session
                    .open_map_path_controlled(PathBuf::from(req.path).as_path(), Some(control))
                    .map(|_| ())
            });
        }
        (Method::Post, "/api/begin-add-map") => {
            let req: AddMapBody = parse_json(&body)?;
            return begin(engine, move |session, control| {
                session
                    .add_map_path_controlled(
                        PathBuf::from(req.path).as_path(),
                        req.channel,
                        Some(control),
                    )
                    .map(|_| ())
            });
        }
        (Method::Post, "/api/begin-compare") => {
            let req: PathBody = parse_json(&body)?;
            return begin(engine, move |session, control| {
                session
                    .open_compare_path_controlled(PathBuf::from(req.path).as_path(), Some(control))
                    .map(|_| ())
            });
        }
        (Method::Post, "/api/begin-project") => {
            let req: PathBody = parse_json(&body)?;
            return started(engine.begin_project_file(PathBuf::from(req.path)));
        }
        (Method::Post, "/api/begin-project-json") => {
            let req: ProjectBody = parse_json(&body)?;
            return started(engine.begin_project_json(req.json, req.base_dir.map(PathBuf::from)));
        }
        (Method::Post, "/api/begin-capture") => {
            let req: CaptureBody = parse_json(&body)?;
            return started(engine.begin_capture(req.iface, req.duration_ms));
        }
        (Method::Get | Method::Post, "/api/progress") => {
            return json(&lift(engine.progress(true))?);
        }
        (Method::Post, "/api/cancel") => return json(&lift(engine.cancel())?),
        _ => {}
    }
    match (method, path) {
        (Method::Post, "/api/open-sample") => {
            json(&lift(engine.with_session(|session| session.open_sample()))?)
        }
        (Method::Post, "/api/open-bytes") => {
            let name = filename.unwrap_or_else(|| "upload.log".into());
            json(&lift(
                engine.with_session(|session| session.open_bytes(&name, body)),
            )?)
        }
        (Method::Post, "/api/open-map") => {
            let json_text = json_text(&body)?;
            json(&lift(
                engine.with_session(|session| session.open_map_json(&json_text)),
            )?)
        }
        (Method::Post, "/api/add-map") => {
            let req: AddJsonBody = parse_json(&body)?;
            json(&lift(engine.with_session(|session| {
                session.add_map_json(&req.json, req.channel)
            }))?)
        }
        (Method::Post, "/api/query") => {
            let query: Query = parse_json(&body)?;
            json(&lift(engine.with_session(|session| session.query(&query)))?)
        }
        (Method::Post, "/api/values") => {
            let req: TimeBody = parse_json(&body)?;
            json(&lift(
                engine.with_session(|session| session.values_at(req.t_us)),
            )?)
        }
        (Method::Post, "/api/step") => {
            let req: StepBody = parse_json(&body)?;
            json(&lift(engine.with_session(|session| {
                session.step(req.t_us, req.direction)
            }))?)
        }
        (Method::Post, "/api/frame") => {
            let req: TimeBody = parse_json(&body)?;
            json(&lift(
                engine.with_session(|session| session.frame_at(req.t_us)),
            )?)
        }
        (Method::Post, "/api/project-json") => {
            let view: ProjectView = parse_json(&body)?;
            let text = lift(engine.with_session(|session| session.project_json(&view)))?;
            json(&ProjectJsonReply { json: &text })
        }
        (Method::Post, "/api/bus") => {
            let req: ExportBody = parse_json(&body)?;
            json(&lift(engine.with_session(|session| {
                session.bus_load(req.t0_us, req.t1_us)
            }))?)
        }
        (Method::Post, "/api/stats") => {
            let req: StatsBody = parse_json(&body)?;
            json(&lift(engine.with_session(|session| {
                session.stats(&req.name, req.t0_us, req.t1_us)
            }))?)
        }
        (Method::Post, "/api/export-csv") => {
            let req: ExportBody = parse_json(&body)?;
            let export = lift(engine.with_session(|session| {
                session.export_csv_report(&req.names, req.t0_us, req.t1_us)
            }))?;
            export_json(&export)
        }
        (Method::Post, "/api/export-slog") => {
            let req: ExportBody = parse_json(&body)?;
            let export = lift(
                engine.with_session(|session| session.export_slog_report(req.t0_us, req.t1_us)),
            )?;
            export_json(&export)
        }
        (Method::Post, "/api/math") => {
            let req: MathBody = parse_json(&body)?;
            json(&lift(
                engine.with_session(|session| session.set_math(req.channels)),
            )?)
        }
        (Method::Post, "/api/triggers") => {
            let req: TriggerBody = parse_json(&body)?;
            json(&lift(
                engine.with_session(|session| session.set_triggers(req.triggers)),
            )?)
        }
        (Method::Post, "/api/timeout-factor") => {
            let req: TimeoutBody = parse_json(&body)?;
            json(&lift(engine.with_session(|session| {
                session.set_timeout_factor(req.factor)
            }))?)
        }
        (Method::Post, "/api/compare-bytes") => json(&lift(
            engine.with_session(|session| session.open_compare_bytes(body)),
        )?),
        (Method::Post, "/api/compare-offset") => {
            let req: OffsetBody = parse_json(&body)?;
            json(&lift(engine.with_session(|session| {
                session.set_compare_offset(req.offset_us);
                session.summary()
            }))?)
        }
        (Method::Post, "/api/compare-clear") => json(&lift(engine.with_session(|session| {
            session.clear_compare();
            session.summary()
        }))?),
        _ => Err(format!("no route for {path}")),
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
    engine: &Arc<Engine>,
    work: impl FnOnce(&mut loom_core::Session, &loom_core::IndexControl) -> loom_core::Result<()>
        + Send
        + 'static,
) -> Result<Vec<u8>, String> {
    started(engine.begin(work))
}

fn started(result: loom_core::Result<()>) -> Result<Vec<u8>, String> {
    lift(result)?;
    Ok(br#"{"started":true}"#.to_vec())
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

/// A map upload is the DBC or map text itself, or `{"json": "<text>"}` around it.
fn json_text(body: &[u8]) -> Result<Cow<'_, str>, String> {
    if body.first() == Some(&b'{') {
        if let Ok(wrapped) = serde_json::from_slice::<WrappedText>(body) {
            return Ok(wrapped.json);
        }
    }
    std::str::from_utf8(body)
        .map(Cow::Borrowed)
        .map_err(|_| "map is not utf-8".into())
}

#[derive(Deserialize)]
struct WrappedText<'a> {
    #[serde(borrow)]
    json: Cow<'a, str>,
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

/// An export's text, its data row count, and whether the row cap cut it short.
#[derive(Serialize)]
struct ExportReply<'a> {
    text: &'a str,
    rows: usize,
    truncated: bool,
}

#[derive(Serialize)]
struct ProjectJsonReply<'a> {
    json: &'a str,
}

fn export_json(export: &Export) -> Result<Vec<u8>, String> {
    json(&ExportReply {
        text: &export.text,
        rows: export.rows,
        truncated: export.truncated,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_export_cut_at_the_row_cap_reports_its_rows_and_the_cut() {
        let export = Export {
            text: "t_us,A\n0,1.000000\n".into(),
            rows: 500_000,
            truncated: true,
        };
        let body = export_json(&export).unwrap();
        let reply: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            reply,
            serde_json::json!({ "text": "t_us,A\n0,1.000000\n", "rows": 500000, "truncated": true })
        );
    }

    const LOG: &str = "SLOGv1
F 0 1A0 800C881378640000
F 10000 1A0 800C881378640000
";

    fn call(
        engine: &Arc<Engine>,
        path: &str,
        filename: Option<&str>,
        body: Vec<u8>,
    ) -> serde_json::Value {
        let bytes = route(engine, Method::Post, path, filename.map(String::from), body).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn an_uploaded_log_opens_and_a_second_upload_becomes_the_compare_log() {
        let engine = Arc::new(Engine::new());
        let reply = call(
            &engine,
            "/api/open-bytes",
            Some("drive.slog"),
            LOG.as_bytes().to_vec(),
        );
        assert_eq!(reply["logLabel"], "drive.slog");
        assert_eq!(reply["frameCount"], 2);

        let reply = call(&engine, "/api/compare-bytes", None, LOG.as_bytes().to_vec());
        assert_eq!(reply["frameCount"], 2);
        assert_eq!(reply["logLabel"], "drive.slog");
    }

    #[test]
    fn a_map_upload_is_raw_text_or_wrapped_in_json() {
        assert_eq!(json_text(br#"{"json":"a \"b\""}"#).unwrap(), "a \"b\"");
        assert_eq!(
            json_text(br#"{"name":"bare map","json":7}"#).unwrap(),
            r#"{"name":"bare map","json":7}"#
        );
        assert_eq!(json_text(b"VERSION \"\"").unwrap(), "VERSION \"\"");
        assert!(json_text(&[0xFF, 0xFE]).is_err());
    }

    #[test]
    fn a_project_opens_through_the_job_routes_and_reports_its_summary() {
        let engine = Arc::new(Engine::new());
        let project = serde_json::json!({
            "format": "signal-loom",
            "version": 1,
            "logPath": std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/cluster_drive.slog"),
            "view": { "playheadUs": 0, "spanUs": 1_000_000, "plotted": [] },
        });
        let body = serde_json::json!({ "json": project.to_string() }).to_string();
        let reply = call(&engine, "/api/begin-project-json", None, body.into_bytes());
        assert_eq!(reply, serde_json::json!({ "started": true }));

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let status = loop {
            let status = call(&engine, "/api/progress", None, Vec::new());
            if status["done"] == true {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "job never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(status["error"], serde_json::Value::Null);
        assert_eq!(status["summary"]["logLabel"], "cluster_drive.slog");
        assert_eq!(status["project"]["warnings"], serde_json::json!([]));
    }
}
