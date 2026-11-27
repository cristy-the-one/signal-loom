//! `loom-judge`: judge specifications and inspect CAN / telemetry recordings.
//!
//! Modes:
//! - Judge:   `loom-judge --log FILE --dbc FILE[@CH] --request FILE|- --out FILE|-`
//! - Check:   `loom-judge --check --dbc FILE[@CH] --request FILE|-`
//! - Inspect: `loom-judge --inspect --log FILE [--dbc FILE[@CH]] [--no-hash]`
//! - Stats:   `loom-judge --stats [SIG,...] --log FILE [--dbc FILE[@CH]] [--from-us T0] [--to-us T1]`
//! - Samples: `loom-judge --samples SIG[,...] --log FILE [--dbc FILE[@CH]] [--from-us T0] [--to-us T1] [--max-points N]`
//! - Dump:    `loom-judge --dump-dbc FILE`
//!
//! Exit 0 after a verdict or inspection is written. Exit 2 when a request is
//! refused, writing no verdict file. Exit 1 when a file could not be read or
//! written, or the command line could not be used.

use std::io::Read;
use std::path::Path;

use loom_core::inspect::{inspect_log, parse_map_or_dbc, samples_for_log, stats_for_log};
use loom_core::judge::{self, Evaluation, VerdictDbc, VerdictLog};
use loom_core::SignalMap;
use sha2::{Digest, Sha256};

const VERSION: &str = concat!("loom-judge ", env!("CARGO_PKG_VERSION"));

const USAGE: &str = "\
usage:
  loom-judge --log FILE --dbc FILE [--dbc FILE@CHANNEL] --request FILE|- --out FILE|-
  loom-judge --check --dbc FILE [--dbc FILE@CHANNEL] --request FILE|-
  loom-judge --inspect --log FILE [--dbc FILE@CHANNEL] [--no-hash]
  loom-judge --stats [SIG[,SIG...]] --log FILE [--dbc FILE@CHANNEL] [--from-us T0] [--to-us T1]
  loom-judge --samples SIG[,SIG...] --log FILE [--dbc FILE@CHANNEL] [--from-us T0] [--to-us T1] [--max-points N]
  loom-judge --dump-dbc FILE
  loom-judge --help | --version

options:
  --log FILE            Recording (.blf, .asc, .slog, .slb, .csv, candump)
  --dbc FILE[@CHANNEL]  Vector .dbc or Signal Loom .map.json (repeatable; channel 0..255)
  --request FILE|-      loom-judge/1 JSON request file, or '-' for stdin
  --out FILE|-          Output loom-verdict/1 file, or '-' for stdout (summary goes to stderr)
  --check               Validate --request against --dbc without reading a recording
  --inspect             Write deterministic loom-inspect/1 JSON summary of --log to stdout
  --no-hash             Skip SHA-256 computation in --inspect for instant triage on huge files
  --stats [SIG,...]     Write loom-stats/1 JSON window statistics (all active signals if omitted)
  --samples SIG,...     Write loom-samples/1 JSON series (raw, or decimated with --max-points)
  --from-us T0          Window start in microseconds (defaults to recording start)
  --to-us T1            Window end in microseconds (defaults to recording end)
  --max-points N        Decimate --samples to at most N points per signal
  --dump-dbc FILE       Write loom-dbc-dump/1 JSON for a .dbc or .map.json to stdout

exit codes:
  0  Completed (verdict written, request valid, or inspection/dump printed)
  1  Command-line usage or file read/write error
  2  Judge request refused (lists every problem on stderr)";

fn main() {
    match run() {
        Ok(Output::StdoutLine(line)) => println!("{line}"),
        Ok(Output::StdoutRaw(text)) => print!("{text}"),
        Ok(Output::VerdictStdout { verdict, summary }) => {
            print!("{verdict}");
            eprintln!("{summary}");
        }
        Err(err) => {
            match &err {
                Fail::Usage(message) | Fail::File(message) => eprintln!("{message}"),
                Fail::Refused(problems) => {
                    eprintln!("request refused");
                    for problem in problems {
                        eprintln!("{problem}");
                    }
                }
            }
            std::process::exit(err.code());
        }
    }
}

