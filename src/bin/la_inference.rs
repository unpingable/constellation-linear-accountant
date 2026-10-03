// SPDX-License-Identifier: Apache-2.0
//! `la_inference` — durable inference accounting CLI (protocol `v: 1`).
//!
//! One JSON request on stdin → one JSON result on stdout. Requests are typed,
//! versioned, deny unknown fields, and are bounded to 64 KiB. See
//! `docs/INFERENCE_ACCOUNTING.md`. `la_cli` (v0) is a separate, unchanged binary.
//!
//! ```text
//! la_inference [--store PATH] [--dev] enroll --file OWNER_FILE
//! la_inference [--store PATH] [--dev] reserve|bind-occurrence|begin-call|settle|close|recover|inspect   < request.json
//! la_inference [--store PATH] [--dev] reconcile --milestone ID
//! la_inference version
//! ```
//!
//! Exit status: 0 result emitted (including refusals, conflicts, exhaustion);
//! 1 reconcile verdict FAIL; 2 usage/protocol error; 3 storage/integrity error.
//!
//! `--dev` opens a store owned by the invoking user (production requires root
//! and a root-owned store). Only in dev mode are the test hooks honored:
//! `LA_INFERENCE_DEV_NOW_MS` (LA clock override) and `LA_INFERENCE_DEV_CRASH`
//! (`before_commit` | `after_commit`: abort at that durability barrier). A
//! store records its mode at creation and refuses to open in the other mode.

use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::ExitCode;

use linear_accountant::inference::{
    parse_request, CrashPoint, Mode, Request, Store, DEFAULT_STORE_PATH, MAX_REQUEST_BYTES,
    PROTOCOL_VERSION, SCHEMA_VERSION,
};
use serde_json::{json, Value};

const EXIT_FAIL: u8 = 1;
const EXIT_PROTOCOL: u8 = 2;
const EXIT_STORAGE: u8 = 3;

fn emit(v: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{v}");
    let _ = out.flush();
}

fn fail(kind: &str, message: &str, code: u8) -> ExitCode {
    emit(&json!({"v": PROTOCOL_VERSION, "error": {"kind": kind, "message": message}}));
    ExitCode::from(code)
}

struct Args {
    store: PathBuf,
    mode: Mode,
    cmd: String,
    file: Option<PathBuf>,
    milestone: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut it = std::env::args().skip(1);
    let mut store = PathBuf::from(DEFAULT_STORE_PATH);
    let mut mode = Mode::Production;
    let mut cmd = None;
    let mut file = None;
    let mut milestone = None;
    while let Some(a) = it.next() {
        match a.as_str() {
            "--store" => store = PathBuf::from(it.next().ok_or("--store needs a path")?),
            "--dev" => mode = Mode::Dev,
            "--file" => file = Some(PathBuf::from(it.next().ok_or("--file needs a path")?)),
            "--milestone" => milestone = Some(it.next().ok_or("--milestone needs an id")?),
            s if s.starts_with('-') => return Err(format!("unknown option {s}")),
            s if cmd.is_none() => cmd = Some(s.to_string()),
            s => return Err(format!("unexpected argument {s}")),
        }
    }
    let cmd = cmd.ok_or("missing command")?;
    match (cmd.as_str(), &file, &milestone) {
        ("enroll", Some(_), None) | ("reconcile", None, Some(_)) => {}
        ("enroll", _, _) => return Err("enroll requires --file and nothing else".into()),
        ("reconcile", _, _) => return Err("reconcile requires --milestone".into()),
        (_, None, None) => {}
        _ => return Err(format!("{cmd} takes no --file/--milestone")),
    }
    Ok(Args {
        store,
        mode,
        cmd,
        file,
        milestone,
    })
}

fn read_bounded(r: &mut impl Read) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    r.take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("read error: {e}"))?;
    if buf.len() > MAX_REQUEST_BYTES {
        return Err(format!("request exceeds {MAX_REQUEST_BYTES} bytes"));
    }
    Ok(buf)
}

