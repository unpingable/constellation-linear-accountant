# Inference accounting (`la_inference`, protocol `v: 1`)

Durable episode and invocation accounting for one named consumer: the
Constellation remediation consumer's model decider
(`cartography/architecture/agentic-remediation-v2/DESIGN.md` §1). This is the
consumer trigger for the thaw. It is not a general scheduler, agent framework,
policy engine, distributed accountant, or refund facility.

Code: `src/inference.rs` (books, store) and `src/bin/la_inference.rs` (CLI).
Tests: `tests/la_inference_cli.rs` drives the real binary.

**Proof boundary.** The Lean model and differential oracle in `verification/`
cover the v0 in-memory core only. They do **not** cover this extension's
persistence, replay, settlement, ceilings, or recovery. Those properties are
tested, not proven. `la_cli` and its v0 protocol are unchanged.

## How it uses the v0 core

Only the existing operations are used, and there are no refunds:

| Step | Core operation | Effect |
|---|---|---|
| `enroll` | `deposit(scope, host_allocation)` | Finite stock for one enrollment scope, citing the owner admission ref |
| `reserve` | `request_capacity(episode max cost)` | The whole episode cost ceiling leaves stock as one token |
| `begin-call` | `consume(token, call max cost, event = invocation id)` | The call's worst-case cost is burned atomically with the durable send fence |
| `settle` | none | Usage is recorded; consumption is never reversed |
| `close` | `revoke(token)` | Unused episode allocation is retired, never recycled |

For each begun call: `call ceiling = accounted cost + slack` (or
`accounted = ceiling + overage` on a breach). The accounted figure is a
conservative spendability charge, not a claim about the provider invoice.

## Ceilings and units

All quantities are unsigned integers. Money is integer micro-USD. Provider
charges arrive as a plain decimal USD string (`"0.0001276"`, no float or
exponent) and are rounded **up** (`0.0001276` → `128`). Sums are checked for
overflow; an enrollment whose `max_calls × call ceiling` products overflow is
rejected.

- Enrollment `episode_ceilings` bound what a reservation may request.
  `call_ceilings` bound each call envelope.
- Episode input and output token ceilings are aggregate over begun calls'
  envelopes. Each call names its own sublimit.
- `max_calls` counts every begun call, including failed and malformed ones.
  `retry_ceiling` counts attempts after the first, so `retry_index = call_index`.
- Episode deadline is the earliest of `start + max_wall_ms`, the caller's
  `deadline_unix_ms` (the held window), the eligibility expiry, and the
  enrollment's `valid_until`. Call deadline is
  `min(begin + call max_wall_ms, episode deadline)`.

## Records

- **Enrollment**: admission id/ref, basis kind, actor, scope (one enrollment per
  scope), milestone id, host allocation, validity window, provider/model
  allowlist, and episode and call ceilings.
- **Reservation** (`rsv-N`): episode id (the idempotency key), condition ref,
  AG campaign and occurrence (nullable, fill-once), eligibility ref and expiry,
  provider and model class, bounds, start, deadline, status, terminal class,
  exhausted outcome, breaches, reserved token sums, consumed and retired micro-USD.
- **Invocation** (`rsv-N/cK`): call and retry index, request-policy digest, call
  envelope, reserved time, call deadline, dispatch state, and settlement.
- **Settlement**: terminal class, reported provider and model, generation id,
  usage units (input/output/total, optional reasoning/cache), actual cost
  (nullable), accounted cost, ceiling, slack, overage, `usage_source`
  (`provider_reported` | `ceiling_assumed` | `unsent`), breaches, `late`, and
  `source` (`caller` | `recovery`).
- **Terminal classes**: `proposal`, `abstain`, `escalate`, `malformed`,
  `provider_error`, `timeout`, `crash_unknown`, `cancelled_unsent`,
  `budget_exhausted`, `retry_exhausted`, `accounting_error`.