fn run() -> Result<Output, Fail> {
    match parse_args(std::env::args().skip(1))? {
        Args::Help => Ok(Output::StdoutLine(USAGE.to_string())),
        Args::Version => Ok(Output::StdoutLine(VERSION.to_string())),
        Args::Dump(path) => Ok(Output::StdoutRaw(dump_file(&path)?)),
        Args::Check { dbcs, request } => check_only(&dbcs, &request),
        Args::Inspect { log, dbcs, no_hash } => inspect_cmd(&log, &dbcs, no_hash),
        Args::Stats {
            log,
            dbcs,
            signals,
            from_us,
            to_us,
        } => stats_cmd(&log, &dbcs, &signals, from_us, to_us),
        Args::Samples {
            log,
            dbcs,
            signals,
            from_us,
            to_us,
            max_points,
        } => samples_cmd(&log, &dbcs, &signals, from_us, to_us, max_points),
        Args::Judge {
            log,
            dbcs,
            request,
            out,
        } => judge_files(&log, &dbcs, &request, &out),
    }
}

fn load_map_set(dbc_args: &[(String, u8)]) -> Result<(SignalMap, Vec<VerdictDbc>), Fail> {
    let mut opened = Vec::with_capacity(dbc_args.len());
    for (path, channel) in dbc_args {
        opened.push(read_dbc(path, *channel)?);
    }
    let mut opened = opened.into_iter();
    let first = opened
        .next()
        .ok_or_else(|| Fail::Usage(format!("missing --dbc\n{USAGE}")))?;
    let mut map = first.map;
    // `append` applies the channel for every DBC after the first. The first
    // one is already the map, so it takes the same channel rule directly.
    apply_channel(&mut map, first.meta.channel);
    let mut dbcs = vec![first.meta];
    for dbc in opened {
        let channel = dbc.meta.channel;
        map.append(dbc.map, channel);
        dbcs.push(dbc.meta);
    }
    Ok((map, dbcs))
}

fn load_optional_map(dbc_args: &[(String, u8)]) -> Result<Option<SignalMap>, Fail> {
    if dbc_args.is_empty() {
        return Ok(None);
    }
    let (map, _) = load_map_set(dbc_args)?;
    Ok(Some(map))
}

fn check_only(dbc_args: &[(String, u8)], request_path: &str) -> Result<Output, Fail> {
    let request_bytes = read_input(request_path)?;
    let request_text = as_text(&request_bytes, request_path)?;
    let (map, _) = load_map_set(dbc_args)?;
    let request = match judge::check(request_text, &map) {
        Ok(request) => request,
        Err(refusal) => return Err(Fail::Refused(refusal.problems)),
    };
    let checks: usize = request.cases.iter().map(|c| c.checks.len()).sum();
    Ok(Output::StdoutLine(format!(
        "ok spec_id={} cases={} checks={}",
        request.spec_id,
        request.cases.len(),
        checks
    )))
}

fn inspect_cmd(log_path: &str, dbc_args: &[(String, u8)], no_hash: bool) -> Result<Output, Fail> {
    let map = load_optional_map(dbc_args)?;
    let sha256 = if no_hash {
        None
    } else {
        Some(sha256_file(log_path)?)
    };
    let report = inspect_log(Path::new(log_path), map.as_ref(), sha256)
        .map_err(|err| Fail::File(err.to_string()))?;
    let json =
        loom_core::inspect::render_json(&report).map_err(|err| Fail::File(err.to_string()))?;
    Ok(Output::StdoutRaw(json))
}

fn stats_cmd(
    log_path: &str,
    dbc_args: &[(String, u8)],
    signals: &[String],
    from_us: Option<u64>,
    to_us: Option<u64>,
) -> Result<Output, Fail> {
    let map = load_optional_map(dbc_args)?;
    let report = stats_for_log(Path::new(log_path), map.as_ref(), signals, from_us, to_us)
        .map_err(|err| Fail::File(err.to_string()))?;
    let json =
        loom_core::inspect::render_json(&report).map_err(|err| Fail::File(err.to_string()))?;
    Ok(Output::StdoutRaw(json))
}

