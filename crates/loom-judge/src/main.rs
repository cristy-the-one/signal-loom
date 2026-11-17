//! `loom-judge --log FILE --dbc FILE [--dbc FILE@CHANNEL] --request FILE --out FILE`
//!
//! Exit 0 after a verdict is written, whatever the verdicts were. Exit 2 when
//! the request is refused, and write no verdict file. Exit 1 when a file
//! could not be read or written, or the command line could not be used.
//!
//! The verdict names each file by the text after its last `/` or `\`.
//! `Path::file_name` is not used: on Linux it does not split on `\`.

use std::io::Read;
use std::path::Path;

use loom_core::judge::{self, Evaluation, VerdictDbc, VerdictLog};
use loom_core::SignalMap;
use sha2::{Digest, Sha256};

const USAGE: &str = "\
usage: loom-judge --log FILE --dbc FILE [--dbc FILE@CHANNEL] --request FILE --out FILE";

fn main() {
    match run() {
        Ok(line) => println!("{line}"),
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

fn run() -> Result<String, Fail> {
    let args = parse_args(std::env::args().skip(1))?;
    let request_bytes = read_file(&args.request)?;
    let request_text = as_text(&request_bytes, &args.request)?;

    let mut opened = Vec::with_capacity(args.dbcs.len());
    for (path, channel) in &args.dbcs {
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

    let request = match judge::check(request_text, &map) {
        Ok(request) => request,
        Err(refusal) => return Err(Fail::Refused(refusal.problems)),
    };

    let log_sha256 = sha256_file(&args.log)?;
    let judgement = judge::evaluate_log(Path::new(&args.log), &map, &request)
        .map_err(|err| Fail::File(err.to_string()))?;
    let verdict = judge::render_verdict(
        &judgement.evaluation,
        &sha256_bytes(&request_bytes),
        &VerdictLog {
            file: base_name(&args.log).to_string(),
            sha256: log_sha256,
            t_start_us: judgement.log.t_start_us,
            t_end_us: judgement.log.t_end_us,
            frames: judgement.log.frames,
            skipped: judgement.log.skipped,
            events_truncated: judgement.log.events_truncated,
        },
        &dbcs,
    );
    std::fs::write(&args.out, verdict.as_bytes())
        .map_err(|err| Fail::File(format!("could not write {}: {err}", args.out)))?;
    Ok(summary(&judgement.evaluation))
}

#[derive(Debug)]
struct Args {
    log: String,
    dbcs: Vec<(String, u8)>,
    request: String,
    out: String,
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
    let mut dbcs = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => set_once(&mut log, "--log", need(&mut args, "--log")?)?,
            "--request" => set_once(&mut request, "--request", need(&mut args, "--request")?)?,
            "--out" => set_once(&mut out, "--out", need(&mut args, "--out")?)?,
            "--dbc" => dbcs.push(split_dbc(&need(&mut args, "--dbc")?)?),
            "--help" | "-h" => return Err(Fail::Usage(USAGE.to_string())),
            other => {
                return Err(Fail::Usage(format!("unknown argument {other}\n{USAGE}")));
            }
        }
    }
    if dbcs.is_empty() {
        return Err(Fail::Usage(format!("missing --dbc\n{USAGE}")));
    }
    Ok(Args {
        log: log.ok_or_else(|| Fail::Usage(format!("missing --log\n{USAGE}")))?,
        dbcs,
        request: request.ok_or_else(|| Fail::Usage(format!("missing --request\n{USAGE}")))?,
        out: out.ok_or_else(|| Fail::Usage(format!("missing --out\n{USAGE}")))?,
    })
}

fn set_once(slot: &mut Option<String>, flag: &str, value: String) -> Result<(), Fail> {
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

fn read_dbc(path: &str, channel: u8) -> Result<OpenDbc, Fail> {
    let bytes = read_file(path)?;
    let text = as_text(&bytes, path)?;
    let map = judge::parse_dbc(text)
        .map_err(|err| Fail::File(format!("could not read {path} as a DBC: {err}")))?;
    Ok(OpenDbc {
        map,
        meta: VerdictDbc {
            file: base_name(path).to_string(),
            sha256: sha256_bytes(&bytes),
            channel,
        },
    })
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
    use super::{base_name, parse_args, split_dbc};

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
        assert_eq!(args.log, "brake.asc");
        assert_eq!(args.dbcs, vec![("brake.dbc".to_string(), 1)]);
        assert_eq!(args.request, "request.json");
        assert_eq!(args.out, "verdict.json");

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
        assert_eq!(
            two.dbcs,
            vec![("a.dbc".to_string(), 0), (r"C:\data\b.dbc".to_string(), 3),]
        );
        assert_eq!(base_name(r"C:\data\b.dbc"), "b.dbc");
    }
}
