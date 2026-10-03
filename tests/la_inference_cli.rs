// SPDX-License-Identifier: Apache-2.0
//! Drives the real `la_inference` binary against dev-mode stores.
#![cfg(feature = "inference")]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_la_inference");
const MILESTONE: &str = "agentic-remediation-v2";
const PROVIDER: &str = "openrouter/google-vertex";
const MODEL: &str = "google/gemini-2.5-flash-lite";
const T0: u64 = 1_000_000;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Env {
    root: PathBuf,
    store: PathBuf,
    now: std::cell::Cell<u64>,
}

struct Out {
    code: i32,
    v: Value,
}

impl Out {
    fn r(&self) -> &Value {
        &self.v["result"]
    }
    fn outcome(&self) -> &str {
        self.v["result"]["outcome"].as_str().unwrap_or("<none>")
    }
}

impl Env {
    fn new(name: &str) -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("la-inf-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = root.join("books").join("inference.sqlite");
        Env {
            root,
            store,
            now: std::cell::Cell::new(T0),
        }
    }

    fn advance(&self, ms: u64) {
        self.now.set(self.now.get() + ms);
    }

    fn exec(&self, args: &[&str], stdin: &[u8], envs: &[(&str, String)]) -> Out {
        let mut c = Command::new(BIN);
        c.arg("--store").arg(&self.store).arg("--dev").args(args);
        c.env("LA_INFERENCE_DEV_NOW_MS", self.now.get().to_string());
        c.env_remove("LA_INFERENCE_DEV_CRASH");
        for (k, v) in envs {
            c.env(k, v);
        }
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        // The process may exit before reading stdin (argv errors); ignore EPIPE.
        let _ = child.stdin.take().unwrap().write_all(stdin);
        let o = child.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&o.stdout);
        let v = serde_json::from_str(text.trim()).unwrap_or(Value::Null);
        Out {
            code: o.status.code().unwrap_or(-1),
            v,
        }
    }

    fn run(&self, cmd: &str, req: Value) -> Out {
        self.exec(&[cmd], req.to_string().as_bytes(), &[])
    }

    fn enroll(&self, e: &Value) -> Out {
        let p = self.root.join(format!(
            "enroll-{}.json",
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::write(&p, e.to_string()).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        self.exec(&["enroll", "--file", p.to_str().unwrap()], b"", &[])
    }

    fn reconcile(&self) -> Out {
        self.exec(&["reconcile", "--milestone", MILESTONE], b"", &[])
    }

    fn inspect(&self, rid: Option<&str>) -> Out {
        self.run(
            "inspect",
            json!({"v":1,"cmd":"inspect","reservation_id":rid}),
        )
    }
}

fn enrollment(allocation: u64) -> Value {
    json!({
        "v": 1, "cmd": "enroll",
        "admission_id": "adm-vm-1",
        "admission_ref": "owner-grant:2026-10-03",
        "basis_kind": "owner_grant",
        "actor": "constellation-remediation-consumer",
        "scope": "agentic-v2/vm",
        "milestone_id": MILESTONE,
        "host_allocation_micro_usd": allocation,
        "valid_from_unix_ms": 0,
        "valid_until_unix_ms": 9_000_000_000_000u64,
        "models": [{"provider": PROVIDER, "model_class": MODEL}],
        "episode_ceilings": {"max_calls": 3, "retry_ceiling": 2, "max_input_tokens": 8192,
            "max_output_tokens": 512, "max_cost_micro_usd": 7500, "max_wall_ms": 35000},
        "call_ceilings": {"max_input_tokens": 4096, "max_output_tokens": 256,
            "max_cost_micro_usd": 2500, "max_wall_ms": 15000}
    })
}

fn bounds() -> Value {
    json!({"max_calls": 2, "retry_ceiling": 1, "max_input_tokens": 8192,
        "max_output_tokens": 512, "max_cost_micro_usd": 5000, "max_wall_ms": 35000})
}

fn reserve_req(episode: &str, b: Value) -> Value {
    json!({
        "v": 1, "cmd": "reserve",
        "admission_id": "adm-vm-1",
        "episode_id": episode,
        "condition_ref": "cond:attention-canary-down",
        "ag_campaign": "camp-1", "ag_occurrence": "occ-1",
        "eligibility_ref": "standing:canary",
        "eligibility_valid_until_unix_ms": T0 + 3_600_000,
        "provider": PROVIDER, "model_class": MODEL,
        "bounds": b,
        "deadline_unix_ms": T0 + 3_600_000
    })
}

fn begin_req(rid: &str, idx: u32) -> Value {
    json!({"v":1,"cmd":"begin-call","reservation_id":rid,"call_index":idx,
        "request_policy_digest":"sha256:0f1e","max_input_tokens":4096,
        "max_output_tokens":256,"max_cost_micro_usd":2500,"max_wall_ms":15000})
}

fn settle_req(inv: &str, class: &str) -> Value {
    json!({"v":1,"cmd":"settle","invocation_id":inv,"terminal_class":class,
        "reported_provider":PROVIDER,"reported_model":MODEL,
        "provider_generation_id":"gen-1",
        "usage":{"input_units":900,"output_units":20,"total_units":920},
        "actual_cost_usd":"0.000098"})
}

fn bare_settle(inv: &str, class: &str) -> Value {
    json!({"v":1,"cmd":"settle","invocation_id":inv,"terminal_class":class})
}

/// Enrolled store with one granted reservation `rsv-1`.
fn setup(name: &str, b: Value) -> Env {
    let env = Env::new(name);
    assert_eq!(env.enroll(&enrollment(1_000_000)).outcome(), "enrolled");
    let o = env.run("reserve", reserve_req("ep-1", b));
    assert_eq!(o.outcome(), "granted", "{:?}", o.v);
    assert_eq!(o.r()["reservation_id"], "rsv-1");
    env
}

fn begin(env: &Env, idx: u32) -> Out {
    env.run("begin-call", begin_req("rsv-1", idx))
}

fn assert_exhausted(o: &Out, dim: &str, class: &str) {
    assert_eq!(o.outcome(), "exhausted", "{:?}", o.v);
    assert_eq!(o.r()["send_permitted"], false);
    let e = &o.r()["exhausted"];
    assert_eq!(e["dimension"], dim, "{:?}", o.v);
    assert_eq!(e["terminal_class"], class);
    assert_eq!(e["reasoning_stopped"], true);
    assert_eq!(e["escalation_required"], true);
}

// ---------------------------------------------------------------------------
// enroll
// ---------------------------------------------------------------------------

#[test]
fn enroll_is_idempotent_and_conflicts_are_refused() {
    let env = Env::new("enroll");
    let o = env.enroll(&enrollment(1_000_000));
    assert_eq!((o.code, o.outcome()), (0, "enrolled"));
    assert_eq!(o.r()["available_micro_usd"], 1_000_000);
    let again = env.enroll(&enrollment(1_000_000));
    assert_eq!(again.outcome(), "replayed");
    assert_eq!(again.r()["receipt"], o.r()["receipt"]);
    // Same admission id, different payload: conflict, no second deposit.
    assert_eq!(env.enroll(&enrollment(2_000_000)).outcome(), "conflict");
    // Different admission id, same scope: conflict (no cloned stock).
    let mut other = enrollment(1_000_000);
    other["admission_id"] = json!("adm-vm-2");
    assert_eq!(env.enroll(&other).outcome(), "conflict");
    let i = env.inspect(None);
    assert_eq!(i.r()["enrollments"][0]["available_micro_usd"], 1_000_000);
    assert_eq!(i.r()["enrollments"].as_array().unwrap().len(), 1);
}

#[test]
fn enroll_requires_an_owner_file() {
    let env = Env::new("enroll-file");
    // Not reachable through stdin.
    let o = env.exec(&["enroll"], enrollment(1).to_string().as_bytes(), &[]);
    assert_eq!(o.code, 2);
    let p = env.root.join("e.json");
    std::fs::write(&p, enrollment(1).to_string()).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o666)).unwrap();
    let o = env.exec(&["enroll", "--file", p.to_str().unwrap()], b"", &[]);
    assert_eq!(o.code, 2, "group/other-writable file must be refused");
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = env.root.join("link.json");
    std::os::unix::fs::symlink(&p, &link).unwrap();
    let o = env.exec(&["enroll", "--file", link.to_str().unwrap()], b"", &[]);
    assert_eq!(o.code, 2, "symlinked enrollment file must be refused");
    let o = env.exec(&["enroll", "--file", p.to_str().unwrap()], b"", &[]);
    assert_eq!(o.outcome(), "enrolled");
}