fn samples_cmd(
    log_path: &str,
    dbc_args: &[(String, u8)],
    signals: &[String],
    from_us: Option<u64>,
    to_us: Option<u64>,
    max_points: Option<usize>,
) -> Result<Output, Fail> {
    let map = load_optional_map(dbc_args)?;
    let report = samples_for_log(
        Path::new(log_path),
        map.as_ref(),
        signals,
        from_us,
        to_us,
        max_points,
    )
    .map_err(|err| Fail::File(err.to_string()))?;
    let json =
        loom_core::inspect::render_json(&report).map_err(|err| Fail::File(err.to_string()))?;
    Ok(Output::StdoutRaw(json))
}

fn judge_files(
    log_path: &str,
    dbc_args: &[(String, u8)],
    request_path: &str,
    out_path: &str,
) -> Result<Output, Fail> {
    let request_bytes = read_input(request_path)?;
    let request_text = as_text(&request_bytes, request_path)?;
    let (map, dbcs) = load_map_set(dbc_args)?;

    let request = match judge::check(request_text, &map) {
        Ok(request) => request,
        Err(refusal) => return Err(Fail::Refused(refusal.problems)),
    };

    let log_sha256 = sha256_file(log_path)?;
    let judgement = judge::evaluate_log(Path::new(log_path), &map, &request)
        .map_err(|err| Fail::File(err.to_string()))?;
    let verdict = judge::render_verdict(
        &judgement.evaluation,
        &sha256_bytes(&request_bytes),
        &VerdictLog {
            file: base_name(log_path).to_string(),
            sha256: log_sha256,
            t_start_us: judgement.log.t_start_us,
            t_end_us: judgement.log.t_end_us,
            frames: judgement.log.frames,
            skipped: judgement.log.skipped,
            events_truncated: judgement.log.events_truncated,
        },
        &dbcs,
    );
    let summary_line = summary(&judgement.evaluation);
    if out_path == "-" {
        return Ok(Output::VerdictStdout {
            verdict,
            summary: summary_line,
        });
    }
    std::fs::write(out_path, verdict.as_bytes())
        .map_err(|err| Fail::File(format!("could not write {out_path}: {err}")))?;
    Ok(Output::StdoutLine(summary_line))
}

#[derive(Debug)]
enum Args {
    Help,
    Version,
    Judge {
        log: String,
        dbcs: Vec<(String, u8)>,
        request: String,
        out: String,
    },
    Check {
        dbcs: Vec<(String, u8)>,
        request: String,
    },
    Inspect {
        log: String,
        dbcs: Vec<(String, u8)>,
        no_hash: bool,
    },
    Stats {
        log: String,
        dbcs: Vec<(String, u8)>,
        signals: Vec<String>,
        from_us: Option<u64>,
        to_us: Option<u64>,
    },
    Samples {
        log: String,
        dbcs: Vec<(String, u8)>,
        signals: Vec<String>,
        from_us: Option<u64>,
        to_us: Option<u64>,
        max_points: Option<usize>,
    },
    Dump(String),
}

enum Output {
    StdoutLine(String),
    StdoutRaw(String),
    VerdictStdout { verdict: String, summary: String },
}

struct OpenDbc {
    map: SignalMap,
    meta: VerdictDbc,
}

#[derive(Debug)]
enum Fail {
    Usage(String),
    File(String),
    Refused(Vec<String>),
}

impl Fail {
    fn code(&self) -> i32 {
        match self {
            Self::Usage(_) | Self::File(_) => 1,
            Self::Refused(_) => 2,
        }
    }
}