- **Exhausted outcome**: `dimension` (`calls`, `input`, `output`, `cost`, `wall`,
  `retries`, `milestone`), `terminal_class` (`retry_exhausted` for `retries`,
  otherwise `budget_exhausted`), `attempts_used`, `reasoning_stopped: true`, and
  `escalation_required: true`. It never means fixed.

Reservation status after a settlement: `proposal`, `abstain`, or `escalate` make
it `concluded` (no further call). `malformed`, `provider_error`, or
`cancelled_unsent` leave it `open`, so a retry is possible within the ceilings.
`timeout`, `crash_unknown`, or `accounting_error` make it `frozen`, because an
uncertain send is never repeated. Any breach makes it `frozen` with reason
`reconciliation_breach`. Exhaustion is sticky.

## CLI

```text
la_inference [--store PATH] [--dev] enroll --file OWNER_FILE
la_inference [--store PATH] [--dev] reserve|bind-occurrence|begin-call|settle|close|recover|inspect  < request.json
la_inference [--store PATH] [--dev] reconcile --milestone ID
la_inference version
```

- Each invocation takes one JSON request on stdin and writes one JSON line to
  stdout: `{"v":1,"cmd":...,"result":{"outcome":...}}`. Errors are written as
  `{"v":1,"error":{"kind","message"}}`.
- Every request carries `"v":1` and a `"cmd"` equal to the argv command. Unknown
  fields, duplicate keys, wrong types, and requests over 64 KiB are protocol
  errors. Every string is an opaque identifier
  (`[A-Za-z0-9._:/@+=-]{1,128}`).
- Exit status: 0 for any result (refusal, conflict, and exhaustion included), 1
  for a reconcile verdict of `FAIL`, 2 for a usage or protocol error, and 3 for a
  storage, integrity, ownership, or clock error. A protocol error never opens or
  writes the store.
- `enroll` is privileged. It reads only the owner file (no symlink; root-owned
  and not group/other-writable in production) and is not reachable through
  stdin. It is idempotent by `admission_id`. A different payload, or a second
  enrollment for the same scope, is a `conflict`.
- `reserve` is idempotent by `episode_id`: an identical request (or one whose
  AG fields equal the later-bound values) returns `replayed` with the original
  reservation. A different payload is a `conflict`. Refusals and `exhausted`
  results are logged but not bound to the key.
- `bind-occurrence` fills null AG campaign/occurrence exactly once. The same
  values give `already_bound`; different values give a `conflict`.
- `begin-call` returns `send_permitted: true` exactly once per invocation id. A
  replay returns `already_begun` (or `conflict` if the envelope differs) with
  `send_permitted: false`. It also refuses while a previous call is unsettled or
  when the call index is out of order.
- `settle`: an identical replay returns `replayed` with the original receipt. A
  different payload is a `conflict`. If usage or cost is missing, the call is
  charged at its full ceiling with `usage_source: ceiling_assumed` and the
  unknown actuals stay null; actual zero is never invented. Overage above any
  call ceiling, or a provider/model mismatch, is kept as reported (never
  clamped), sets `escalation_required: true`, and freezes further calls.
- `close` refuses while a call is open. Otherwise it revokes the token and
  reports `retired_micro_usd`; it is idempotent.
- `inspect` (optional `reservation_id`) and `reconcile` are read-only.

### Example (real output, dev store, fixed clock)

Here `enroll.json` holds the DESIGN envelope: 1,000,000 micro-USD host
allocation, 2 calls, retry ceiling 1, 8,192/512 episode tokens, 4,096/256 call
tokens, 2,500 micro-USD per call, and 5,000 micro-USD per episode.