// ---------------------------------------------------------------------------
// reserve / bind-occurrence
// ---------------------------------------------------------------------------

#[test]
fn reserve_is_idempotent_by_episode_and_refuses_conflicts() {
    let env = setup("reserve", bounds());
    let again = env.run("reserve", reserve_req("ep-1", bounds()));
    assert_eq!(again.outcome(), "replayed");
    assert_eq!(again.r()["reservation_id"], "rsv-1");
    let mut changed = reserve_req("ep-1", bounds());
    changed["condition_ref"] = json!("cond:other");
    assert_eq!(env.run("reserve", changed).outcome(), "conflict");
    // Only one grant was drawn from stock.
    let i = env.inspect(None);
    assert_eq!(i.r()["enrollments"][0]["available_micro_usd"], 995_000);
    assert_eq!(i.r()["reservations"].as_array().unwrap().len(), 1);
}

#[test]
fn reserve_refuses_outside_the_enrollment() {
    let env = setup("reserve-refuse", bounds());
    let mut r = reserve_req("ep-x", bounds());
    r["admission_id"] = json!("adm-unknown");
    assert_eq!(env.run("reserve", r).r()["reason"], "unknown_admission");
    let mut r = reserve_req("ep-x", bounds());
    r["model_class"] = json!("openai/gpt-4o");
    assert_eq!(env.run("reserve", r).r()["reason"], "model_not_enrolled");
    let mut b = bounds();
    b["max_cost_micro_usd"] = json!(7501);
    assert_eq!(
        env.run("reserve", reserve_req("ep-x", b)).r()["reason"],
        "exceeds_enrollment_ceiling:max_cost_micro_usd"
    );
    let mut r = reserve_req("ep-x", bounds());
    r["eligibility_valid_until_unix_ms"] = json!(T0);
    assert_eq!(env.run("reserve", r).r()["reason"], "eligibility_expired");
    let mut r = reserve_req("ep-x", bounds());
    r["deadline_unix_ms"] = json!(T0 - 1);
    assert_eq!(env.run("reserve", r).r()["reason"], "deadline_passed");
    // The episode deadline is the earliest of wall, window, eligibility.
    let mut r = reserve_req("ep-y", bounds());
    r["deadline_unix_ms"] = json!(T0 + 10_000);
    assert_eq!(env.run("reserve", r).r()["deadline_unix_ms"], T0 + 10_000);
}

#[test]
fn bind_occurrence_fills_once() {
    let env = Env::new("bind");
    env.enroll(&enrollment(1_000_000));
    let mut r = reserve_req("ep-1", bounds());
    r["ag_campaign"] = Value::Null;
    r["ag_occurrence"] = Value::Null;
    assert_eq!(env.run("reserve", r.clone()).outcome(), "granted");
    let bind = |c: &str, o: &str| {
        env.run(
            "bind-occurrence",
            json!({"v":1,"cmd":"bind-occurrence","reservation_id":"rsv-1","ag_campaign":c,"ag_occurrence":o}),
        )
    };
    assert_eq!(bind("camp-1", "occ-1").outcome(), "bound");
    assert_eq!(bind("camp-1", "occ-1").outcome(), "already_bound");
    assert_eq!(bind("camp-2", "occ-1").outcome(), "conflict");
    assert_eq!(bind("camp-1", "occ-9").outcome(), "conflict");
    // Restart re-reserve: the original null payload and the bound payload both replay.
    assert_eq!(env.run("reserve", r).outcome(), "replayed");
    assert_eq!(
        env.run("reserve", reserve_req("ep-1", bounds())).outcome(),
        "replayed"
    );
    let mut wrong = reserve_req("ep-1", bounds());
    wrong["ag_occurrence"] = json!("occ-9");
    assert_eq!(env.run("reserve", wrong).outcome(), "conflict");
    let i = env.inspect(Some("rsv-1"));
    assert_eq!(i.r()["reservations"][0]["ag_occurrence"], "occ-1");
}