fn parse_args<I>(args: I) -> Result<Args, Fail>
where
    I: IntoIterator<Item = String>,
{
    let mut log = None;
    let mut request = None;
    let mut out = None;
    let mut dump_dbc = None;
    let mut check_mode = false;
    let mut inspect_mode = false;
    let mut no_hash = false;
    let mut stats_signals: Option<Vec<String>> = None;
    let mut samples_signals: Option<Vec<String>> = None;
    let mut from_us: Option<u64> = None;
    let mut to_us: Option<u64> = None;
    let mut max_points: Option<usize> = None;
    let mut dbcs = Vec::new();

    let mut args = args.into_iter().peekable();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Args::Help),
            "--version" | "-V" => return Ok(Args::Version),
            "--log" => set_once(&mut log, "--log", need(&mut args, "--log")?)?,
            "--request" => set_once(&mut request, "--request", need(&mut args, "--request")?)?,
            "--out" => set_once(&mut out, "--out", need(&mut args, "--out")?)?,
            "--dump-dbc" => set_once(&mut dump_dbc, "--dump-dbc", need(&mut args, "--dump-dbc")?)?,
            "--dbc" => dbcs.push(split_dbc(&need(&mut args, "--dbc")?)?),
            "--check" => check_mode = true,
            "--inspect" => inspect_mode = true,
            "--no-hash" => no_hash = true,
            "--stats" => {
                if stats_signals.is_some() {
                    return Err(Fail::Usage(format!("--stats was given twice\n{USAGE}")));
                }
                let list = if args.peek().is_some_and(|next| !next.starts_with('-')) {
                    split_csv_names(&args.next().unwrap())
                } else {
                    Vec::new()
                };
                stats_signals = Some(list);
            }
            "--samples" => {
                if samples_signals.is_some() {
                    return Err(Fail::Usage(format!("--samples was given twice\n{USAGE}")));
                }
                let raw = need(&mut args, "--samples")?;
                let list = split_csv_names(&raw);
                if list.is_empty() {
                    return Err(Fail::Usage(format!(
                        "--samples needs at least one signal name\n{USAGE}"
                    )));
                }
                samples_signals = Some(list);
            }
            "--from-us" => {
                let raw = need(&mut args, "--from-us")?;
                let val = raw.parse::<u64>().map_err(|_| {
                    Fail::Usage(format!(
                        "--from-us '{raw}' is not a microsecond integer\n{USAGE}"
                    ))
                })?;
                set_once_val(&mut from_us, "--from-us", val)?;
            }
            "--to-us" => {
                let raw = need(&mut args, "--to-us")?;
                let val = raw.parse::<u64>().map_err(|_| {
                    Fail::Usage(format!(
                        "--to-us '{raw}' is not a microsecond integer\n{USAGE}"
                    ))
                })?;
                set_once_val(&mut to_us, "--to-us", val)?;
            }
            "--max-points" => {
                let raw = need(&mut args, "--max-points")?;
                let val = raw
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n >= 2)
                    .ok_or_else(|| {
                        Fail::Usage(format!(
                            "--max-points '{raw}' must be an integer >= 2\n{USAGE}"
                        ))
                    })?;
                set_once_val(&mut max_points, "--max-points", val)?;
            }
            other => {
                return Err(Fail::Usage(format!("unknown argument {other}\n{USAGE}")));
            }
        }
    }

    let mode_count = usize::from(dump_dbc.is_some())
        + usize::from(check_mode)
        + usize::from(inspect_mode)
        + usize::from(stats_signals.is_some())
        + usize::from(samples_signals.is_some());
    if mode_count > 1 {
        return Err(Fail::Usage(format!(
            "choose only one of --dump-dbc, --check, --inspect, --stats, or --samples\n{USAGE}"
        )));
    }

    if let Some(path) = dump_dbc {
        if log.is_some()
            || request.is_some()
            || out.is_some()
            || !dbcs.is_empty()
            || no_hash
            || from_us.is_some()
            || to_us.is_some()
            || max_points.is_some()
        {
            return Err(Fail::Usage(format!(
                "--dump-dbc does not take other arguments\n{USAGE}"
            )));
        }
        return Ok(Args::Dump(path));
    }

    if check_mode {
        if log.is_some()
            || out.is_some()
            || no_hash
            || from_us.is_some()
            || to_us.is_some()
            || max_points.is_some()
        {
            return Err(Fail::Usage(format!(
                "--check only takes --dbc and --request\n{USAGE}"
            )));
        }
        if dbcs.is_empty() {
            return Err(Fail::Usage(format!("missing --dbc\n{USAGE}")));
        }
        return Ok(Args::Check {
            dbcs,
            request: request.ok_or_else(|| Fail::Usage(format!("missing --request\n{USAGE}")))?,
        });
    }

    if inspect_mode {
        if request.is_some()
            || out.is_some()
            || from_us.is_some()
            || to_us.is_some()
            || max_points.is_some()
        {
            return Err(Fail::Usage(format!(
                "--inspect only takes --log, optional --dbc, and optional --no-hash\n{USAGE}"
            )));
        }
        return Ok(Args::Inspect {
            log: log.ok_or_else(|| Fail::Usage(format!("missing --log\n{USAGE}")))?,
            dbcs,
            no_hash,
        });
    }

    if let Some(signals) = stats_signals {
        if request.is_some() || out.is_some() || no_hash || max_points.is_some() {
            return Err(Fail::Usage(format!(
                "--stats only takes --log, optional --dbc, --from-us, and --to-us\n{USAGE}"
            )));
        }
        return Ok(Args::Stats {
            log: log.ok_or_else(|| Fail::Usage(format!("missing --log\n{USAGE}")))?,
            dbcs,
            signals,
            from_us,
            to_us,
        });
    }

    if let Some(signals) = samples_signals {
        if request.is_some() || out.is_some() || no_hash {
            return Err(Fail::Usage(format!(
                "--samples only takes --log, optional --dbc, --from-us, --to-us, and --max-points\n{USAGE}"
            )));
        }
        return Ok(Args::Samples {
            log: log.ok_or_else(|| Fail::Usage(format!("missing --log\n{USAGE}")))?,
            dbcs,
            signals,
            from_us,
            to_us,
            max_points,
        });
    }

    if no_hash || from_us.is_some() || to_us.is_some() || max_points.is_some() {
        return Err(Fail::Usage(format!(
            "--no-hash / --from-us / --to-us / --max-points require --inspect, --stats, or --samples\n{USAGE}"
        )));
    }
    if dbcs.is_empty() {
        return Err(Fail::Usage(format!("missing --dbc\n{USAGE}")));
    }
    Ok(Args::Judge {
        log: log.ok_or_else(|| Fail::Usage(format!("missing --log\n{USAGE}")))?,
        dbcs,
        request: request.ok_or_else(|| Fail::Usage(format!("missing --request\n{USAGE}")))?,
        out: out.ok_or_else(|| Fail::Usage(format!("missing --out\n{USAGE}")))?,
    })
}

