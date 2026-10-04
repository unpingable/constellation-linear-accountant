// SPDX-License-Identifier: Apache-2.0
//! # Inference accounting extension (durable, single consumer)
//!
//! Durable episode/invocation accounting for ONE named consumer: the
//! Constellation remediation consumer's model decider
//! (`cartography/architecture/agentic-remediation-v2/DESIGN.md` §1). It is built
//! on the unchanged v0 core ([`InMemoryAccountant`]): enrollment `deposit`s a
//! host allocation, `reserve` draws the episode's whole cost ceiling with
//! `request_capacity`, and `begin-call` `consume`s the call's worst-case cost
//! atomically with its durable send fence. There are **no refunds**: reported
//! usage is recorded at settlement but never reverses consumption; slack and
//! unused episode allocation are retired (the token is revoked at `close`), not
//! recycled.
//!
//! The v0 Lean conservation proof (`verification/`) does **not** cover this
//! module: persistence, replay, settlement, and the inference ceilings are
//! tested here, not proven.
//!
//! ## No-storage rule
//!
//! Only typed accounting metadata enters the store. Every string field is an
//! opaque identifier restricted to `[A-Za-z0-9._:/@+=-]{1,128}` (or a decimal
//! cost string), every request type denies unknown fields, and the store
//! persists the *re-serialized typed request*, never the raw input bytes. There
//! is no field for prompts, responses, narration, reasoning, HTTP bodies, or
//! credentials, and no free-text field into which one could be smuggled.
//!
//! ## Durability
//!
//! [`Store`] is a SQLite database (WAL, `synchronous=FULL`) holding an
//! append-only `commands` log (typed request, LA-clock time, result) and an
//! append-only unique-key index (episode, reservation, send fence, settlement,
//! close). Every process holds an exclusive `flock` on `<db>.lock` while it
//! replays the log into a fresh core and executes one command. Replay recomputes
//! every stored result and refuses to start on any difference.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OpenFlags, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    BudgetAdmissionRef, CapacityDecision, CapacityRequest, ConsumeRequest, ConsumptionDecision,
    DepositDecision, Event, EventId, InMemoryAccountant, RequestId, Scope, TokenId,
};

/// Wire protocol version (`"v"` in every request and result).
pub const PROTOCOL_VERSION: u32 = 1;
/// Store schema version recorded in `meta`.
pub const SCHEMA_VERSION: &str = "1";
/// Upper bound on one request (stdin or enrollment file), in bytes.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Default production store path.
pub const DEFAULT_STORE_PATH: &str = "/var/lib/linear-accountant/inference.sqlite";

const MAX_ID_LEN: usize = 128;
const MAX_MODELS: usize = 16;
const MAX_KNOWN_UNSENT: usize = 64;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A malformed request. Never logged; the CLI exits nonzero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub String);

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A storage, integrity, ownership, or clock failure. The CLI exits nonzero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError {
    pub kind: &'static str,
    pub message: String,
}

impl StoreError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        StoreError {
            kind,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        StoreError::new("sqlite", e.to_string())
    }
}

// ---------------------------------------------------------------------------
// Typed requests (deny unknown fields; identifiers only)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ModelClass {
    pub provider: String,
    pub model_class: String,
}

/// Episode-level ceilings. Token ceilings are aggregate over the episode's
/// begun calls; `retry_ceiling` counts attempts after the first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EpisodeBounds {
    pub max_calls: u32,
    pub retry_ceiling: u32,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_cost_micro_usd: u64,
    pub max_wall_ms: u64,
}

/// Per-call ceilings an enrollment allows a single call envelope to request.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CallBounds {
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_cost_micro_usd: u64,
    pub max_wall_ms: u64,
}