```text
$ la_inference --dev --store books/inference.sqlite enroll --file enroll.json
{"cmd":"enroll","result":{"admission_id":"adm-v2-vm","available_micro_usd":1000000,"core_receipt":"ReceiptId(1)","deposited_micro_usd":1000000,"outcome":"enrolled","receipt":"lai-1","scope":"agentic-v2/vm"},"v":1}

$ echo '{"v":1,"cmd":"reserve","admission_id":"adm-v2-vm","episode_id":"ep-9f2c","condition_ref":"cond:attention-canary-down","ag_campaign":"camp-41","ag_occurrence":"occ-41-1","eligibility_ref":"standing:canary-start","eligibility_valid_until_unix_ms":1790000300000,"provider":"openrouter/google-vertex","model_class":"google/gemini-2.5-flash-lite","bounds":{"max_calls":2,"retry_ceiling":1,"max_input_tokens":8192,"max_output_tokens":512,"max_cost_micro_usd":5000,"max_wall_ms":35000},"deadline_unix_ms":1790000090000}' | la_inference --dev --store books/inference.sqlite reserve
{"cmd":"reserve","result":{"ag_campaign":"camp-41","ag_occurrence":"occ-41-1","deadline_unix_ms":1790000035000,"episode_id":"ep-9f2c","granted_micro_usd":5000,"outcome":"granted","receipt":"lai-2","reservation_id":"rsv-1","start_unix_ms":1790000000000,"status":"open"},"v":1}

$ echo '{"v":1,"cmd":"begin-call","reservation_id":"rsv-1","call_index":0,"request_policy_digest":"sha256:5d1c","max_input_tokens":4096,"max_output_tokens":256,"max_cost_micro_usd":2500,"max_wall_ms":15000}' | la_inference --dev --store books/inference.sqlite begin-call
{"cmd":"begin-call","result":{"call_deadline_unix_ms":1790000015000,"call_index":0,"consumed_micro_usd":2500,"invocation_id":"rsv-1/c0","max_input_tokens":4096,"max_output_tokens":256,"outcome":"send_permitted","receipt":"lai-3","retry_index":0,"send_permitted":true,"token_remaining_micro_usd":2500},"v":1}

$ # same request again
{"cmd":"begin-call","result":{"begin_receipt":"lai-3","dispatch_state":"send_permitted","invocation_id":"rsv-1/c0","outcome":"already_begun","send_permitted":false},"v":1}

$ echo '{"v":1,"cmd":"settle","invocation_id":"rsv-1/c0","terminal_class":"malformed"}' | la_inference --dev --store books/inference.sqlite settle
{"cmd":"settle","result":{"escalation_required":false,"outcome":"settled","settlement":{"accounted_cost_micro_usd":2500,"actual_cost_micro_usd":null,"breaches":[],"call_ceiling_micro_usd":2500,"invocation_id":"rsv-1/c0","late":false,"overage_micro_usd":0,"provider_generation_id":null,"receipt":"lai-5","reported_model":null,"reported_provider":null,"reservation_id":"rsv-1","settled_unix_ms":1790000000000,"slack_micro_usd":0,"source":"caller","terminal_class":"malformed","usage":null,"usage_source":"ceiling_assumed"}},"v":1}

$ # begin-call index 1 → send_permitted (token_remaining 0); then:
$ echo '{"v":1,"cmd":"settle","invocation_id":"rsv-1/c1","terminal_class":"proposal","reported_provider":"openrouter/google-vertex","reported_model":"google/gemini-2.5-flash-lite","provider_generation_id":"gen-1790000004-abc","usage":{"input_units":1180,"output_units":24,"total_units":1204},"actual_cost_usd":"0.0001276"}' | la_inference --dev --store books/inference.sqlite settle
{"cmd":"settle","result":{"escalation_required":false,"outcome":"settled","settlement":{"accounted_cost_micro_usd":128,"actual_cost_micro_usd":128,"breaches":[],"call_ceiling_micro_usd":2500,...,"slack_micro_usd":2372,"terminal_class":"proposal","usage_source":"provider_reported"}},"v":1}

$ echo '{"v":1,"cmd":"settle","invocation_id":"rsv-1/c1","terminal_class":"abstain"}' | la_inference --dev --store books/inference.sqlite settle
{"cmd":"settle","result":{"outcome":"conflict","reason":"invocation already settled with a different payload"},"v":1}

$ # begin-call index 2
{"cmd":"begin-call","result":{"outcome":"refused","reason":"reasoning_concluded","send_permitted":false},"v":1}

$ echo '{"v":1,"cmd":"close","reservation_id":"rsv-1"}' | la_inference --dev --store books/inference.sqlite close
{"cmd":"close","result":{"closed_from":"concluded","closed_unix_ms":1790000000000,"consumed_micro_usd":5000,"outcome":"closed","reservation_id":"rsv-1","retired_micro_usd":0,"terminal_class":"proposal"},"v":1}

$ la_inference --dev --store books/inference.sqlite reconcile --milestone agentic-remediation-v2
{"cmd":"reconcile","result":{"findings":[],"milestone_id":"agentic-remediation-v2","outcome":"reconciled","scopes":[{"admission_id":"adm-v2-vm","available_micro_usd":995000,"conserved":true,"deposited_micro_usd":1000000,"granted_micro_usd":5000,"scope":"agentic-v2/vm"}],"totals":{"accounted_micro_usd":2628,"actual_known_micro_usd":128,"actual_unknown_invocations":1,"ceiling_assumed_invocations":1,"ceiling_micro_usd":5000,"consumed_micro_usd":5000,"granted_micro_usd":5000,"invocations":2,"overage_micro_usd":0,"reservations":1,"retired_micro_usd":0,"slack_micro_usd":2372},"verdict":"PASS"},"v":1}

$ echo '{"v":1,"cmd":"settle","invocation_id":"rsv-1/c1","terminal_class":"proposal","response":"I fixed it"}' | la_inference --dev --store books/inference.sqlite settle   # exit 2
{"error":{"kind":"protocol","message":"invalid request JSON: unknown field `response`, expected one of `v`, `cmd`, `invocation_id`, `terminal_class`, `reported_provider`, `reported_model`, `provider_generation_id`, `usage`, `actual_cost_usd` at line 1 column 87"},"v":1}
```