#[test]
fn reconcile_flags_calls_without_an_ag_binding() {
    let env = Env::new("unbound");
    env.enroll(&enrollment(1_000_000));
    let mut r = reserve_req("ep-1", bounds());
    r["ag_occurrence"] = Value::Null;
    env.run("reserve", r);
    begin(&env, 0);
    env.run("settle", settle_req("rsv-1/c0", "abstain"));
    let rc = env.reconcile();
    assert_eq!(rc.r()["findings"][0]["kind"], "unbound_occurrence");
    env.run(
        "bind-occurrence",
        json!({"v":1,"cmd":"bind-occurrence","reservation_id":"rsv-1","ag_campaign":"camp-1","ag_occurrence":"occ-1"}),
    );
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
}

// ---------------------------------------------------------------------------
// begin-call / settle
// ---------------------------------------------------------------------------

#[test]
fn begin_call_is_a_send_fence() {
    let env = setup("fence", bounds());
    let first = begin(&env, 0);
    assert_eq!(first.outcome(), "send_permitted");
    assert_eq!(first.r()["send_permitted"], true);
    assert_eq!(first.r()["invocation_id"], "rsv-1/c0");
    assert_eq!(first.r()["token_remaining_micro_usd"], 2500);
    let again = begin(&env, 0);
    assert_eq!(again.outcome(), "already_begun");
    assert_eq!(again.r()["send_permitted"], false);
    let mut altered = begin_req("rsv-1", 0);
    altered["max_input_tokens"] = json!(10);
    let c = env.run("begin-call", altered);
    assert_eq!(
        (c.outcome(), &c.r()["send_permitted"]),
        ("conflict", &json!(false))
    );
    let next = begin(&env, 1);
    assert_eq!(next.r()["reason"], "previous_call_open");
    assert_eq!(next.r()["send_permitted"], false);
    let skip = begin(&env, 5);
    assert_eq!(skip.r()["send_permitted"], false);
    // Exactly one core consumption happened.
    let i = env.inspect(Some("rsv-1"));
    assert_eq!(
        i.r()["reservations"][0]["token"]["remaining_micro_usd"],
        2500
    );
    let mut big = begin_req("rsv-1", 1);
    big["max_cost_micro_usd"] = json!(2501);
    env.run("settle", settle_req("rsv-1/c0", "malformed"));
    let o = env.run("begin-call", big);
    assert_eq!(
        o.r()["reason"],
        "envelope_exceeds_call_ceiling:max_cost_micro_usd"
    );
    assert_eq!(
        env.run("begin-call", begin_req("rsv-1", 2)).r()["reason"],
        "call_index_out_of_order:next=1"
    );
    assert_eq!(
        env.run("begin-call", begin_req("rsv-9", 0)).r()["reason"],
        "unknown_reservation"
    );
}

#[test]
fn settle_is_idempotent_and_refuses_conflicting_replay() {
    let env = setup("settle", bounds());
    begin(&env, 0);
    let s = env.run("settle", settle_req("rsv-1/c0", "proposal"));
    assert_eq!(s.outcome(), "settled");
    let st = &s.r()["settlement"];
    assert_eq!(st["usage_source"], "provider_reported");
    assert_eq!(st["actual_cost_micro_usd"], 98);
    assert_eq!(st["accounted_cost_micro_usd"], 98);
    assert_eq!(st["slack_micro_usd"], 2402);
    env.advance(10);
    let replay = env.run("settle", settle_req("rsv-1/c0", "proposal"));
    assert_eq!(replay.outcome(), "replayed");
    assert_eq!(replay.r()["settlement"]["receipt"], st["receipt"]);
    assert_eq!(
        replay.r()["settlement"]["settled_unix_ms"],
        st["settled_unix_ms"]
    );
    let mut altered = settle_req("rsv-1/c0", "proposal");
    altered["actual_cost_usd"] = json!("0.000001");
    assert_eq!(env.run("settle", altered).outcome(), "conflict");
    assert_eq!(
        env.run("settle", settle_req("rsv-1/c0", "abstain"))
            .outcome(),
        "conflict"
    );
    assert_eq!(
        env.run("settle", settle_req("rsv-1/c7", "proposal")).r()["reason"],
        "unknown_invocation"
    );
    // A decision concludes reasoning: no further call.
    assert_eq!(begin(&env, 1).r()["reason"], "reasoning_concluded");
}

#[test]
fn missing_usage_settles_at_the_call_ceiling() {
    let env = setup("ceiling", bounds());
    begin(&env, 0);
    let s = env.run("settle", bare_settle("rsv-1/c0", "provider_error"));
    let st = &s.r()["settlement"];
    assert_eq!(st["usage_source"], "ceiling_assumed");
    assert_eq!(st["accounted_cost_micro_usd"], 2500);
    assert_eq!(st["actual_cost_micro_usd"], Value::Null);
    assert_eq!(st["usage"], Value::Null);
    assert_eq!(st["slack_micro_usd"], 0);
    // Usage without the account charge is still ceiling-assumed.
    begin(&env, 1);
    let mut req = settle_req("rsv-1/c1", "malformed");
    req["actual_cost_usd"] = Value::Null;
    let st = env.run("settle", req).r()["settlement"].clone();
    assert_eq!(st["usage_source"], "ceiling_assumed");
    assert_eq!(st["accounted_cost_micro_usd"], 2500);
    assert_eq!(st["usage"]["input_units"], 900);
    // cancelled_unsent cannot carry provider data.
    let env2 = setup("unsent-data", bounds());
    begin(&env2, 0);
    let mut bad = settle_req("rsv-1/c0", "cancelled_unsent");
    bad["reported_provider"] = Value::Null;
    assert_eq!(
        env2.run("settle", bad).r()["reason"],
        "cancelled_unsent_with_provider_data"
    );
    let ok = env2.run("settle", bare_settle("rsv-1/c0", "cancelled_unsent"));
    assert_eq!(ok.r()["settlement"]["usage_source"], "unsent");
    assert_eq!(ok.r()["settlement"]["accounted_cost_micro_usd"], 0);
    assert_eq!(ok.r()["settlement"]["actual_cost_micro_usd"], 0);
    // The allocation stays consumed: no refund.
    let i = env2.inspect(Some("rsv-1"));
    assert_eq!(
        i.r()["reservations"][0]["token"]["remaining_micro_usd"],
        2500
    );
}