fn split_csv_names(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn set_once(slot: &mut Option<String>, flag: &str, value: String) -> Result<(), Fail> {
    set_once_val(slot, flag, value)
}

fn set_once_val<T>(slot: &mut Option<T>, flag: &str, value: T) -> Result<(), Fail> {
    if slot.is_some() {
        return Err(Fail::Usage(format!("{flag} was given twice\n{USAGE}")));
    }
    *slot = Some(value);
    Ok(())
}

fn need<I>(args: &mut I, flag: &str) -> Result<String, Fail>
where
    I: Iterator<Item = String>,
{
    args.next()
        .ok_or_else(|| Fail::Usage(format!("{flag} needs a value\n{USAGE}")))
}

/// `FILE` is channel 0. `FILE@CHANNEL` takes the channel when the text after
/// the last `@` is an integer in `0..=255`.
fn split_dbc(arg: &str) -> Result<(String, u8), Fail> {
    if let Some((file, channel)) = arg.rsplit_once('@') {
        if !file.is_empty()
            && !channel.is_empty()
            && channel.bytes().all(|byte| byte.is_ascii_digit())
        {
            let value: u16 = channel.parse().map_err(|_| {
                Fail::Usage(format!("--dbc channel in {arg} is not an integer\n{USAGE}"))
            })?;
            let channel = u8::try_from(value).map_err(|_| {
                Fail::Usage(format!(
                    "--dbc channel {value} is not between 0 and 255\n{USAGE}"
                ))
            })?;
            return Ok((file.to_string(), channel));
        }
    }
    Ok((arg.to_string(), 0))
}

/// The text after the last `/` or `\`, on every platform.
///
/// A Windows drive prefix with no separator (`C:brake.asc`) is not part of
/// the name. `Path::file_name` is the wrong tool here: on Linux it leaves
/// `\` in the name.
fn base_name(path: &str) -> &str {
    let bytes = path.as_bytes();
    let start = bytes
        .iter()
        .rposition(|byte| *byte == b'/' || *byte == b'\\')
        .map(|index| index + 1)
        .unwrap_or(0);
    let name = &path[start..];
    let name_bytes = name.as_bytes();
    if name_bytes.len() >= 2 && name_bytes[0].is_ascii_alphabetic() && name_bytes[1] == b':' {
        &name[2..]
    } else {
        name
    }
}

fn apply_channel(map: &mut SignalMap, channel: u8) {
    if channel == 0 {
        return;
    }
    for signal in &mut map.signals {
        if signal.channel == 0 {
            signal.channel = channel;
        }
    }
}

fn summary(evaluation: &Evaluation) -> String {
    let counts = judge::count_cases(&evaluation.cases);
    format!(
        "cases={} pass={} fail={} inconclusive={} not_exercised={} not_judgeable={}",
        counts.cases,
        counts.pass,
        counts.fail,
        counts.inconclusive,
        counts.not_exercised,
        counts.not_judgeable
    )
}

fn dump_file(path: &str) -> Result<String, Fail> {
    let bytes = read_file(path)?;
    let text = as_text(&bytes, path)?;
    let map = parse_map_or_dbc(text).map_err(|err| {
        Fail::File(format!(
            "could not read {path} as a DBC or signal map: {err}"
        ))
    })?;
    Ok(loom_core::dump_dbc(&map))
}

fn read_dbc(path: &str, channel: u8) -> Result<OpenDbc, Fail> {
    let bytes = read_file(path)?;
    let text = as_text(&bytes, path)?;
    let map = parse_map_or_dbc(text).map_err(|err| {
        Fail::File(format!(
            "could not read {path} as a DBC or signal map: {err}"
        ))
    })?;
    Ok(OpenDbc {
        map,
        meta: VerdictDbc {
            file: base_name(path).to_string(),
            sha256: sha256_bytes(&bytes),
            channel,
        },
    })
}

fn read_input(path: &str) -> Result<Vec<u8>, Fail> {
    if path == "-" {
        let mut buf = Vec::new();
        std::io::stdin()
            .read_to_end(&mut buf)
            .map_err(|err| Fail::File(format!("could not read stdin: {err}")))?;
        return Ok(buf);
    }
    read_file(path)
}

fn read_file(path: &str) -> Result<Vec<u8>, Fail> {
    std::fs::read(path).map_err(|err| Fail::File(format!("could not read {path}: {err}")))
}

fn as_text<'a>(bytes: &'a [u8], path: &str) -> Result<&'a str, Fail> {
    std::str::from_utf8(bytes).map_err(|_| Fail::File(format!("could not read {path}: not UTF-8")))
}