## Reconcile

`reconcile --milestone ID` covers every enrollment with that milestone. It
returns `PASS` only when there are no findings. The checks are:

- Stock conservation per scope: `deposited = available + Σ original grants`
  (unique tokens). The deposit must equal the enrolled allocation.
- Per token, `consumed ≤ original`, the grant equals the reservation's cost
  ceiling, and consumed equals the sum of begun call ceilings and the books.
- Every invocation belongs to exactly one reservation and has exactly one
  terminal settlement. **Any open invocation is a finding (`open_invocation`).**
- Each send fence is unique: exactly one core `Consumed` event per invocation
  id, with no unexplained consumption on the token.
- Call and retry limits are respected, no call began after the deadline, and
  every reservation with calls has its AG binding filled.
- Per settlement, `accounted + slack = ceiling + overage`. Any breach is a
  finding.
- Totals cover granted, consumed, retired, ceiling, accounted, known actual,
  unknown-actual and ceiling-assumed counts, slack, and overage.

Each host has its own store and enrollment. Summing books across hosts at
milestone acceptance is the operator's join over each host's reconcile output.

## No-storage rule

No prompt, response, narration, reasoning trace, HTTP body, or credential enters
LA. This is enforced structurally:

- No request type has a field for such content, and every request denies
  unknown fields.
- Every string field must be an opaque identifier with no spaces, quotes, or
  braces, at most 128 bytes, so prose cannot pass through an id field.
- The store persists the re-serialized typed request, never the raw stdin
  bytes. Protocol errors write nothing.
- The CLI never reads provider credentials and does not log its environment.

`no_prompt_response_narration_or_credential_is_stored` injects marker strings in
several ways: unknown fields (`prompt`, `response`, `narration`, `authorization`,
`usage.reasoning_trace`), prose inside identifier fields, a model-shaped answer
sent as a request, and the `OPENROUTER_API_KEY` environment variable. It then
scans every file in the store directory and requires that no marker appears.

## Durability