#[test]
fn overage_is_recorded_truthfully_and_freezes_calls() {
    let env = setup("overage", bounds());
    begin(&env, 0);
    let mut req = settle_req("rsv-1/c0", "malformed");
    req["actual_cost_usd"] = json!("0.0031");
    req["usage"]["output_units"] = json!(300);
    let s = env.run("settle", req);
    let st = &s.r()["settlement"];
    assert_eq!(s.r()["escalation_required"], true);
    assert_eq!(st["actual_cost_micro_usd"], 3100, "never clamped");
    assert_eq!(st["accounted_cost_micro_usd"], 3100);
    assert_eq!(st["overage_micro_usd"], 600);
    assert_eq!(
        st["breaches"],
        json!(["output_over_ceiling", "cost_over_ceiling"])
    );
    let b = begin(&env, 1);
    assert_eq!(b.r()["reason"], "reservation_frozen:reconciliation_breach");
    assert_eq!(b.r()["send_permitted"], false);
    let rc = env.reconcile();
    assert_eq!((rc.code, &rc.r()["verdict"]), (1, &json!("FAIL")));
    assert!(rc.r()["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["kind"] == "reconciliation_breach"));
}

#[test]
fn reported_model_mismatch_is_a_breach() {
    let env = setup("mismatch", bounds());
    begin(&env, 0);
    let mut req = settle_req("rsv-1/c0", "proposal");
    req["reported_model"] = json!("google/gemini-2.5-pro");
    let s = env.run("settle", req);
    assert_eq!(s.r()["settlement"]["breaches"], json!(["model_mismatch"]));
    assert_eq!(env.reconcile().r()["verdict"], "FAIL");
}

// ---------------------------------------------------------------------------
// exhaustion and retry ceiling
// ---------------------------------------------------------------------------

#[test]
fn exhaustion_by_calls() {
    let mut b = bounds();
    b["max_calls"] = json!(1);
    let env = setup("ex-calls", b);
    begin(&env, 0);
    env.run("settle", settle_req("rsv-1/c0", "malformed"));
    let o = begin(&env, 1);
    assert_exhausted(&o, "calls", "budget_exhausted");
    assert_eq!(o.r()["exhausted"]["attempts_used"], 1);
    // Exhaustion is sticky.
    assert_exhausted(&begin(&env, 1), "calls", "budget_exhausted");
}

#[test]
fn exhaustion_by_retries_is_retry_exhausted_after_exactly_two_sends() {
    // The DESIGN envelope: max 2 calls, retry ceiling 1, two malformed answers.
    let env = setup("ex-retries", bounds());
    let mut sends = 0;
    for idx in 0..3 {
        let o = begin(&env, idx);
        if o.r()["send_permitted"] == true {
            sends += 1;
            env.run("settle", settle_req(&format!("rsv-1/c{idx}"), "malformed"));
        } else {
            assert_exhausted(&o, "retries", "retry_exhausted");
        }
    }
    assert_eq!(sends, 2);
    // Retries bind before calls when max_calls allows more.
    let mut b = bounds();
    b["max_calls"] = json!(3);
    b["max_cost_micro_usd"] = json!(7500);
    let env = setup("ex-retries-only", b);
    for idx in 0..2 {
        assert_eq!(begin(&env, idx).r()["send_permitted"], true);
        env.run(
            "settle",
            bare_settle(&format!("rsv-1/c{idx}"), "provider_error"),
        );
    }
    assert_exhausted(&begin(&env, 2), "retries", "retry_exhausted");
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
}

#[test]
fn exhaustion_by_input_and_output_tokens() {
    let mut b = bounds();
    b["max_input_tokens"] = json!(6000);
    let env = setup("ex-input", b);
    begin(&env, 0);
    env.run("settle", settle_req("rsv-1/c0", "malformed"));
    assert_exhausted(&begin(&env, 1), "input", "budget_exhausted");

    let mut b = bounds();
    b["max_output_tokens"] = json!(300);
    let env = setup("ex-output", b);
    begin(&env, 0);
    env.run("settle", settle_req("rsv-1/c0", "malformed"));
    assert_exhausted(&begin(&env, 1), "output", "budget_exhausted");
}

#[test]
fn exhaustion_by_cost_when_episode_is_below_one_call() {
    let mut b = bounds();
    b["max_cost_micro_usd"] = json!(2000);
    let env = setup("ex-cost", b);
    let o = begin(&env, 0);
    assert_exhausted(&o, "cost", "budget_exhausted");
    assert_eq!(o.r()["exhausted"]["attempts_used"], 0);
    let i = env.inspect(Some("rsv-1"));
    assert_eq!(
        i.r()["reservations"][0]["token"]["remaining_micro_usd"],
        2000
    );
}

#[test]
fn exhaustion_by_wall() {
    let env = setup("ex-wall", bounds());
    env.advance(35_000);
    assert_exhausted(&begin(&env, 0), "wall", "budget_exhausted");
}

#[test]
fn exhaustion_by_milestone_stock() {
    let env = Env::new("ex-milestone");
    env.enroll(&enrollment(6_000));
    assert_eq!(
        env.run("reserve", reserve_req("ep-1", bounds())).outcome(),
        "granted"
    );
    let o = env.run("reserve", reserve_req("ep-2", bounds()));
    assert_exhausted(&o, "milestone", "budget_exhausted");
    assert_eq!(o.r()["available_micro_usd"], 1000);
    // A refused reserve is not bound to the key and grants nothing.
    let i = env.inspect(None);
    assert_eq!(i.r()["reservations"].as_array().unwrap().len(), 1);
    assert_eq!(i.r()["enrollments"][0]["available_micro_usd"], 1000);
}

#[test]
fn timeout_freezes_and_late_settlement_is_flagged() {
    let env = setup("timeout", bounds());
    begin(&env, 0);
    env.advance(20_000);
    let s = env.run("settle", bare_settle("rsv-1/c0", "timeout"));
    assert_eq!(s.r()["settlement"]["late"], true);
    assert_eq!(s.r()["settlement"]["accounted_cost_micro_usd"], 2500);
    assert_eq!(begin(&env, 1).r()["reason"], "reservation_frozen:timeout");
}

// ---------------------------------------------------------------------------
// close / recover
// ---------------------------------------------------------------------------

#[test]
fn close_retires_unused_allocation() {
    let env = setup("close", bounds());
    begin(&env, 0);
    let close = || {
        env.run(
            "close",
            json!({"v":1,"cmd":"close","reservation_id":"rsv-1"}),
        )
    };
    assert_eq!(close().r()["reason"], "open_invocation");
    env.run("settle", settle_req("rsv-1/c0", "abstain"));
    let c = close();
    assert_eq!(c.outcome(), "closed");
    assert_eq!(c.r()["retired_micro_usd"], 2500);
    assert_eq!(c.r()["closed_from"], "concluded");
    env.advance(5);
    let again = close();
    assert_eq!(again.outcome(), "already_closed");
    assert_eq!(again.r()["closed_unix_ms"], c.r()["closed_unix_ms"]);
    assert_eq!(begin(&env, 1).r()["reason"], "reservation_closed");
    // Retired, not recycled: stock does not come back.
    let i = env.inspect(None);
    assert_eq!(i.r()["enrollments"][0]["available_micro_usd"], 995_000);
    assert_eq!(i.r()["reservations"][0]["token"]["status"], "Revoked");
}

#[test]
fn recover_never_resends() {
    let env = Env::new("recover");
    env.enroll(&enrollment(1_000_000));
    env.run("reserve", reserve_req("ep-1", bounds()));
    env.run("reserve", reserve_req("ep-2", bounds()));
    env.run("begin-call", begin_req("rsv-1", 0));
    env.run("begin-call", begin_req("rsv-2", 0));
    let bad = env.run(
        "recover",
        json!({"v":1,"cmd":"recover","known_unsent":["rsv-9/c0"]}),
    );
    assert_eq!(bad.outcome(), "refused");
    let rec = env.run(
        "recover",
        json!({"v":1,"cmd":"recover","known_unsent":["rsv-1/c0"]}),
    );
    assert_eq!(rec.outcome(), "recovered");
    let settled = rec.r()["settled"].as_array().unwrap();
    assert_eq!(settled.len(), 2);
    let by = |id: &str| {
        settled
            .iter()
            .find(|s| s["invocation_id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(by("rsv-1/c0")["terminal_class"], "cancelled_unsent");
    assert_eq!(by("rsv-1/c0")["accounted_cost_micro_usd"], 0);
    assert_eq!(by("rsv-2/c0")["terminal_class"], "crash_unknown");
    assert_eq!(by("rsv-2/c0")["usage_source"], "ceiling_assumed");
    assert_eq!(by("rsv-2/c0")["accounted_cost_micro_usd"], 2500);
    assert_eq!(by("rsv-2/c0")["source"], "recovery");
    // Uncertain send: the episode is frozen; the same call never re-permits.
    assert_eq!(
        env.run("begin-call", begin_req("rsv-2", 0)).r()["send_permitted"],
        false
    );
    assert_eq!(
        env.run("begin-call", begin_req("rsv-2", 1)).r()["reason"],
        "reservation_frozen:crash_unknown"
    );
    // A late caller settle cannot overwrite the recovery settlement.
    assert_eq!(
        env.run("settle", settle_req("rsv-2/c0", "proposal"))
            .outcome(),
        "conflict"
    );
    // Recovery is idempotent: nothing left open.
    let again = env.run(
        "recover",
        json!({"v":1,"cmd":"recover","known_unsent":["rsv-1/c0"]}),
    );
    assert_eq!(again.r()["settled"], json!([]));
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
}

// ---------------------------------------------------------------------------
// crash at each barrier
// ---------------------------------------------------------------------------

fn crash(env: &Env, cmd: &str, req: &Value, at: &str) -> Out {
    env.exec(
        &[cmd],
        req.to_string().as_bytes(),
        &[("LA_INFERENCE_DEV_CRASH", at.to_string())],
    )
}

#[test]
fn crash_before_commit_leaves_no_trace() {
    let env = Env::new("crash-before");
    env.enroll(&enrollment(1_000_000));
    let r = reserve_req("ep-1", bounds());
    let o = crash(&env, "reserve", &r, "before_commit");
    assert_ne!(o.code, 0);
    assert_eq!(o.v, Value::Null, "no result may be emitted before commit");
    assert_eq!(env.run("reserve", r).outcome(), "granted");
    let b = begin_req("rsv-1", 0);
    assert_ne!(crash(&env, "begin-call", &b, "before_commit").code, 0);
    assert_eq!(env.run("begin-call", b).r()["send_permitted"], true);
    let s = settle_req("rsv-1/c0", "proposal");
    assert_ne!(crash(&env, "settle", &s, "before_commit").code, 0);
    assert_eq!(env.run("settle", s).outcome(), "settled");
    let c = json!({"v":1,"cmd":"close","reservation_id":"rsv-1"});
    assert_ne!(crash(&env, "close", &c, "before_commit").code, 0);
    assert_eq!(env.run("close", c).outcome(), "closed");
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
    assert_eq!(
        env.inspect(None).r()["enrollments"][0]["available_micro_usd"],
        995_000
    );
}

#[test]
fn crash_after_commit_replays_and_never_permits_a_second_send() {
    let env = Env::new("crash-after");
    env.enroll(&enrollment(1_000_000));
    let r = reserve_req("ep-1", bounds());
    assert_ne!(crash(&env, "reserve", &r, "after_commit").code, 0);
    let o = env.run("reserve", r);
    assert_eq!(
        (o.outcome(), &o.r()["reservation_id"]),
        ("replayed", &json!("rsv-1"))
    );
    let b = begin_req("rsv-1", 0);
    let o = crash(&env, "begin-call", &b, "after_commit");
    assert_eq!(o.v, Value::Null, "the permit was never delivered");
    // The caller never saw the permit; the retry is refused a send.
    let o = env.run("begin-call", b);
    assert_eq!(
        (o.outcome(), &o.r()["send_permitted"]),
        ("already_begun", &json!(false))
    );
    // The consumer's own record says it never sent: cancelled, no cost, no resend.
    let rec = env.run(
        "recover",
        json!({"v":1,"cmd":"recover","known_unsent":["rsv-1/c0"]}),
    );
    assert_eq!(rec.r()["settled"][0]["terminal_class"], "cancelled_unsent");
    let b1 = begin_req("rsv-1", 1);
    assert_ne!(crash(&env, "begin-call", &b1, "after_commit").code, 0);
    // Send status unknown at restart: crash_unknown at ceiling.
    let rec = env.run("recover", json!({"v":1,"cmd":"recover"}));
    assert_eq!(rec.r()["settled"][0]["terminal_class"], "crash_unknown");
    assert_eq!(rec.r()["settled"][0]["accounted_cost_micro_usd"], 2500);
    assert_eq!(env.run("begin-call", b1).r()["send_permitted"], false);
    let c = json!({"v":1,"cmd":"close","reservation_id":"rsv-1"});
    assert_ne!(crash(&env, "close", &c, "after_commit").code, 0);
    assert_eq!(env.run("close", c).outcome(), "already_closed");
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
}

#[test]
fn crash_after_settle_commit_replays_the_receipt() {
    let env = setup("crash-settle", bounds());
    begin(&env, 0);
    let s = settle_req("rsv-1/c0", "proposal");
    assert_ne!(crash(&env, "settle", &s, "after_commit").code, 0);
    let o = env.run("settle", s);
    assert_eq!(o.outcome(), "replayed");
    assert_eq!(o.r()["settlement"]["receipt"], "lai-4");
}

#[test]
fn consumer_barriers_recover_without_resend() {
    // after reserve (no call yet): re-reserve the same key, nothing to recover.
    let env = setup("barrier-reserve", bounds());
    assert_eq!(
        env.run("reserve", reserve_req("ep-1", bounds())).outcome(),
        "replayed"
    );
    assert_eq!(
        env.run("recover", json!({"v":1,"cmd":"recover"})).r()["settled"],
        json!([])
    );
    // after begin-call, before send: known unsent.
    begin(&env, 0);
    let r = env.run(
        "recover",
        json!({"v":1,"cmd":"recover","reservation_id":"rsv-1","known_unsent":["rsv-1/c0"]}),
    );
    assert_eq!(r.r()["settled"][0]["terminal_class"], "cancelled_unsent");
    // after send, before settle: uncertain.
    assert_eq!(begin(&env, 1).r()["send_permitted"], true);
    let r = env.run(
        "recover",
        json!({"v":1,"cmd":"recover","reservation_id":"rsv-1"}),
    );
    assert_eq!(r.r()["settled"][0]["terminal_class"], "crash_unknown");
    // after settle: nothing open.
    let env = setup("barrier-settled", bounds());
    begin(&env, 0);
    env.run("settle", settle_req("rsv-1/c0", "proposal"));
    assert_eq!(
        env.run("recover", json!({"v":1,"cmd":"recover"})).r()["settled"],
        json!([])
    );
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
}

// ---------------------------------------------------------------------------
// reconcile and conservation
// ---------------------------------------------------------------------------

#[test]
fn reconcile_blocks_on_open_calls_and_unknown_milestone() {
    let env = setup("reconcile", bounds());
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
    begin(&env, 0);
    let rc = env.reconcile();
    assert_eq!((rc.code, &rc.r()["verdict"]), (1, &json!("FAIL")));
    assert_eq!(rc.r()["findings"][0]["kind"], "open_invocation");
    env.run("settle", bare_settle("rsv-1/c0", "provider_error"));
    let rc = env.reconcile();
    assert_eq!((rc.code, &rc.r()["verdict"]), (0, &json!("PASS")));
    assert_eq!(rc.r()["totals"]["ceiling_assumed_invocations"], 1);
    assert_eq!(rc.r()["totals"]["actual_unknown_invocations"], 1);
    let other = env.exec(&["reconcile", "--milestone", "nope"], b"", &[]);
    assert_eq!(other.code, 1);
    assert_eq!(other.r()["findings"][0]["kind"], "unknown_milestone");
}

#[test]
fn stock_is_conserved_across_many_episodes() {
    let env = Env::new("conserve");
    env.enroll(&enrollment(23_000));
    let mut granted = 0;
    for ep in 0..6 {
        let o = env.run("reserve", reserve_req(&format!("ep-{ep}"), bounds()));
        if o.outcome() == "granted" {
            granted += 1;
            let rid = o.r()["reservation_id"].as_str().unwrap().to_string();
            env.run("begin-call", begin_req(&rid, 0));
            let class = if ep % 2 == 0 { "malformed" } else { "proposal" };
            env.run("settle", settle_req(&format!("{rid}/c0"), class));
            env.run("close", json!({"v":1,"cmd":"close","reservation_id":rid}));
        } else {
            assert_exhausted(&o, "milestone", "budget_exhausted");
        }
        env.advance(1);
    }
    assert_eq!(granted, 4);
    let rc = env.reconcile();
    assert_eq!(rc.r()["verdict"], "PASS", "{}", rc.v);
    let sc = &rc.r()["scopes"][0];
    assert_eq!(sc["deposited_micro_usd"], 23_000);
    assert_eq!(sc["available_micro_usd"], 3_000);
    assert_eq!(sc["granted_micro_usd"], 20_000);
    let t = &rc.r()["totals"];
    assert_eq!(t["consumed_micro_usd"], 10_000);
    assert_eq!(t["retired_micro_usd"], 10_000);
    assert_eq!(t["accounted_micro_usd"], 4 * 98);
}

#[test]
fn concurrent_reserves_never_overgrant() {
    let env = Env::new("concurrent");
    env.enroll(&enrollment(15_000));
    let distinct: Vec<Value> = (0..8)
        .map(|i| spawn_reserve(env.store.clone(), format!("ep-{i}")))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    let granted = distinct
        .iter()
        .filter(|r| r["outcome"] == "granted")
        .count();
    let exhausted = distinct
        .iter()
        .filter(|r| r["outcome"] == "exhausted")
        .count();
    assert_eq!((granted, exhausted), (3, 5));
    // Same episode raced: exactly one reservation id, exactly one grant.
    let env2 = Env::new("concurrent-same");
    env2.enroll(&enrollment(1_000_000));
    let same: Vec<Value> = (0..6)
        .map(|_| spawn_reserve(env2.store.clone(), "ep-same".into()))
        .collect::<Vec<_>>()
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect();
    assert_eq!(same.iter().filter(|r| r["outcome"] == "granted").count(), 1);
    assert!(same.iter().all(|r| r["reservation_id"] == "rsv-1"));
    assert_eq!(
        env2.inspect(None).r()["enrollments"][0]["available_micro_usd"],
        995_000
    );
    assert_eq!(env.reconcile().r()["verdict"], "PASS");
    assert_eq!(env2.reconcile().r()["verdict"], "PASS");
}

fn spawn_reserve(store: PathBuf, episode: String) -> std::thread::JoinHandle<Value> {
    std::thread::spawn(move || {
        let mut c = Command::new(BIN)
            .arg("--store")
            .arg(&store)
            .arg("--dev")
            .arg("reserve")
            .env("LA_INFERENCE_DEV_NOW_MS", T0.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        c.stdin
            .take()
            .unwrap()
            .write_all(reserve_req(&episode, bounds()).to_string().as_bytes())
            .unwrap();
        let o = c.wait_with_output().unwrap();
        assert!(o.status.success());
        serde_json::from_slice::<Value>(&o.stdout).unwrap()["result"].clone()
    })
}

// ---------------------------------------------------------------------------
// no-storage rule
// ---------------------------------------------------------------------------

fn scan_for_markers(dir: &Path, markers: &[&str]) -> Vec<String> {
    let mut hits = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        let bytes = std::fs::read(&p).unwrap();
        for m in markers {
            if bytes.windows(m.len()).any(|w| w == m.as_bytes()) {
                hits.push(format!("{} in {}", m, p.display()));
            }
        }
    }
    hits
}

#[test]
fn no_prompt_response_narration_or_credential_is_stored() {
    let env = setup("no-storage", bounds());
    let markers = [
        "PROMPTMARKER7f3a",
        "RESPONSEMARKER9c1d",
        "NARRATIONMARKER2b8e",
        "CREDENTIALMARKER5e44",
        "REASONINGMARKER0aa1",
    ];
    let secret_env = [
        ("OPENROUTER_API_KEY", format!("sk-or-{}", markers[3])),
        ("LA_PROMPT", markers[0].to_string()),
    ];
    let run = |cmd: &str, req: Value| env.exec(&[cmd], req.to_string().as_bytes(), &secret_env);
    // Smuggling attempts: unknown fields and prose in identifier fields.
    let mut attempts = Vec::new();
    let mut r = begin_req("rsv-1", 0);
    r["prompt"] = json!(format!("system: {}", markers[0]));
    attempts.push(("begin-call", r));
    let mut r = begin_req("rsv-1", 0);
    r["request_policy_digest"] = json!(format!("ignore the resolver {}", markers[2]));
    attempts.push(("begin-call", r));
    let mut s = settle_req("rsv-1/c0", "proposal");
    s["response"] = json!(markers[1]);
    attempts.push(("settle", s));
    let mut s = settle_req("rsv-1/c0", "proposal");
    s["usage"]["reasoning_trace"] = json!(markers[4]);
    attempts.push(("settle", s));
    let mut s = settle_req("rsv-1/c0", "proposal");
    s["provider_generation_id"] = json!(format!("{{\"content\":\"{}\"}}", markers[1]));
    attempts.push(("settle", s));
    let mut s = settle_req("rsv-1/c0", "proposal");
    s["authorization"] = json!(format!("Bearer {}", markers[3]));
    attempts.push(("settle", s));
    let mut rr = reserve_req("ep-2", bounds());
    rr["condition_ref"] = json!(format!("summary: already healthy {}", markers[2]));
    attempts.push(("reserve", rr));
    let mut rr = reserve_req("ep-3", bounds());
    rr["narration"] = json!(markers[2]);
    attempts.push(("reserve", rr));
    // A model-shaped answer pasted as a whole request.
    attempts.push((
        "settle",
        json!({"decision":"start_canary","reason":"current_down","grant":markers[3]}),
    ));
    for (cmd, req) in attempts {
        let o = run(cmd, req);
        assert_eq!(
            o.code, 2,
            "smuggling attempt must be a protocol error: {}",
            o.v
        );
    }
    // The legitimate flow, under the same hostile environment.
    assert_eq!(
        run("begin-call", begin_req("rsv-1", 0)).r()["send_permitted"],
        true
    );
    assert_eq!(
        run("settle", settle_req("rsv-1/c0", "proposal")).outcome(),
        "settled"
    );
    assert_eq!(
        run(
            "close",
            json!({"v":1,"cmd":"close","reservation_id":"rsv-1"})
        )
        .outcome(),
        "closed"
    );
    let hits = scan_for_markers(env.store.parent().unwrap(), &markers);
    assert!(hits.is_empty(), "markers persisted: {hits:?}");
    // Sanity: the scanner does find what is stored.
    assert!(!scan_for_markers(env.store.parent().unwrap(), &["sha256:0f1e"]).is_empty());
}

// ---------------------------------------------------------------------------
// protocol and store hardening
// ---------------------------------------------------------------------------

#[test]
fn protocol_errors_exit_nonzero_and_write_nothing() {
    let env = Env::new("protocol");
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("reserve", b"not json".to_vec()),
        ("reserve", {
            let mut r = reserve_req("ep-1", bounds());
            r["v"] = json!(2);
            r.to_string().into_bytes()
        }),
        ("reserve", begin_req("rsv-1", 0).to_string().into_bytes()),
        ("begin-call", {
            let mut r = begin_req("rsv-1", 0);
            r["cmd"] = json!("settle");
            r.to_string().into_bytes()
        }),
        (
            "close",
            br#"{"v":1,"cmd":"close","reservation_id":"a","reservation_id":"b"}"#.to_vec(),
        ),
        ("settle", {
            let mut s = settle_req("rsv-1/c0", "proposal");
            s["actual_cost_usd"] = json!(0.0001);
            s.to_string().into_bytes()
        }),
        ("settle", {
            let mut s = settle_req("rsv-1/c0", "proposal");
            s["actual_cost_usd"] = json!("1e-4");
            s.to_string().into_bytes()
        }),
        ("settle", {
            let mut s = settle_req("rsv-1/c0", "fixed");
            s["terminal_class"] = json!("fixed");
            s.to_string().into_bytes()
        }),
        ("begin-call", {
            let mut r = begin_req("rsv-1", 0);
            r["max_cost_micro_usd"] = json!(-1);
            r.to_string().into_bytes()
        }),
        ("begin-call", {
            let mut r = begin_req("rsv-1", 0);
            r["max_wall_ms"] = json!(0);
            r.to_string().into_bytes()
        }),
        ("reserve", vec![b' '; 70_000]),
        ("frobnicate", b"{}".to_vec()),
    ];
    for (cmd, bytes) in cases {
        let o = env.exec(&[cmd], &bytes, &[]);
        assert_eq!(o.code, 2, "{cmd}: {}", o.v);
        assert!(o.v["error"].is_object());
    }
    assert!(
        !env.store.exists(),
        "protocol errors must not create the store"
    );
}

#[test]
fn production_mode_requires_root() {
    let env = Env::new("prod");
    let mut c = Command::new(BIN);
    c.arg("--store").arg(&env.store).arg("inspect");
    c.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut ch = c.spawn().unwrap();
    ch.stdin
        .take()
        .unwrap()
        .write_all(br#"{"v":1,"cmd":"inspect"}"#)
        .unwrap();
    let o = ch.wait_with_output().unwrap();
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        assert_eq!(o.status.code(), Some(3));
        let v: Value = serde_json::from_slice(&o.stdout).unwrap();
        assert_eq!(v["error"]["kind"], "ownership");
    }
}

#[test]
fn dev_store_is_not_openable_in_production_mode() {
    // A dev store refuses production mode (and production needs root anyway);
    // the clock override is never read without --dev.
    let env = setup("mode", bounds());
    let mut c = Command::new(BIN);
    c.arg("--store").arg(&env.store).arg("inspect");
    c.env("LA_INFERENCE_DEV_NOW_MS", "1");
    c.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut ch = c.spawn().unwrap();
    ch.stdin
        .take()
        .unwrap()
        .write_all(br#"{"v":1,"cmd":"inspect"}"#)
        .unwrap();
    assert_eq!(ch.wait_with_output().unwrap().status.code(), Some(3));
}

#[test]
fn store_refuses_symlinks_permissions_copies_and_clock_rollback() {
    let inspect = json!({"v":1,"cmd":"inspect"});
    // Clock rollback.
    let env = setup("clock", bounds());
    env.now.set(T0 - 1);
    let o = env.run("inspect", inspect.clone());
    assert_eq!(
        (o.code, &o.v["error"]["kind"]),
        (3, &json!("clock_rollback"))
    );
    let o = env.run("begin-call", begin_req("rsv-1", 0));
    assert_eq!(o.code, 3);
    env.now.set(T0);
    assert_eq!(env.run("inspect", inspect.clone()).code, 0);

    // Symlinked database file.
    let env = setup("symlink-file", bounds());
    let real = env.root.join("elsewhere.sqlite");
    std::fs::rename(&env.store, &real).unwrap();
    std::os::unix::fs::symlink(&real, &env.store).unwrap();
    let o = env.run("inspect", inspect.clone());
    assert_eq!((o.code, &o.v["error"]["kind"]), (3, &json!("symlink")));

    // Symlinked directory.
    let env = setup("symlink-dir", bounds());
    let books = env.store.parent().unwrap().to_path_buf();
    let moved = env.root.join("moved");
    std::fs::rename(&books, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &books).unwrap();
    let o = env.run("inspect", inspect.clone());
    assert_eq!((o.code, &o.v["error"]["kind"]), (3, &json!("symlink")));

    // Group-readable directory / database.
    let env = setup("perms", bounds());
    let books = env.store.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&books, std::fs::Permissions::from_mode(0o750)).unwrap();
    assert_eq!(
        env.run("inspect", inspect.clone()).v["error"]["kind"],
        "ownership"
    );
    std::fs::set_permissions(&books, std::fs::Permissions::from_mode(0o700)).unwrap();
    std::fs::set_permissions(&env.store, std::fs::Permissions::from_mode(0o640)).unwrap();
    assert_eq!(
        env.run("inspect", inspect.clone()).v["error"]["kind"],
        "ownership"
    );

    // A copied store must not become a second active allocation.
    let env = setup("copy", bounds());
    let copy = Env::new("copy-target");
    std::fs::create_dir_all(copy.store.parent().unwrap()).unwrap();
    std::fs::set_permissions(
        copy.store.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::copy(&env.store, &copy.store).unwrap();
    let o = copy.run("reserve", reserve_req("ep-2", bounds()));
    assert_eq!(
        (o.code, &o.v["error"]["kind"]),
        (3, &json!("store_identity"))
    );
}

#[test]
fn store_refuses_unknown_schema_and_tampered_log() {
    let inspect = json!({"v":1,"cmd":"inspect"});
    // Unknown schema version.
    let env = setup("schema", bounds());
    {
        let c = rusqlite::Connection::open(&env.store).unwrap();
        c.execute_batch(
            "DROP TRIGGER meta_immutable_u; UPDATE meta SET value='99' WHERE key='schema_version';",
        )
        .unwrap();
    }
    assert_eq!(
        env.run("inspect", inspect.clone()).v["error"]["kind"],
        "schema"
    );

    // A foreign SQLite database.
    let env = Env::new("foreign");
    std::fs::create_dir_all(env.store.parent().unwrap()).unwrap();
    std::fs::set_permissions(
        env.store.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    {
        let c = rusqlite::Connection::open(&env.store).unwrap();
        c.execute_batch("CREATE TABLE t (x);").unwrap();
    }
    std::fs::set_permissions(&env.store, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        env.run("inspect", inspect.clone()).v["error"]["kind"],
        "schema"
    );

    // An empty file (crash during first creation) initializes cleanly.
    let env = Env::new("empty");
    std::fs::create_dir_all(env.store.parent().unwrap()).unwrap();
    std::fs::set_permissions(
        env.store.parent().unwrap(),
        std::fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    std::fs::write(&env.store, b"").unwrap();
    std::fs::set_permissions(&env.store, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(env.enroll(&enrollment(1)).outcome(), "enrolled");

    // The log is append-only, and a tampered row is caught by replay.
    let env = setup("tamper", bounds());
    {
        let c = rusqlite::Connection::open(&env.store).unwrap();
        assert!(c.execute("DELETE FROM commands WHERE seq=2", []).is_err());
        assert!(c
            .execute("UPDATE commands SET result='{}' WHERE seq=2", [])
            .is_err());
        c.execute_batch(
            "DROP TRIGGER commands_append_only_u; \
             UPDATE commands SET request=replace(request, '5000', '4000') WHERE seq=2;",
        )
        .unwrap();
    }
    let o = env.run("inspect", inspect);
    assert_eq!(
        (o.code, &o.v["error"]["kind"]),
        (3, &json!("replay_mismatch"))
    );
}