fn sha256_file(path: &str) -> Result<String, Fail> {
    let mut file = std::fs::File::open(path)
        .map_err(|err| Fail::File(format!("could not read {path}: {err}")))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|err| Fail::File(format!("could not read {path}: {err}")))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex_digest(&Sha256::digest(bytes))
}

fn hex_digest(digest: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0xf) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base_name, parse_args, split_dbc, Args};

    #[test]
    fn base_name_splits_on_slash_and_backslash() {
        assert_eq!(base_name(r"C:\data\brake.asc"), "brake.asc");
        assert_eq!(base_name("/tmp/x/brake.asc"), "brake.asc");
        assert!(!base_name(r"C:\data\brake.asc").contains('\\'));
        assert!(!base_name(r"C:\data\brake.asc").contains('/'));
        assert!(!base_name(r"C:\data\brake.asc").contains(':'));
        assert!(!base_name("/tmp/x/brake.asc").contains('/'));
        assert!(!base_name("/tmp/x/brake.asc").contains('\\'));
    }

    #[test]
    fn base_name_strips_a_drive_prefix_and_keeps_a_plain_name() {
        assert_eq!(base_name(r"C:brake.asc"), "brake.asc");
        assert_eq!(base_name("brake.asc"), "brake.asc");
        assert_eq!(base_name(r"\\server\share\brake.asc"), "brake.asc");
        assert_eq!(base_name("C:/data/brake.asc"), "brake.asc");
    }

    #[test]
    fn plain_dbc_is_channel_zero_and_at_channel_is_that_channel() {
        assert_eq!(
            split_dbc("fixtures/judge/brake.dbc").unwrap(),
            ("fixtures/judge/brake.dbc".to_string(), 0)
        );
        assert_eq!(
            split_dbc(r"C:\data\brake.dbc@2").unwrap(),
            (r"C:\data\brake.dbc".to_string(), 2)
        );
        assert_eq!(base_name(r"C:\data\brake.dbc"), "brake.dbc");
        assert!(split_dbc("brake.dbc@256").is_err());
        assert_eq!(
            split_dbc("brake.dbc@-1").unwrap(),
            ("brake.dbc@-1".to_string(), 0)
        );
    }

    #[test]
    fn help_and_version_parse_cleanly() {
        assert!(matches!(
            parse_args(["--help"].into_iter().map(str::to_string)).unwrap(),
            Args::Help
        ));
        assert!(matches!(
            parse_args(["-h"].into_iter().map(str::to_string)).unwrap(),
            Args::Help
        ));
        assert!(matches!(
            parse_args(["--version"].into_iter().map(str::to_string)).unwrap(),
            Args::Version
        ));
    }

    #[test]
    fn max_points_is_refused_where_it_does_nothing() {
        for mode in [
            &["--check", "--dbc", "a.dbc", "--request", "r.json"][..],
            &["--inspect", "--log", "a.blf"][..],
        ] {
            let args = mode
                .iter()
                .chain(["--max-points", "50"].iter())
                .map(|s| s.to_string());
            assert!(
                matches!(parse_args(args), Err(super::Fail::Usage(_))),
                "{mode:?} accepted --max-points"
            );
        }
    }

    #[test]
    fn check_inspect_stats_and_samples_modes_parse() {
        let check = parse_args(
            ["--check", "--dbc", "a.dbc", "--request", "-"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(matches!(check, Args::Check { request, .. } if request == "-"));

        let inspect = parse_args(
            ["--inspect", "--log", "a.blf", "--no-hash"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        assert!(matches!(inspect, Args::Inspect { no_hash: true, .. }));

        let stats = parse_args(
            [
                "--stats",
                "--log",
                "a.blf",
                "--from-us",
                "100",
                "--to-us",
                "500",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        assert!(matches!(
            stats,
            Args::Stats {
                ref signals,
                from_us: Some(100),
                to_us: Some(500),
                ..
            } if signals.is_empty()
        ));

        let samples = parse_args(
            [
                "--samples",
                "Speed,EngineRPM",
                "--log",
                "a.blf",
                "--max-points",
                "200",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        assert!(matches!(
            samples,
            Args::Samples {
                ref signals,
                max_points: Some(200),
                ..
            } if signals == &["Speed", "EngineRPM"]
        ));
    }

    #[test]
    fn the_command_line_requires_each_file() {
        let err = parse_args(std::iter::empty::<String>()).unwrap_err();
        assert_eq!(err.code(), 1);
        let args = parse_args(
            [
                "--out",
                "verdict.json",
                "--dbc",
                "brake.dbc@1",
                "--log",
                "brake.asc",
                "--request",
                "request.json",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        let Args::Judge {
            log,
            dbcs,
            request,
            out,
        } = args
        else {
            panic!("judge command");
        };
        assert_eq!(log, "brake.asc");
        assert_eq!(dbcs, vec![("brake.dbc".to_string(), 1)]);
        assert_eq!(request, "request.json");
        assert_eq!(out, "verdict.json");

        let two = parse_args(
            [
                "--log",
                "brake.asc",
                "--dbc",
                "a.dbc",
                "--dbc",
                r"C:\data\b.dbc@3",
                "--request",
                "request.json",
                "--out",
                "verdict.json",
            ]
            .into_iter()
            .map(str::to_string),
        )
        .unwrap();
        let Args::Judge { dbcs, .. } = two else {
            panic!("judge command");
        };
        assert_eq!(
            dbcs,
            vec![("a.dbc".to_string(), 0), (r"C:\data\b.dbc".to_string(), 3),]
        );
        assert_eq!(base_name(r"C:\data\b.dbc"), "b.dbc");
    }

    #[test]
    fn dump_dbc_is_its_own_command() {
        let args = parse_args(
            ["--dump-dbc", "fixtures/hypercar_lap.dbc"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap();
        let Args::Dump(path) = args else {
            panic!("dump command");
        };
        assert_eq!(path, "fixtures/hypercar_lap.dbc");

        let err = parse_args(
            ["--dump-dbc", "a.dbc", "--log", "a.asc"]
                .into_iter()
                .map(str::to_string),
        )
        .unwrap_err();
        assert_eq!(err.code(), 1);
    }
}