/// Owner-installed enrollment (privileged, imported only via `enroll --file`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EnrollRequest {
    pub v: u32,
    pub cmd: String,
    pub admission_id: String,
    pub admission_ref: String,
    pub basis_kind: String,
    pub actor: String,
    pub scope: String,
    pub milestone_id: String,
    pub host_allocation_micro_usd: u64,
    pub valid_from_unix_ms: u64,
    pub valid_until_unix_ms: u64,
    pub models: Vec<ModelClass>,
    pub episode_ceilings: EpisodeBounds,
    pub call_ceilings: CallBounds,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReserveRequest {
    pub v: u32,
    pub cmd: String,
    pub admission_id: String,
    pub episode_id: String,
    pub condition_ref: String,
    #[serde(default)]
    pub ag_campaign: Option<String>,
    #[serde(default)]
    pub ag_occurrence: Option<String>,
    pub eligibility_ref: String,
    pub eligibility_valid_until_unix_ms: u64,
    pub provider: String,
    pub model_class: String,
    pub bounds: EpisodeBounds,
    pub deadline_unix_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BindRequest {
    pub v: u32,
    pub cmd: String,
    pub reservation_id: String,
    pub ag_campaign: String,
    pub ag_occurrence: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BeginCallRequest {
    pub v: u32,
    pub cmd: String,
    pub reservation_id: String,
    pub call_index: u32,
    pub request_policy_digest: String,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_cost_micro_usd: u64,
    pub max_wall_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Usage {
    pub input_units: u64,
    pub output_units: u64,
    pub total_units: u64,
    #[serde(default)]
    pub reasoning_units: Option<u64>,
    #[serde(default)]
    pub cache_read_units: Option<u64>,
    #[serde(default)]
    pub cache_write_units: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClass {
    Proposal,
    Abstain,
    Escalate,
    Malformed,
    ProviderError,
    Timeout,
    CrashUnknown,
    CancelledUnsent,
    BudgetExhausted,
    RetryExhausted,
    AccountingError,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SettleRequest {
    pub v: u32,
    pub cmd: String,
    pub invocation_id: String,
    pub terminal_class: TerminalClass,
    #[serde(default)]
    pub reported_provider: Option<String>,
    #[serde(default)]
    pub reported_model: Option<String>,
    #[serde(default)]
    pub provider_generation_id: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
    /// Provider account charge in USD as a plain decimal string (no float).
    #[serde(default)]
    pub actual_cost_usd: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CloseRequest {
    pub v: u32,
    pub cmd: String,
    pub reservation_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoverRequest {
    pub v: u32,
    pub cmd: String,
    #[serde(default)]
    pub reservation_id: Option<String>,
    /// Invocations the caller's own durable state proves were never sent.
    #[serde(default)]
    pub known_unsent: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InspectRequest {
    pub v: u32,
    pub cmd: String,
    #[serde(default)]
    pub reservation_id: Option<String>,
}

/// A state-changing command as persisted in the append-only log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", content = "request", rename_all = "kebab-case")]
pub enum Command {
    Enroll(EnrollRequest),
    Reserve(ReserveRequest),
    BindOccurrence(BindRequest),
    BeginCall(BeginCallRequest),
    Settle(SettleRequest),
    Close(CloseRequest),
    Recover(RecoverRequest),
}

impl Command {
    pub fn name(&self) -> &'static str {
        match self {
            Command::Enroll(_) => "enroll",
            Command::Reserve(_) => "reserve",
            Command::BindOccurrence(_) => "bind-occurrence",
            Command::BeginCall(_) => "begin-call",
            Command::Settle(_) => "settle",
            Command::Close(_) => "close",
            Command::Recover(_) => "recover",
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing and static validation
// ---------------------------------------------------------------------------

fn perr<T>(msg: impl Into<String>) -> Result<T, ProtocolError> {
    Err(ProtocolError(msg.into()))
}

/// True for an opaque identifier: 1..=128 bytes of `[A-Za-z0-9._:/@+=-]`.
pub fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ID_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/@+=-".contains(&b))
}

fn check_id(field: &str, s: &str) -> Result<(), ProtocolError> {
    if valid_id(s) {
        Ok(())
    } else {
        perr(format!(
            "field {field}: not an opaque identifier ([A-Za-z0-9._:/@+=-], 1..={MAX_ID_LEN} bytes)"
        ))
    }
}

fn check_opt_id(field: &str, s: &Option<String>) -> Result<(), ProtocolError> {
    match s {
        Some(s) => check_id(field, s),
        None => Ok(()),
    }
}

fn check_positive(field: &str, n: u64) -> Result<(), ProtocolError> {
    if n == 0 {
        perr(format!("field {field}: must be > 0"))
    } else {
        Ok(())
    }
}

fn check_header(v: u32, cmd: &str, expected: &str) -> Result<(), ProtocolError> {
    if v != PROTOCOL_VERSION {
        return perr(format!(
            "unsupported protocol version {v} (expected {PROTOCOL_VERSION})"
        ));
    }
    if cmd != expected {
        return perr(format!(
            "cmd mismatch: request names {cmd:?}, invoked as {expected:?}"
        ));
    }
    Ok(())
}

fn check_episode_bounds(b: &EpisodeBounds) -> Result<(), ProtocolError> {
    check_positive("max_calls", b.max_calls as u64)?;
    check_positive("max_input_tokens", b.max_input_tokens)?;
    check_positive("max_output_tokens", b.max_output_tokens)?;
    check_positive("max_cost_micro_usd", b.max_cost_micro_usd)?;
    check_positive("max_wall_ms", b.max_wall_ms)?;
    Ok(())
}

/// Convert a provider decimal USD charge to integer micro-USD, rounding UP.
/// Accepts `DIGITS[.DIGITS]` only (no sign, exponent, or float parsing).
pub fn usd_to_micro_ceil(s: &str) -> Result<u64, ProtocolError> {
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    let digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
    if int_part.is_empty()
        || int_part.len() > 20
        || !digits(int_part)
        || (s.contains('.') && frac_part.is_empty())
        || frac_part.len() > 30
        || !digits(frac_part)
    {
        return perr("actual_cost_usd: expected a plain decimal string like \"0.000123\"");
    }
    let overflow = || ProtocolError("actual_cost_usd: overflows u64 micro-USD".into());
    let int: u64 = int_part.parse().map_err(|_| overflow())?;
    let mut micro = int.checked_mul(1_000_000).ok_or_else(overflow)?;
    let head: String = frac_part.chars().take(6).collect();
    let head = format!("{head:0<6}");
    micro = micro
        .checked_add(head.parse::<u64>().map_err(|_| overflow())?)
        .ok_or_else(overflow)?;
    if frac_part.chars().skip(6).any(|c| c != '0') {
        micro = micro.checked_add(1).ok_or_else(overflow)?;
    }
    Ok(micro)
}

fn parse_bounded<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, ProtocolError> {
    if bytes.len() > MAX_REQUEST_BYTES {
        return perr(format!("request exceeds {MAX_REQUEST_BYTES} bytes"));
    }
    serde_json::from_slice(bytes).map_err(|e| {
        let mut m = e.to_string();
        m.truncate(200);
        ProtocolError(format!("invalid request JSON: {m}"))
    })
}

/// A parsed, validated request: either a logged command or a read-only query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Command(Box<Command>),
    Inspect(InspectRequest),
}

/// Parse and statically validate one request for the CLI command `cmd`.
pub fn parse_request(cmd: &str, bytes: &[u8]) -> Result<Request, ProtocolError> {
    let r = match cmd {
        "enroll" => {
            let r: EnrollRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            for (f, s) in [
                ("admission_id", &r.admission_id),
                ("admission_ref", &r.admission_ref),
                ("basis_kind", &r.basis_kind),
                ("actor", &r.actor),
                ("scope", &r.scope),
                ("milestone_id", &r.milestone_id),
            ] {
                check_id(f, s)?;
            }
            check_positive("host_allocation_micro_usd", r.host_allocation_micro_usd)?;
            if r.valid_until_unix_ms <= r.valid_from_unix_ms {
                return perr("valid_until_unix_ms must be after valid_from_unix_ms");
            }
            if r.models.is_empty() || r.models.len() > MAX_MODELS {
                return perr(format!("models: 1..={MAX_MODELS} entries required"));
            }
            for m in &r.models {
                check_id("provider", &m.provider)?;
                check_id("model_class", &m.model_class)?;
            }
            check_episode_bounds(&r.episode_ceilings)?;
            let c = &r.call_ceilings;
            check_positive("call max_input_tokens", c.max_input_tokens)?;
            check_positive("call max_output_tokens", c.max_output_tokens)?;
            check_positive("call max_cost_micro_usd", c.max_cost_micro_usd)?;
            check_positive("call max_wall_ms", c.max_wall_ms)?;
            // Ceiling products must be representable (overflow-checked ceilings).
            let calls = r.episode_ceilings.max_calls as u64;
            for (f, per) in [
                ("max_input_tokens", c.max_input_tokens),
                ("max_output_tokens", c.max_output_tokens),
                ("max_cost_micro_usd", c.max_cost_micro_usd),
                ("max_wall_ms", c.max_wall_ms),
            ] {
                if calls.checked_mul(per).is_none() {
                    return perr(format!("max_calls * call {f} overflows u64"));
                }
            }
            Request::Command(Box::new(Command::Enroll(r)))
        }
        "reserve" => {
            let r: ReserveRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            for (f, s) in [
                ("admission_id", &r.admission_id),
                ("episode_id", &r.episode_id),
                ("condition_ref", &r.condition_ref),
                ("eligibility_ref", &r.eligibility_ref),
                ("provider", &r.provider),
                ("model_class", &r.model_class),
            ] {
                check_id(f, s)?;
            }
            check_opt_id("ag_campaign", &r.ag_campaign)?;
            check_opt_id("ag_occurrence", &r.ag_occurrence)?;
            check_episode_bounds(&r.bounds)?;
            Request::Command(Box::new(Command::Reserve(r)))
        }
        "bind-occurrence" => {
            let r: BindRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            check_id("reservation_id", &r.reservation_id)?;
            check_id("ag_campaign", &r.ag_campaign)?;
            check_id("ag_occurrence", &r.ag_occurrence)?;
            Request::Command(Box::new(Command::BindOccurrence(r)))
        }
        "begin-call" => {
            let r: BeginCallRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            check_id("reservation_id", &r.reservation_id)?;
            check_id("request_policy_digest", &r.request_policy_digest)?;
            check_positive("max_input_tokens", r.max_input_tokens)?;
            check_positive("max_output_tokens", r.max_output_tokens)?;
            check_positive("max_cost_micro_usd", r.max_cost_micro_usd)?;
            check_positive("max_wall_ms", r.max_wall_ms)?;
            Request::Command(Box::new(Command::BeginCall(r)))
        }
        "settle" => {
            let r: SettleRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            check_id("invocation_id", &r.invocation_id)?;
            check_opt_id("reported_provider", &r.reported_provider)?;
            check_opt_id("reported_model", &r.reported_model)?;
            check_opt_id("provider_generation_id", &r.provider_generation_id)?;
            if let Some(c) = &r.actual_cost_usd {
                usd_to_micro_ceil(c)?;
            }
            Request::Command(Box::new(Command::Settle(r)))
        }
        "close" => {
            let r: CloseRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            check_id("reservation_id", &r.reservation_id)?;
            Request::Command(Box::new(Command::Close(r)))
        }
        "recover" => {
            let r: RecoverRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            check_opt_id("reservation_id", &r.reservation_id)?;
            if r.known_unsent.len() > MAX_KNOWN_UNSENT {
                return perr(format!("known_unsent: at most {MAX_KNOWN_UNSENT} entries"));
            }
            for id in &r.known_unsent {
                check_id("known_unsent[]", id)?;
            }
            Request::Command(Box::new(Command::Recover(r)))
        }
        "inspect" => {
            let r: InspectRequest = parse_bounded(bytes)?;
            check_header(r.v, &r.cmd, cmd)?;
            check_opt_id("reservation_id", &r.reservation_id)?;
            Request::Inspect(r)
        }
        other => return perr(format!("unknown command {other:?}")),
    };
    Ok(r)
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Dimension {
    Calls,
    Input,
    Output,
    Cost,
    Wall,
    Retries,
    Milestone,
}

/// The only shape an exhausted budget takes. Never "fixed".
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ExhaustedOutcome {
    pub dimension: Dimension,
    pub terminal_class: TerminalClass,
    pub attempts_used: u32,
    pub reasoning_stopped: bool,
    pub escalation_required: bool,
}

impl ExhaustedOutcome {
    fn new(dimension: Dimension, attempts_used: u32) -> Self {
        let terminal_class = if dimension == Dimension::Retries {
            TerminalClass::RetryExhausted
        } else {
            TerminalClass::BudgetExhausted
        };
        ExhaustedOutcome {
            dimension,
            terminal_class,
            attempts_used,
            reasoning_stopped: true,
            escalation_required: true,
        }
    }
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReservationStatus {
    /// May begin another call (subject to every ceiling).
    Open,
    /// A decision class (proposal/abstain/escalate) settled; reasoning is over.
    Concluded,
    /// Uncertain send, timeout, accounting error, or reconciliation breach.
    Frozen,
    /// A budget or retry dimension is exhausted.
    Exhausted,
    /// Closed; unused allocation retired (token revoked).
    Closed,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageSource {
    ProviderReported,
    CeilingAssumed,
    Unsent,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct EnrollmentRecord {
    pub enrollment: EnrollRequest,
    pub enrolled_unix_ms: u64,
    pub receipt: String,
    pub core_receipt: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Reservation {
    pub reservation_id: String,
    pub admission_id: String,
    pub episode_id: String,
    pub condition_ref: String,
    pub ag_campaign: Option<String>,
    pub ag_occurrence: Option<String>,
    pub eligibility_ref: String,
    pub eligibility_valid_until_unix_ms: u64,
    pub provider: String,
    pub model_class: String,
    pub bounds: EpisodeBounds,
    pub start_unix_ms: u64,
    pub deadline_unix_ms: u64,
    pub status: ReservationStatus,
    pub terminal_class: Option<TerminalClass>,
    pub exhausted: Option<ExhaustedOutcome>,
    pub freeze_reason: Option<String>,
    pub breaches: Vec<String>,
    pub invocation_ids: Vec<String>,
    pub reserved_input_tokens: u64,
    pub reserved_output_tokens: u64,
    pub consumed_micro_usd: u64,
    pub retired_micro_usd: Option<u64>,
    pub closed_unix_ms: Option<u64>,
    pub closed_from: Option<ReservationStatus>,
    pub receipt: String,
    pub core_grant_receipt: String,
    #[serde(skip)]
    request: ReserveRequest,
    #[serde(skip)]
    token: TokenId,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Settlement {
    pub invocation_id: String,
    pub reservation_id: String,
    pub terminal_class: TerminalClass,
    pub reported_provider: Option<String>,
    pub reported_model: Option<String>,
    pub provider_generation_id: Option<String>,
    pub usage: Option<Usage>,
    pub actual_cost_micro_usd: Option<u64>,
    pub accounted_cost_micro_usd: u64,
    pub call_ceiling_micro_usd: u64,
    pub slack_micro_usd: u64,
    pub overage_micro_usd: u64,
    pub usage_source: UsageSource,
    pub breaches: Vec<String>,
    pub late: bool,
    pub source: &'static str,
    pub settled_unix_ms: u64,
    pub receipt: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    pub invocation_id: String,
    pub reservation_id: String,
    pub call_index: u32,
    pub retry_index: u32,
    pub request_policy_digest: String,
    pub max_input_tokens: u64,
    pub max_output_tokens: u64,
    pub max_cost_micro_usd: u64,
    pub max_wall_ms: u64,
    pub reserved_unix_ms: u64,
    pub call_deadline_unix_ms: u64,
    pub dispatch_state: &'static str,
    pub begin_receipt: String,
    pub core_consume_receipt: String,
    pub settlement: Option<Settlement>,
    #[serde(skip)]
    begin_request: BeginCallRequest,
    #[serde(skip)]
    settle_request: Option<SettleRequest>,
}

// ---------------------------------------------------------------------------
// Books: deterministic state machine over the v0 core
// ---------------------------------------------------------------------------

/// In-memory books rebuilt by replaying the command log into a fresh core.
pub struct Books {
    core: InMemoryAccountant,
    enrollments: BTreeMap<String, EnrollmentRecord>,
    scope_owner: BTreeMap<String, String>,
    episodes: BTreeMap<String, String>,
    reservations: BTreeMap<String, Reservation>,
    invocations: BTreeMap<String, Invocation>,
    next_reservation: u64,
    last_now: u64,
    index_keys: BTreeSet<(String, String)>,
    pending_keys: Vec<(String, String)>,
}

fn class_name(c: TerminalClass) -> String {
    serde_json::to_value(c)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn rcpt(seq: u64) -> String {
    format!("lai-{seq}")
}

fn dbg<T: std::fmt::Debug>(t: &T) -> String {
    format!("{t:?}")
}

fn refused(reason: impl Into<String>) -> Value {
    json!({"outcome": "refused", "reason": reason.into()})
}

fn conflict(reason: impl Into<String>) -> Value {
    json!({"outcome": "conflict", "reason": reason.into()})
}

impl Default for Books {
    fn default() -> Self {
        Self::new()
    }
}

impl Books {
    pub fn new() -> Self {
        Books {
            core: InMemoryAccountant::new(),
            enrollments: BTreeMap::new(),
            scope_owner: BTreeMap::new(),
            episodes: BTreeMap::new(),
            reservations: BTreeMap::new(),
            invocations: BTreeMap::new(),
            next_reservation: 1,
            last_now: 0,
            index_keys: BTreeSet::new(),
            pending_keys: Vec::new(),
        }
    }

    /// The LA-clock time of the last applied command.
    pub fn last_now(&self) -> u64 {
        self.last_now
    }

    pub fn reservation(&self, id: &str) -> Option<&Reservation> {
        self.reservations.get(id)
    }

    pub fn invocation(&self, id: &str) -> Option<&Invocation> {
        self.invocations.get(id)
    }

    /// Unique keys this state implies (episode, reservation, send fence, ...).
    pub fn index_keys(&self) -> &BTreeSet<(String, String)> {
        &self.index_keys
    }

    fn key(&mut self, kind: &str, key: &str) {
        let k = (kind.to_string(), key.to_string());
        self.index_keys.insert(k.clone());
        self.pending_keys.push(k);
    }

    /// Apply one command at LA time `now` as log entry `seq`. Deterministic:
    /// replaying the same log yields byte-identical results.
    pub fn apply(&mut self, seq: u64, now: u64, cmd: &Command) -> (Value, Vec<(String, String)>) {
        self.pending_keys.clear();
        self.last_now = self.last_now.max(now);
        let v = match cmd {
            Command::Enroll(r) => self.enroll(seq, now, r),
            Command::Reserve(r) => self.reserve(seq, now, r),
            Command::BindOccurrence(r) => self.bind(r),
            Command::BeginCall(r) => self.begin_call(seq, now, r),
            Command::Settle(r) => self.settle(seq, now, r),
            Command::Close(r) => self.close(now, r),
            Command::Recover(r) => self.recover(seq, now, r),
        };
        (v, std::mem::take(&mut self.pending_keys))
    }

    fn enroll(&mut self, seq: u64, now: u64, r: &EnrollRequest) -> Value {
        if let Some(e) = self.enrollments.get(&r.admission_id) {
            if &e.enrollment == r {
                return json!({
                    "outcome": "replayed",
                    "admission_id": r.admission_id,
                    "scope": r.scope,
                    "deposited_micro_usd": r.host_allocation_micro_usd,
                    "receipt": e.receipt,
                });
            }
            return conflict("admission_id already enrolled with a different payload");
        }
        if self.scope_owner.contains_key(&r.scope) {
            return conflict("scope already belongs to another enrollment");
        }
        let scope = Scope(r.scope.clone());
        if self
            .core
            .available(&scope)
            .checked_add(r.host_allocation_micro_usd)
            .is_none()
        {
            return refused("host allocation overflows scope stock");
        }
        let admission = BudgetAdmissionRef {
            admission_ref: r.admission_ref.clone(),
            basis_kind: r.basis_kind.clone(),
        };
        let core_receipt = match self
            .core
            .deposit(&scope, r.host_allocation_micro_usd, &admission)
        {
            DepositDecision::Deposited { receipt, .. } => dbg(&receipt),
            DepositDecision::Refused { reason, .. } => return refused(reason),
        };
        self.enrollments.insert(
            r.admission_id.clone(),
            EnrollmentRecord {
                enrollment: r.clone(),
                enrolled_unix_ms: now,
                receipt: rcpt(seq),
                core_receipt: core_receipt.clone(),
            },
        );
        self.scope_owner
            .insert(r.scope.clone(), r.admission_id.clone());
        self.key("enrollment", &r.admission_id);
        self.key("scope", &r.scope);
        json!({
            "outcome": "enrolled",
            "admission_id": r.admission_id,
            "scope": r.scope,
            "deposited_micro_usd": r.host_allocation_micro_usd,
            "available_micro_usd": self.core.available(&scope),
            "receipt": rcpt(seq),
            "core_receipt": core_receipt,
        })
    }

    fn reservation_view(&self, res: &Reservation, outcome: &str) -> Value {
        json!({
            "outcome": outcome,
            "reservation_id": res.reservation_id,
            "episode_id": res.episode_id,
            "granted_micro_usd": res.bounds.max_cost_micro_usd,
            "start_unix_ms": res.start_unix_ms,
            "deadline_unix_ms": res.deadline_unix_ms,
            "status": res.status,
            "ag_campaign": res.ag_campaign,
            "ag_occurrence": res.ag_occurrence,
            "receipt": res.receipt,
        })
    }

    fn reserve(&mut self, seq: u64, now: u64, r: &ReserveRequest) -> Value {
        if let Some(rid) = self.episodes.get(&r.episode_id) {
            let res = &self.reservations[rid];
            let mut cmp = r.clone();
            // A binding filled later by bind-occurrence is equal, not new.
            if cmp.ag_campaign == res.ag_campaign && cmp.ag_occurrence == res.ag_occurrence {
                cmp.ag_campaign = res.request.ag_campaign.clone();
                cmp.ag_occurrence = res.request.ag_occurrence.clone();
            }
            if cmp == res.request {
                return self.reservation_view(res, "replayed");
            }
            return conflict("episode_id already reserved with a different payload");
        }
        let Some(enr) = self.enrollments.get(&r.admission_id) else {
            return refused("unknown_admission");
        };
        let e = &enr.enrollment;
        if now < e.valid_from_unix_ms || now >= e.valid_until_unix_ms {
            return refused("enrollment_not_valid_now");
        }
        if !e
            .models
            .iter()
            .any(|m| m.provider == r.provider && m.model_class == r.model_class)
        {
            return refused("model_not_enrolled");
        }
        let c = &e.episode_ceilings;
        let b = &r.bounds;
        for (f, want, max) in [
            ("max_calls", b.max_calls as u64, c.max_calls as u64),
            (
                "retry_ceiling",
                b.retry_ceiling as u64,
                c.retry_ceiling as u64,
            ),
            ("max_input_tokens", b.max_input_tokens, c.max_input_tokens),
            (
                "max_output_tokens",
                b.max_output_tokens,
                c.max_output_tokens,
            ),
            (
                "max_cost_micro_usd",
                b.max_cost_micro_usd,
                c.max_cost_micro_usd,
            ),
            ("max_wall_ms", b.max_wall_ms, c.max_wall_ms),
        ] {
            if want > max {
                return refused(format!("exceeds_enrollment_ceiling:{f}"));
            }
        }
        if now >= r.eligibility_valid_until_unix_ms {
            return refused("eligibility_expired");
        }
        let Some(wall_end) = now.checked_add(b.max_wall_ms) else {
            return refused("overflow:max_wall_ms");
        };
        let deadline = wall_end
            .min(r.deadline_unix_ms)
            .min(r.eligibility_valid_until_unix_ms)
            .min(e.valid_until_unix_ms);
        if deadline <= now {
            return refused("deadline_passed");
        }
        let scope = Scope(e.scope.clone());
        let actor = e.actor.clone();
        let available = self.core.available(&scope);
        if available < b.max_cost_micro_usd {
            let mut v = json!({
                "outcome": "exhausted",
                "exhausted": ExhaustedOutcome::new(Dimension::Milestone, 0),
                "available_micro_usd": available,
                "requested_micro_usd": b.max_cost_micro_usd,
            });
            v["send_permitted"] = json!(false);
            return v;
        }
        let rid = format!("rsv-{}", self.next_reservation);
        let decision = self.core.request_capacity(
            CapacityRequest {
                request_id: RequestId(rid.clone()),
                actor,
                action: "inference-episode".into(),
                target: r.episode_id.clone(),
                scope,
                requested_capacity: b.max_cost_micro_usd,
                eligibility_reference: r.eligibility_ref.clone(),
                eligibility_valid_until: r.eligibility_valid_until_unix_ms,
                expires_after: deadline - now,
                idempotency_key: Some(rid.clone()),
            },
            now,
        );
        let (token, core_receipt) = match decision {
            CapacityDecision::Granted {
                token_id, receipt, ..
            } => (token_id, dbg(&receipt)),
            CapacityDecision::Denied { denial_reason, .. } => {
                return refused(format!("accounting_error:core_denied:{denial_reason}"));
            }
        };
        self.next_reservation += 1;
        let res = Reservation {
            reservation_id: rid.clone(),
            admission_id: r.admission_id.clone(),
            episode_id: r.episode_id.clone(),
            condition_ref: r.condition_ref.clone(),
            ag_campaign: r.ag_campaign.clone(),
            ag_occurrence: r.ag_occurrence.clone(),
            eligibility_ref: r.eligibility_ref.clone(),
            eligibility_valid_until_unix_ms: r.eligibility_valid_until_unix_ms,
            provider: r.provider.clone(),
            model_class: r.model_class.clone(),
            bounds: b.clone(),
            start_unix_ms: now,
            deadline_unix_ms: deadline,
            status: ReservationStatus::Open,
            terminal_class: None,
            exhausted: None,
            freeze_reason: None,
            breaches: Vec::new(),
            invocation_ids: Vec::new(),
            reserved_input_tokens: 0,
            reserved_output_tokens: 0,
            consumed_micro_usd: 0,
            retired_micro_usd: None,
            closed_unix_ms: None,
            closed_from: None,
            receipt: rcpt(seq),
            core_grant_receipt: core_receipt,
            request: r.clone(),
            token,
        };
        let view = self.reservation_view(&res, "granted");
        self.episodes.insert(r.episode_id.clone(), rid.clone());
        self.reservations.insert(rid.clone(), res);
        self.key("episode", &r.episode_id);
        self.key("reservation", &rid);
        view
    }

    fn bind(&mut self, r: &BindRequest) -> Value {
        let Some(res) = self.reservations.get_mut(&r.reservation_id) else {
            return refused("unknown_reservation");
        };
        let mut changed = false;
        for (slot, want) in [
            (&mut res.ag_campaign, &r.ag_campaign),
            (&mut res.ag_occurrence, &r.ag_occurrence),
        ] {
            match slot {
                Some(have) if have != want => {
                    return conflict("binding already filled with a different value");
                }
                Some(_) => {}
                None => {
                    *slot = Some(want.clone());
                    changed = true;
                }
            }
        }
        json!({
            "outcome": if changed { "bound" } else { "already_bound" },
            "reservation_id": res.reservation_id,
            "ag_campaign": res.ag_campaign,
            "ag_occurrence": res.ag_occurrence,
        })
    }

    fn exhaust(&mut self, rid: &str, dim: Dimension) -> Value {
        let res = self.reservations.get_mut(rid).expect("reservation exists");
        let outcome = ExhaustedOutcome::new(dim, res.invocation_ids.len() as u32);
        res.status = ReservationStatus::Exhausted;
        res.terminal_class = Some(outcome.terminal_class);
        res.exhausted = Some(outcome.clone());
        json!({
            "outcome": "exhausted",
            "send_permitted": false,
            "reservation_id": rid,
            "exhausted": outcome,
        })
    }

    fn begin_call(&mut self, seq: u64, now: u64, r: &BeginCallRequest) -> Value {
        let no_send = |mut v: Value| {
            v["send_permitted"] = json!(false);
            v
        };
        let Some(res) = self.reservations.get(&r.reservation_id) else {
            return no_send(refused("unknown_reservation"));
        };
        let inv_id = format!("{}/c{}", r.reservation_id, r.call_index);
        if let Some(inv) = self.invocations.get(&inv_id) {
            // The send fence: an already-begun invocation NEVER permits a send.
            let outcome = if &inv.begin_request == r {
                "already_begun"
            } else {
                "conflict"
            };
            return json!({
                "outcome": outcome,
                "send_permitted": false,
                "invocation_id": inv_id,
                "dispatch_state": inv.dispatch_state,
                "begin_receipt": inv.begin_receipt,
            });
        }
        match res.status {
            ReservationStatus::Open => {}
            ReservationStatus::Closed => return no_send(refused("reservation_closed")),
            ReservationStatus::Concluded => return no_send(refused("reasoning_concluded")),
            ReservationStatus::Frozen => {
                return no_send(refused(format!(
                    "reservation_frozen:{}",
                    res.freeze_reason.clone().unwrap_or_default()
                )))
            }
            ReservationStatus::Exhausted => {
                return json!({
                    "outcome": "exhausted",
                    "send_permitted": false,
                    "reservation_id": res.reservation_id,
                    "exhausted": res.exhausted,
                });
            }
        }
        if res
            .invocation_ids
            .iter()
            .any(|i| self.invocations[i].settlement.is_none())
        {
            return no_send(refused("previous_call_open"));
        }
        if r.call_index as usize != res.invocation_ids.len() {
            return no_send(refused(format!(
                "call_index_out_of_order:next={}",
                res.invocation_ids.len()
            )));
        }
        let cc = &self.enrollments[&res.admission_id].enrollment.call_ceilings;
        for (f, want, max) in [
            ("max_input_tokens", r.max_input_tokens, cc.max_input_tokens),
            (
                "max_output_tokens",
                r.max_output_tokens,
                cc.max_output_tokens,
            ),
            (
                "max_cost_micro_usd",
                r.max_cost_micro_usd,
                cc.max_cost_micro_usd,
            ),
            ("max_wall_ms", r.max_wall_ms, cc.max_wall_ms),
        ] {
            if want > max {
                return no_send(refused(format!("envelope_exceeds_call_ceiling:{f}")));
            }
        }
        let b = &res.bounds;
        let rid = res.reservation_id.clone();
        let over = |have: u64, add: u64, max: u64| have.checked_add(add).is_none_or(|t| t > max);
        let dim = if now >= res.deadline_unix_ms {
            Some(Dimension::Wall)
        } else if r.call_index > b.retry_ceiling {
            Some(Dimension::Retries)
        } else if r.call_index >= b.max_calls {
            Some(Dimension::Calls)
        } else if over(
            res.reserved_input_tokens,
            r.max_input_tokens,
            b.max_input_tokens,
        ) {
            Some(Dimension::Input)
        } else if over(
            res.reserved_output_tokens,
            r.max_output_tokens,
            b.max_output_tokens,
        ) {
            Some(Dimension::Output)
        } else {
            let remaining = self
                .core
                .inspect_token(res.token)
                .map(|t| t.remaining_capacity)
                .unwrap_or(0);
            (remaining < r.max_cost_micro_usd).then_some(Dimension::Cost)
        };
        if let Some(d) = dim {
            return self.exhaust(&rid, d);
        }
        let scope = Scope(self.enrollments[&res.admission_id].enrollment.scope.clone());
        let actor = self.enrollments[&res.admission_id].enrollment.actor.clone();
        let deadline = res.deadline_unix_ms;
        let token = res.token;
        let decision = self.core.consume(
            ConsumeRequest {
                consumption_event_id: EventId(inv_id.clone()),
                token_id: token,
                actor,
                action: "inference-call".into(),
                target: inv_id.clone(),
                amount: r.max_cost_micro_usd,
                scope,
            },
            now,
        );
        let (remaining, core_receipt) = match decision {
            ConsumptionDecision::Consumed {
                remaining_capacity,
                receipt,
                ..
            } => (remaining_capacity, dbg(&receipt)),
            ConsumptionDecision::Expired { .. } => return self.exhaust(&rid, Dimension::Wall),
            ConsumptionDecision::InsufficientCapacity { .. } => {
                return self.exhaust(&rid, Dimension::Cost)
            }
            other => {
                let res = self.reservations.get_mut(&rid).expect("exists");
                res.status = ReservationStatus::Frozen;
                res.terminal_class = Some(TerminalClass::AccountingError);
                res.freeze_reason = Some("accounting_error".into());
                return no_send(refused(format!("accounting_error:{}", dbg(&other))));
            }
        };
        let call_deadline = now.saturating_add(r.max_wall_ms).min(deadline);
        let inv = Invocation {
            invocation_id: inv_id.clone(),
            reservation_id: rid.clone(),
            call_index: r.call_index,
            retry_index: r.call_index,
            request_policy_digest: r.request_policy_digest.clone(),
            max_input_tokens: r.max_input_tokens,
            max_output_tokens: r.max_output_tokens,
            max_cost_micro_usd: r.max_cost_micro_usd,
            max_wall_ms: r.max_wall_ms,
            reserved_unix_ms: now,
            call_deadline_unix_ms: call_deadline,
            dispatch_state: "send_permitted",
            begin_receipt: rcpt(seq),
            core_consume_receipt: core_receipt,
            settlement: None,
            begin_request: r.clone(),
            settle_request: None,
        };
        let res = self.reservations.get_mut(&rid).expect("exists");
        res.invocation_ids.push(inv_id.clone());
        // Checked above against the episode ceilings, so these cannot overflow.
        res.reserved_input_tokens += r.max_input_tokens;
        res.reserved_output_tokens += r.max_output_tokens;
        res.consumed_micro_usd += r.max_cost_micro_usd;
        self.invocations.insert(inv_id.clone(), inv);
        self.key("send_fence", &inv_id);
        json!({
            "outcome": "send_permitted",
            "send_permitted": true,
            "invocation_id": inv_id,
            "call_index": r.call_index,
            "retry_index": r.call_index,
            "consumed_micro_usd": r.max_cost_micro_usd,
            "token_remaining_micro_usd": remaining,
            "max_input_tokens": r.max_input_tokens,
            "max_output_tokens": r.max_output_tokens,
            "call_deadline_unix_ms": call_deadline,
            "receipt": rcpt(seq),
        })
    }

    /// Settle an open invocation. `source` is `caller` or `recovery`.
    fn settle_inner(
        &mut self,
        seq: u64,
        now: u64,
        r: &SettleRequest,
        source: &'static str,
    ) -> Result<Settlement, Value> {
        let Some(inv) = self.invocations.get(&r.invocation_id) else {
            return Err(refused("unknown_invocation"));
        };
        if inv.settlement.is_some() {
            return Err(conflict("already settled"));
        }
        let unsent = r.terminal_class == TerminalClass::CancelledUnsent;
        if unsent
            && (r.usage.is_some()
                || r.actual_cost_usd.is_some()
                || r.provider_generation_id.is_some())
        {
            return Err(refused("cancelled_unsent_with_provider_data"));
        }
        let res = &self.reservations[&inv.reservation_id];
        let ceiling = inv.max_cost_micro_usd;
        let actual_cost = match (&r.actual_cost_usd, unsent) {
            (_, true) => Some(0),
            (Some(c), false) => match usd_to_micro_ceil(c) {
                Ok(m) => Some(m),
                Err(e) => return Err(refused(e.0)),
            },
            (None, false) => None,
        };
        let (usage_source, accounted) = if unsent {
            (UsageSource::Unsent, 0)
        } else if let (Some(_), Some(cost)) = (&r.usage, actual_cost) {
            (UsageSource::ProviderReported, cost)
        } else {
            // Missing usage or cost: charge the full call ceiling; actuals stay unknown.
            (UsageSource::CeilingAssumed, ceiling)
        };
        let mut breaches = Vec::new();
        if let Some(u) = &r.usage {
            if u.input_units > inv.max_input_tokens {
                breaches.push("input_over_ceiling".to_string());
            }
            if u.output_units > inv.max_output_tokens {
                breaches.push("output_over_ceiling".to_string());
            }
        }
        if actual_cost.is_some_and(|c| c > ceiling) {
            breaches.push("cost_over_ceiling".to_string());
        }
        if r.reported_provider
            .as_ref()
            .is_some_and(|p| p != &res.provider)
        {
            breaches.push("provider_mismatch".to_string());
        }
        if r.reported_model
            .as_ref()
            .is_some_and(|m| m != &res.model_class)
        {
            breaches.push("model_mismatch".to_string());
        }
        let (slack, overage) = if accounted <= ceiling {
            (ceiling - accounted, 0)
        } else {
            (0, accounted - ceiling)
        };
        Ok(Settlement {
            invocation_id: inv.invocation_id.clone(),
            reservation_id: inv.reservation_id.clone(),
            terminal_class: r.terminal_class,
            reported_provider: r.reported_provider.clone(),
            reported_model: r.reported_model.clone(),
            provider_generation_id: r.provider_generation_id.clone(),
            usage: r.usage.clone(),
            actual_cost_micro_usd: actual_cost,
            accounted_cost_micro_usd: accounted,
            call_ceiling_micro_usd: ceiling,
            slack_micro_usd: slack,
            overage_micro_usd: overage,
            usage_source,
            breaches,
            late: now > inv.call_deadline_unix_ms,
            source,
            settled_unix_ms: now,
            receipt: rcpt(seq),
        })
    }

    fn commit_settlement(&mut self, r: &SettleRequest, s: Settlement) {
        let res = self
            .reservations
            .get_mut(&s.reservation_id)
            .expect("exists");
        if !s.breaches.is_empty() {
            res.breaches.extend(s.breaches.iter().cloned());
            res.status = ReservationStatus::Frozen;
            res.freeze_reason = Some("reconciliation_breach".into());
            res.terminal_class = Some(s.terminal_class);
        } else {
            use TerminalClass::*;
            match s.terminal_class {
                Proposal | Abstain | Escalate => {
                    res.status = ReservationStatus::Concluded;
                    res.terminal_class = Some(s.terminal_class);
                }
                Malformed | ProviderError | CancelledUnsent => {}
                Timeout | CrashUnknown | AccountingError => {
                    res.status = ReservationStatus::Frozen;
                    res.freeze_reason = Some(class_name(s.terminal_class));
                    res.terminal_class = Some(s.terminal_class);
                }
                BudgetExhausted | RetryExhausted => {
                    res.status = ReservationStatus::Exhausted;
                    res.terminal_class = Some(s.terminal_class);
                }
            }
        }
        let id = s.invocation_id.clone();
        let inv = self.invocations.get_mut(&id).expect("exists");
        inv.dispatch_state = "settled";
        inv.settlement = Some(s);
        inv.settle_request = Some(r.clone());
        self.key("settlement", &id);
    }

    fn settlement_view(outcome: &str, s: &Settlement) -> Value {
        json!({
            "outcome": outcome,
            "settlement": s,
            "escalation_required": !s.breaches.is_empty(),
        })
    }

    fn settle(&mut self, seq: u64, now: u64, r: &SettleRequest) -> Value {
        if let Some(inv) = self.invocations.get(&r.invocation_id) {
            if let Some(s) = &inv.settlement {
                if inv.settle_request.as_ref() == Some(r) {
                    return Self::settlement_view("replayed", s);
                }
                return conflict("invocation already settled with a different payload");
            }
        }
        match self.settle_inner(seq, now, r, "caller") {
            Ok(s) => {
                let v = Self::settlement_view("settled", &s);
                self.commit_settlement(r, s);
                v
            }
            Err(v) => v,
        }
    }

    fn close(&mut self, now: u64, r: &CloseRequest) -> Value {
        let Some(res) = self.reservations.get(&r.reservation_id) else {
            return refused("unknown_reservation");
        };
        let view = |res: &Reservation, outcome: &str| {
            json!({
                "outcome": outcome,
                "reservation_id": res.reservation_id,
                "closed_from": res.closed_from,
                "terminal_class": res.terminal_class,
                "consumed_micro_usd": res.consumed_micro_usd,
                "retired_micro_usd": res.retired_micro_usd,
                "closed_unix_ms": res.closed_unix_ms,
            })
        };
        if res.status == ReservationStatus::Closed {
            return view(res, "already_closed");
        }
        if res
            .invocation_ids
            .iter()
            .any(|i| self.invocations[i].settlement.is_none())
        {
            return refused("open_invocation");
        }
        let token = res.token;
        let remaining = self
            .core
            .inspect_token(token)
            .map(|t| t.remaining_capacity)
            .unwrap_or(0);
        // Retire, never recycle: the token is revoked; stock is not restored.
        self.core.revoke(token, "episode closed", now);
        let rid = r.reservation_id.clone();
        let res = self.reservations.get_mut(&rid).expect("exists");
        res.closed_from = Some(res.status);
        res.status = ReservationStatus::Closed;
        res.retired_micro_usd = Some(remaining);
        res.closed_unix_ms = Some(now);
        self.key("close", &rid);
        let res = &self.reservations[&rid];
        view(res, "closed")
    }

    fn recover(&mut self, seq: u64, now: u64, r: &RecoverRequest) -> Value {
        let open: Vec<String> = self
            .invocations
            .values()
            .filter(|i| i.settlement.is_none())
            .filter(|i| {
                r.reservation_id
                    .as_ref()
                    .is_none_or(|rid| &i.reservation_id == rid)
            })
            .map(|i| i.invocation_id.clone())
            .collect();
        for id in &r.known_unsent {
            match self.invocations.get(id) {
                None => return refused(format!("unknown_invocation:{id}")),
                Some(i) if i.settlement.is_none() && !open.contains(id) => {
                    return refused(format!("outside_recovery_scope:{id}"));
                }
                Some(i) => {
                    if let Some(s) = &i.settlement {
                        if s.terminal_class != TerminalClass::CancelledUnsent {
                            return conflict(format!("already_settled_differently:{id}"));
                        }
                    }
                }
            }
        }
        let mut settled = Vec::new();
        for id in open {
            let class = if r.known_unsent.contains(&id) {
                TerminalClass::CancelledUnsent
            } else {
                // Send status uncertain: charge the ceiling; NEVER resend.
                TerminalClass::CrashUnknown
            };
            let req = SettleRequest {
                v: PROTOCOL_VERSION,
                cmd: "settle".into(),
                invocation_id: id.clone(),
                terminal_class: class,
                reported_provider: None,
                reported_model: None,
                provider_generation_id: None,
                usage: None,
                actual_cost_usd: None,
            };
            match self.settle_inner(seq, now, &req, "recovery") {
                Ok(s) => {
                    settled.push(serde_json::to_value(&s).expect("serializable"));
                    self.commit_settlement(&req, s);
                }
                Err(v) => return v,
            }
        }
        json!({"outcome": "recovered", "settled": settled, "receipt": rcpt(seq)})
    }

    // -- read-only views ---------------------------------------------------

    fn enrollment_view(&self, e: &EnrollmentRecord) -> Value {
        let scope = Scope(e.enrollment.scope.clone());
        json!({
            "admission_id": e.enrollment.admission_id,
            "admission_ref": e.enrollment.admission_ref,
            "milestone_id": e.enrollment.milestone_id,
            "scope": e.enrollment.scope,
            "deposited_micro_usd": e.enrollment.host_allocation_micro_usd,
            "available_micro_usd": self.core.available(&scope),
            "valid_from_unix_ms": e.enrollment.valid_from_unix_ms,
            "valid_until_unix_ms": e.enrollment.valid_until_unix_ms,
            "receipt": e.receipt,
        })
    }

    fn full_reservation(&self, res: &Reservation) -> Value {
        let mut v = serde_json::to_value(res).expect("serializable");
        let invs: Vec<&Invocation> = res
            .invocation_ids
            .iter()
            .map(|i| &self.invocations[i])
            .collect();
        v["invocations"] = serde_json::to_value(invs).expect("serializable");
        if let Some(t) = self.core.inspect_token(res.token) {
            v["token"] = json!({
                "original_micro_usd": t.original_capacity,
                "remaining_micro_usd": t.remaining_capacity,
                "status": dbg(&t.status),
            });
        }
        v
    }

    /// Read-only books view, optionally restricted to one reservation.
    pub fn inspect(&self, reservation_id: Option<&str>) -> Value {
        let enrollments: Vec<Value> = self
            .enrollments
            .values()
            .map(|e| self.enrollment_view(e))
            .collect();
        let reservations: Vec<Value> = self
            .reservations
            .values()
            .filter(|r| reservation_id.is_none_or(|id| r.reservation_id == id))
            .map(|r| self.full_reservation(r))
            .collect();
        json!({"outcome": "inspected", "enrollments": enrollments, "reservations": reservations})
    }

    /// Read-only milestone reconciliation. `verdict` is PASS only with zero
    /// findings; any open invocation is a finding.
    pub fn reconcile(&self, milestone: &str) -> Value {
        let mut findings: Vec<Value> = Vec::new();
        let mut f =
            |kind: &str, detail: Value| findings.push(json!({"kind": kind, "detail": detail}));
        let enrollments: Vec<&EnrollmentRecord> = self
            .enrollments
            .values()
            .filter(|e| e.enrollment.milestone_id == milestone)
            .collect();
        if enrollments.is_empty() {
            f("unknown_milestone", json!(milestone));
        }
        let ledger = self.core.ledger();
        let mut totals = BTreeMap::<&str, u64>::new();
        let mut add = |k: &'static str, n: u64| {
            let e = totals.entry(k).or_insert(0);
            *e = e.saturating_add(n);
        };
        let mut scopes = Vec::new();
        for e in &enrollments {
            let en = &e.enrollment;
            let scope = Scope(en.scope.clone());
            let deposited: u128 = ledger
                .iter()
                .filter_map(|rec| match &rec.event {
                    Event::Deposited {
                        scope: s, amount, ..
                    } if *s == scope => Some(*amount as u128),
                    _ => None,
                })
                .sum();
            let available = self.core.available(&scope) as u128;
            let mut granted: u128 = 0;
            let mut seen_tokens = BTreeSet::new();
            for res in self
                .reservations
                .values()
                .filter(|r| r.admission_id == en.admission_id)
            {
                let Some(t) = self.core.inspect_token(res.token) else {
                    f("missing_token", json!(res.reservation_id));
                    continue;
                };
                if seen_tokens.insert(dbg(&res.token)) {
                    granted += t.original_capacity as u128;
                } else {
                    f("token_shared_by_reservations", json!(res.reservation_id));
                }
                let consumed = t.original_capacity.saturating_sub(t.remaining_capacity);
                if t.remaining_capacity > t.original_capacity {
                    f("consumed_exceeds_original", json!(res.reservation_id));
                }
                if t.original_capacity != res.bounds.max_cost_micro_usd {
                    f("grant_differs_from_reservation", json!(res.reservation_id));
                }
                let ceilings: u64 = res
                    .invocation_ids
                    .iter()
                    .map(|i| self.invocations[i].max_cost_micro_usd)
                    .sum();
                if consumed != ceilings || consumed != res.consumed_micro_usd {
                    f(
                        "token_books_mismatch",
                        json!({"reservation_id": res.reservation_id, "core_consumed": consumed, "call_ceilings": ceilings}),
                    );
                }
                add("granted_micro_usd", t.original_capacity);
                add("consumed_micro_usd", consumed);
                add("reservations", 1);
                if res.status == ReservationStatus::Closed {
                    add("retired_micro_usd", res.retired_micro_usd.unwrap_or(0));
                } else {
                    add("unclosed_reservations", 1);
                }
                if res.invocation_ids.len() > res.bounds.max_calls as usize {
                    f("call_limit_exceeded", json!(res.reservation_id));
                }
                if !res.invocation_ids.is_empty()
                    && (res.ag_campaign.is_none() || res.ag_occurrence.is_none())
                {
                    // v2 saves campaign/occurrence before reserve; a call without
                    // its AG binding cannot be joined to the governed episode.
                    f("unbound_occurrence", json!(res.reservation_id));
                }
                if !res.breaches.is_empty() {
                    f(
                        "reconciliation_breach",
                        json!({"reservation_id": res.reservation_id, "breaches": res.breaches}),
                    );
                }
                for iid in &res.invocation_ids {
                    let inv = &self.invocations[iid];
                    add("invocations", 1);
                    if inv.reservation_id != res.reservation_id {
                        f("invocation_reservation_mismatch", json!(iid));
                    }
                    if inv.retry_index > res.bounds.retry_ceiling {
                        f("retry_limit_exceeded", json!(iid));
                    }
                    if inv.reserved_unix_ms >= res.deadline_unix_ms {
                        f("begun_after_deadline", json!(iid));
                    }
                    let fences = ledger
                        .iter()
                        .filter(|rec| {
                            matches!(&rec.event, Event::Consumed { token_id, event_id, .. }
                                if *token_id == res.token && event_id == iid)
                        })
                        .count();
                    if fences != 1
                        || !self
                            .index_keys
                            .contains(&("send_fence".into(), iid.clone()))
                    {
                        f(
                            "send_fence_not_unique",
                            json!({"invocation_id": iid, "fences": fences}),
                        );
                    }
                    match &inv.settlement {
                        None => f("open_invocation", json!(iid)),
                        Some(s) => {
                            add("ceiling_micro_usd", s.call_ceiling_micro_usd);
                            add("accounted_micro_usd", s.accounted_cost_micro_usd);
                            add("slack_micro_usd", s.slack_micro_usd);
                            add("overage_micro_usd", s.overage_micro_usd);
                            if let Some(a) = s.actual_cost_micro_usd {
                                add("actual_known_micro_usd", a);
                            } else {
                                add("actual_unknown_invocations", 1);
                            }
                            if s.usage_source == UsageSource::CeilingAssumed {
                                add("ceiling_assumed_invocations", 1);
                            }
                            let lhs =
                                s.accounted_cost_micro_usd as u128 + s.slack_micro_usd as u128;
                            let rhs =
                                s.call_ceiling_micro_usd as u128 + s.overage_micro_usd as u128;
                            if lhs != rhs {
                                f("settlement_arithmetic", json!(iid));
                            }
                        }
                    }
                }
                // Every core consumption on this token must be a known invocation.
                for rec in ledger {
                    if let Event::Consumed {
                        token_id, event_id, ..
                    } = &rec.event
                    {
                        if *token_id == res.token && !res.invocation_ids.contains(event_id) {
                            f("unexplained_consumption", json!(event_id));
                        }
                    }
                }
            }
            let conserved = deposited == available + granted;
            if !conserved {
                f(
                    "conservation_violation",
                    json!({"scope": en.scope, "deposited": deposited as u64, "available": available as u64, "granted": granted as u64}),
                );
            }
            if deposited != en.host_allocation_micro_usd as u128 {
                f("deposit_differs_from_enrollment", json!(en.scope));
            }
            scopes.push(json!({
                "scope": en.scope,
                "admission_id": en.admission_id,
                "deposited_micro_usd": deposited as u64,
                "available_micro_usd": available as u64,
                "granted_micro_usd": granted as u64,
                "conserved": conserved,
            }));
        }
        let verdict = if findings.is_empty() { "PASS" } else { "FAIL" };
        json!({
            "outcome": "reconciled",
            "milestone_id": milestone,
            "verdict": verdict,
            "scopes": scopes,
            "totals": totals,
            "findings": findings,
        })
    }
}

// ---------------------------------------------------------------------------
// Durable store
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Root-owned store; process must run as root.
    Production,
    /// Store owned by the invoking user (tests, local qualification).
    Dev,
}

impl Mode {
    fn as_str(self) -> &'static str {
        match self {
            Mode::Production => "production",
            Mode::Dev => "dev",
        }
    }
}

/// Dev-only fault injection: abort the process at a durability barrier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrashPoint {
    BeforeCommit,
    AfterCommit,
}

const SCHEMA_SQL: &str = "
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;
CREATE TABLE commands (
    seq INTEGER PRIMARY KEY,
    at_unix_ms INTEGER NOT NULL,
    cmd TEXT NOT NULL,
    request TEXT NOT NULL,
    result TEXT NOT NULL
) STRICT;
CREATE TABLE idx_keys (
    kind TEXT NOT NULL,
    key TEXT NOT NULL,
    seq INTEGER NOT NULL REFERENCES commands(seq),
    PRIMARY KEY (kind, key)
) STRICT;
CREATE TRIGGER commands_append_only_u BEFORE UPDATE ON commands
    BEGIN SELECT RAISE(ABORT, 'commands is append-only'); END;
CREATE TRIGGER commands_append_only_d BEFORE DELETE ON commands
    BEGIN SELECT RAISE(ABORT, 'commands is append-only'); END;
CREATE TRIGGER idx_keys_append_only_u BEFORE UPDATE ON idx_keys
    BEGIN SELECT RAISE(ABORT, 'idx_keys is append-only'); END;
CREATE TRIGGER idx_keys_append_only_d BEFORE DELETE ON idx_keys
    BEGIN SELECT RAISE(ABORT, 'idx_keys is append-only'); END;
CREATE TRIGGER meta_immutable_u BEFORE UPDATE ON meta
    BEGIN SELECT RAISE(ABORT, 'meta is immutable'); END;
CREATE TRIGGER meta_immutable_d BEFORE DELETE ON meta
    BEGIN SELECT RAISE(ABORT, 'meta is immutable'); END;
";

/// The durable inference books: SQLite log + replayed in-memory core.
pub struct Store {
    conn: Connection,
    _lock: File,
    books: Books,
    last_seq: u64,
    poisoned: bool,
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn check_owned(
    what: &str,
    md: &std::fs::Metadata,
    uid: u32,
    want_dir: bool,
) -> Result<(), StoreError> {
    if md.file_type().is_symlink() {
        return Err(StoreError::new("symlink", format!("{what} is a symlink")));
    }
    if want_dir && !md.is_dir() {
        return Err(StoreError::new(
            "ownership",
            format!("{what} is not a directory"),
        ));
    }
    if !want_dir && !md.is_file() {
        return Err(StoreError::new(
            "ownership",
            format!("{what} is not a regular file"),
        ));
    }
    if md.uid() != uid {
        return Err(StoreError::new(
            "ownership",
            format!("{what} owned by uid {} (expected {uid})", md.uid()),
        ));
    }
    if md.mode() & 0o077 != 0 {
        return Err(StoreError::new(
            "ownership",
            format!(
                "{what} mode {:o} grants group/other access",
                md.mode() & 0o777
            ),
        ));
    }
    if !want_dir && md.nlink() != 1 {
        return Err(StoreError::new(
            "ownership",
            format!("{what} is hard-linked"),
        ));
    }
    Ok(())
}

fn check_existing_file(path: &Path, uid: u32) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(md) => check_owned(&path.display().to_string(), &md, uid, false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::new("io", format!("{}: {e}", path.display()))),
    }
}

fn io_err(what: &Path, e: std::io::Error) -> StoreError {
    StoreError::new("io", format!("{}: {e}", what.display()))
}

impl Store {
    /// Open (creating if absent) the store at `path`, take the writer lock,
    /// verify ownership/schema/identity/integrity, replay the log into a fresh
    /// core, and refuse if `now` is earlier than the last recorded time.
    pub fn open(path: &Path, mode: Mode, now: u64) -> Result<Store, StoreError> {
        let uid = match mode {
            Mode::Production => {
                if euid() != 0 {
                    return Err(StoreError::new(
                        "ownership",
                        "production mode requires running as root",
                    ));
                }
                0
            }
            Mode::Dev => euid(),
        };
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| StoreError::new("config", "store path needs a parent directory"))?;
        match std::fs::symlink_metadata(dir) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(dir)
                    .map_err(|e| io_err(dir, e))?;
            }
            Err(e) => return Err(io_err(dir, e)),
            Ok(_) => {}
        }
        let md = std::fs::symlink_metadata(dir).map_err(|e| io_err(dir, e))?;
        check_owned(&dir.display().to_string(), &md, uid, true)?;

        // Writer lock first: replay + one command happen under it.
        let lock_path = sidecar(path, ".lock");
        check_existing_file(&lock_path, uid)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&lock_path)
            .map_err(|e| io_err(&lock_path, e))?;
        check_owned(
            &lock_path.display().to_string(),
            &lock.metadata().map_err(|e| io_err(&lock_path, e))?,
            uid,
            false,
        )?;
        {
            use std::os::fd::AsRawFd;
            // SAFETY: valid open fd; flock blocks until the exclusive lock is held.
            let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) };
            if rc != 0 {
                return Err(io_err(&lock_path, std::io::Error::last_os_error()));
            }
        }

        for p in [
            path.to_path_buf(),
            sidecar(path, "-wal"),
            sidecar(path, "-shm"),
        ] {
            check_existing_file(&p, uid)?;
        }
        let existed = path.exists();
        if !existed {
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
                .map_err(|e| io_err(path, e))?;
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        let file_md = std::fs::symlink_metadata(path).map_err(|e| io_err(path, e))?;
        let canonical = std::fs::canonicalize(path).map_err(|e| io_err(path, e))?;
        let identity = format!(
            "{}|{}|{}",
            canonical.display(),
            file_md.dev(),
            file_md.ino()
        );

        // Validate (or initialize) the schema BEFORE changing journal mode, so a
        // foreign database is refused untouched. A database with no objects at
        // all (fresh, or a crash during first creation) has nothing to lose.
        let objects: i64 =
            conn.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get(0))?;
        if objects == 0 {
            let tx = conn.unchecked_transaction()?;
            tx.execute_batch(SCHEMA_SQL)?;
            for (k, v) in [
                ("schema_version", SCHEMA_VERSION.to_string()),
                ("mode", mode.as_str().to_string()),
                ("store_identity", identity.clone()),
                ("created_unix_ms", now.to_string()),
            ] {
                tx.execute(
                    "INSERT INTO meta (key, value) VALUES (?1, ?2)",
                    params![k, v],
                )?;
            }
            tx.commit()?;
            File::open(dir)
                .and_then(|d| d.sync_all())
                .map_err(|e| io_err(dir, e))?;
        } else {
            let has_meta: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='meta'",
                [],
                |r| r.get(0),
            )?;
            if has_meta != 1 {
                return Err(StoreError::new("schema", "unknown schema (no meta table)"));
            }
            let get = |k: &str| -> Result<String, StoreError> {
                conn.query_row("SELECT value FROM meta WHERE key=?1", [k], |r| r.get(0))
                    .map_err(|_| StoreError::new("schema", format!("meta key {k} missing")))
            };
            let ver = get("schema_version")?;
            if ver != SCHEMA_VERSION {
                return Err(StoreError::new(
                    "schema",
                    format!("unsupported schema version {ver}"),
                ));
            }
            let m = get("mode")?;
            if m != mode.as_str() {
                return Err(StoreError::new(
                    "mode",
                    format!(
                        "store was created in {m} mode; refusing to open in {} mode",
                        mode.as_str()
                    ),
                ));
            }
            let id = get("store_identity")?;
            if id != identity {
                return Err(StoreError::new(
                    "store_identity",
                    "store path/inode differs from creation: a copied or moved store \
                     must not become a second active allocation",
                ));
            }
        }
        let jm: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if jm != "wal" {
            return Err(StoreError::new(
                "sqlite",
                format!("journal_mode is {jm}, not wal"),
            ));
        }
        conn.execute_batch("PRAGMA synchronous=FULL;")?;
        let sync: i64 = conn.query_row("PRAGMA synchronous", [], |r| r.get(0))?;
        if sync != 2 {
            return Err(StoreError::new("sqlite", "synchronous is not FULL"));
        }
        for p in [sidecar(path, "-wal"), sidecar(path, "-shm")] {
            check_existing_file(&p, uid)?;
        }
        let qc: String = conn.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
        if qc != "ok" {
            return Err(StoreError::new("corruption", qc));
        }

        let mut store = Store {
            conn,
            _lock: lock,
            books: Books::new(),
            last_seq: 0,
            poisoned: false,
        };
        store.replay()?;
        if now < store.books.last_now {
            return Err(StoreError::new(
                "clock_rollback",
                format!("now {now} < last recorded {}", store.books.last_now),
            ));
        }
        Ok(store)
    }

    fn replay(&mut self) -> Result<(), StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, at_unix_ms, cmd, request, result FROM commands ORDER BY seq")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        let mut expect_seq = 1u64;
        let mut prev_at = 0u64;
        for row in rows {
            let (seq, at, name, req, res) = row?;
            let (seq, at) = (seq as u64, at as u64);
            if seq != expect_seq {
                return Err(StoreError::new(
                    "corruption",
                    format!("log gap at seq {seq}"),
                ));
            }
            if at < prev_at {
                return Err(StoreError::new(
                    "clock_rollback",
                    format!("log time decreases at seq {seq}"),
                ));
            }
            let cmd: Command = serde_json::from_str(&req).map_err(|e| {
                StoreError::new("schema", format!("unreadable command at seq {seq}: {e}"))
            })?;
            if cmd.name() != name {
                return Err(StoreError::new(
                    "corruption",
                    format!("cmd name mismatch at seq {seq}"),
                ));
            }
            let (out, _) = self.books.apply(seq, at, &cmd);
            // Compare serialized text, never Value-vs-String.
            let replayed = out.to_string();
            if replayed != res {
                return Err(StoreError::new(
                    "replay_mismatch",
                    format!("replayed result differs at seq {seq}"),
                ));
            }
            expect_seq += 1;
            prev_at = at;
        }
        drop(stmt);
        self.last_seq = expect_seq - 1;
        let mut stmt = self.conn.prepare("SELECT kind, key FROM idx_keys")?;
        let stored: BTreeSet<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?;
        if &stored != self.books.index_keys() {
            return Err(StoreError::new(
                "corruption",
                "unique-key index disagrees with replayed log",
            ));
        }
        Ok(())
    }

    pub fn books(&self) -> &Books {
        &self.books
    }

    /// Execute one state-changing command durably. The result is returned
    /// only after the transaction commits (synchronous=FULL).
    pub fn execute(
        &mut self,
        cmd: &Command,
        now: u64,
        crash: Option<CrashPoint>,
    ) -> Result<Value, StoreError> {
        if self.poisoned {
            return Err(StoreError::new(
                "poisoned",
                "store instance discarded after a failed commit",
            ));
        }
        if now < self.books.last_now {
            return Err(StoreError::new(
                "clock_rollback",
                format!("now {now} < last recorded {}", self.books.last_now),
            ));
        }
        let seq = self.last_seq + 1;
        self.poisoned = true; // cleared only after a successful commit
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (result, keys) = self.books.apply(seq, now, cmd);
        let req = serde_json::to_string(cmd).expect("serializable");
        tx.execute(
            "INSERT INTO commands (seq, at_unix_ms, cmd, request, result) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![seq as i64, now as i64, cmd.name(), req, result.to_string()],
        )?;
        for (kind, key) in keys {
            tx.execute(
                "INSERT INTO idx_keys (kind, key, seq) VALUES (?1, ?2, ?3)",
                params![kind, key, seq as i64],
            )?;
        }
        if crash == Some(CrashPoint::BeforeCommit) {
            std::process::abort();
        }
        tx.commit()?;
        if crash == Some(CrashPoint::AfterCommit) {
            std::process::abort();
        }
        self.last_seq = seq;
        self.poisoned = false;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn micro_usd_rounds_up() {
        assert_eq!(usd_to_micro_ceil("0").unwrap(), 0);
        assert_eq!(usd_to_micro_ceil("0.000001").unwrap(), 1);
        assert_eq!(usd_to_micro_ceil("0.0000001").unwrap(), 1);
        assert_eq!(usd_to_micro_ceil("0.00000000000000000001").unwrap(), 1);
        assert_eq!(usd_to_micro_ceil("0.0010240").unwrap(), 1024);
        assert_eq!(usd_to_micro_ceil("1.5").unwrap(), 1_500_000);
        assert_eq!(usd_to_micro_ceil("0.0005125").unwrap(), 513);
        for bad in ["", ".1", "1.", "-1", "1e-6", "0x1", " 1", "1.0.0", "NaN"] {
            assert!(usd_to_micro_ceil(bad).is_err(), "{bad}");
        }
        assert!(usd_to_micro_ceil("18446744073709551616").is_err());
        assert!(usd_to_micro_ceil("18446744073709.551616").is_err());
        assert_eq!(
            usd_to_micro_ceil("18446744073709.551615").unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn identifiers_are_narrow() {
        assert!(valid_id("rsv-1/c0"));
        assert!(valid_id("sha256:abc"));
        assert!(!valid_id(""));
        assert!(!valid_id("has space"));
        assert!(!valid_id("quote\""));
        assert!(!valid_id(&"a".repeat(129)));
    }
}