/// The owner enrollment file: no symlink, regular, bounded; in production it
/// must be root-owned and not group/other writable.
fn read_enrollment_file(path: &PathBuf, mode: Mode) -> Result<Vec<u8>, String> {
    let md = std::fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if md.file_type().is_symlink() || !md.is_file() {
        return Err("enrollment file must be a regular file, not a symlink".into());
    }
    // SAFETY: geteuid has no preconditions.
    let want_uid = match mode {
        Mode::Production => 0,
        Mode::Dev => unsafe { libc::geteuid() },
    };
    if md.uid() != want_uid || md.mode() & 0o022 != 0 {
        return Err("enrollment file has wrong owner or is group/other writable".into());
    }
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read_bounded(&mut f)
}

fn now_ms(mode: Mode) -> Result<u64, String> {
    if mode == Mode::Dev {
        if let Ok(s) = std::env::var("LA_INFERENCE_DEV_NOW_MS") {
            return s
                .parse()
                .map_err(|_| "LA_INFERENCE_DEV_NOW_MS: not a u64".into());
        }
    }
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "system clock before epoch")?;
    u64::try_from(d.as_millis()).map_err(|_| "system clock overflow".into())
}

fn crash_point(mode: Mode) -> Result<Option<CrashPoint>, String> {
    if mode != Mode::Dev {
        return Ok(None);
    }
    match std::env::var("LA_INFERENCE_DEV_CRASH").as_deref() {
        Err(_) => Ok(None),
        Ok("before_commit") => Ok(Some(CrashPoint::BeforeCommit)),
        Ok("after_commit") => Ok(Some(CrashPoint::AfterCommit)),
        Ok(other) => Err(format!("LA_INFERENCE_DEV_CRASH: unknown barrier {other:?}")),
    }
}

fn main() -> ExitCode {
    // Every file this process creates (db, WAL, SHM, lock) is owner-only.
    // SAFETY: umask has no preconditions.
    unsafe {
        libc::umask(0o077);
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => return fail("usage", &e, EXIT_PROTOCOL),
    };
    if args.cmd == "version" {
        emit(&json!({
            "v": PROTOCOL_VERSION,
            "la": "linear-accountant",
            "component": "la_inference",
            "version": env!("CARGO_PKG_VERSION"),
            "commit": option_env!("LA_GIT_COMMIT").unwrap_or("unknown"),
            "schema_version": SCHEMA_VERSION,
        }));
        return ExitCode::SUCCESS;
    }

    // Parse before touching the store: protocol errors never open or write it.
    let request = if args.cmd == "reconcile" {
        None
    } else {
        let bytes = match &args.file {
            Some(p) => read_enrollment_file(p, args.mode),
            None => read_bounded(&mut std::io::stdin().lock()),
        };
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => return fail("protocol", &e, EXIT_PROTOCOL),
        };
        match parse_request(&args.cmd, &bytes) {
            Ok(r) => Some(r),
            Err(e) => return fail("protocol", &e.0, EXIT_PROTOCOL),
        }
    };
    let (now, crash) = match (now_ms(args.mode), crash_point(args.mode)) {
        (Ok(n), Ok(c)) => (n, c),
        (Err(e), _) | (_, Err(e)) => return fail("usage", &e, EXIT_PROTOCOL),
    };

    let mut store = match Store::open(&args.store, args.mode, now) {
        Ok(s) => s,
        Err(e) => return fail(e.kind, &e.message, EXIT_STORAGE),
    };
    let (result, code) = match request {
        None => {
            let milestone = args.milestone.as_deref().unwrap_or_default();
            let r = store.books().reconcile(milestone);
            let code = if r["verdict"] == "PASS" {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(EXIT_FAIL)
            };
            (r, code)
        }
        Some(Request::Inspect(r)) => (
            store.books().inspect(r.reservation_id.as_deref()),
            ExitCode::SUCCESS,
        ),
        Some(Request::Command(c)) => match store.execute(&c, now, crash) {
            Ok(v) => (v, ExitCode::SUCCESS),
            Err(e) => return fail(e.kind, &e.message, EXIT_STORAGE),
        },
    };
    emit(&json!({"v": PROTOCOL_VERSION, "cmd": args.cmd, "result": result}));
    code
}