- **Path**: `--store PATH`, default `/var/lib/linear-accountant/inference.sqlite`.
  If the directory is missing it is created `0700`. Files (database, WAL, SHM,
  `.lock`) are `0600`, with umask `077`.
- **Ownership**: production mode (the default) requires euid 0 and root-owned
  files. `--dev` requires the invoking user's ownership. Symlinks, hard links,
  wrong owners, and group/other permission bits are refused. A store records its
  mode at creation and refuses to open in the other mode.
- **Writer lock**: an exclusive `flock` on `<db>.lock` is held while a process
  replays and executes its one command. SQLite runs in WAL mode with
  `synchronous=FULL` and `BEGIN IMMEDIATE` transactions. A result, grant, send
  permission, or settlement receipt is printed only after commit.
- **Append-only log**: the `commands` table holds `(seq, LA-clock ms, cmd,
  typed request, result)`. The `idx_keys` table holds unique keys for
  enrollment, scope, episode, reservation, send fence, settlement, and close.
  Triggers forbid UPDATE and DELETE on both tables and on `meta`.
- **Replay on startup**: every process rebuilds a fresh core by re-applying the
  log at the recorded times, so logical ticks, token handles, and receipt ids
  are reproduced exactly. It recomputes every result and refuses to start
  (`replay_mismatch`) on any byte difference. It also requires seq contiguity
  and checks that the key index equals the replayed key set. The v0 receipts
  alone are never used for reconstruction.
- **Refusals at open**: an unknown schema version, a foreign database
  (validated before any journal-mode change), `quick_check` corruption, a
  changed path or inode (`store_identity`, so a copied database cannot become a
  second active allocation), and an LA clock earlier than the last recorded
  command (`clock_rollback`). An empty file left by a crash during first
  creation is initialized.
- **Time**: LA's own wall clock in Unix ms is the core `Tick`. The library takes
  `now` as a parameter, and only the binary reads the clock.
- **Dev-only hooks**: `LA_INFERENCE_DEV_NOW_MS` sets the clock and
  `LA_INFERENCE_DEV_CRASH=before_commit|after_commit` aborts at a barrier. Both
  are ignored without `--dev`, and a dev store cannot be opened in production
  mode.

## Recovery

`la_inference` is one process per request, so an open invocation may belong to
a live caller. LA therefore never terminates orphans implicitly. The consumer
runs `recover` once on its own startup, before any new work:

```json
{"v":1,"cmd":"recover","reservation_id":null,"known_unsent":["rsv-3/c1"]}
```

Each open invocation in scope is settled by recovery:

- **Known unsent**: listed in `known_unsent` because the consumer's own durable
  state proves no request left. It becomes `cancelled_unsent` with zero actual
  and zero accounted cost. The consumed allocation stays retired, and a later
  call is a new invocation.
- **Uncertain**: every other open invocation becomes `crash_unknown` at its full
  ceiling with `usage_source: ceiling_assumed`, and the episode is frozen.

Recovery never resends, and the original `begin-call` keeps returning
`already_begun` with `send_permitted: false`. A later caller `settle` for a
recovered invocation is a `conflict`. Recovery is idempotent.

Crash barriers covered by tests:

- A crash before commit leaves nothing; a retry gets the first result.
- A crash after commit replays: `reserve` gives `replayed`, `begin-call` gives
  `already_begun` with no send, `settle` returns the original receipt, and
  `close` gives `already_closed`.
- At the consumer barriers (after reserve, after begin-call before send, after
  send before settle, after settle), recovery reaches the terminal state above.

## Not implemented (declared limits)

- Late provider-data audit corrections linked to a terminal receipt. A late
  settle after recovery is refused as a `conflict`, and capacity is never
  restored.
- Cross-host aggregation. Each host's store reconciles alone.
- Root is trusted. Ownership checks are discipline, not isolation from root. An
  in-place overwrite of the database that keeps its inode and holds an
  internally consistent log is not detected.
- Provider compliance with the requested ceilings is an external premise. LA
  records overage truthfully but cannot prevent it.
