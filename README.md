# Linear Accountant

Public repository name: **Constellation Linear Accountant**
(`constellation-linear-accountant`). The crate and transport binary retain their
current names and interfaces. See [HOWTO.md](HOWTO.md) for a bounded local check.
Public source: <https://github.com/unpingable/constellation-linear-accountant>.

Staying within a spending allowance does not necessarily leave enough capacity
for mandatory obligations. The [accounting / residual-viability bridge](docs/working/ACCOUNTING-VIABILITY-BRIDGE.md)
explains the bounded formal relationship and the reserve-admission behavior
that is **not** implemented by the current stack.

> **Status (2026-07-26): the Linear-Accounting / Spendability office —
> frozen reference boundary, separately instantiated, not part of the
> current operational vertical.** This repository owns the jurisdiction "is
> there conserved capacity, and is it burned exactly once?" — with the
> conservation core machine-checked in Lean (`verification/`). It has
> **never been absorbed**: Docket's reservations/consumption/settlement and
> Constellation AG's one-use decision burn and Docket's local custody
> consumption are domain-local exactly-once disciplines,
> not conservation accounting; unique here remain conservation over a
> finite stock, mint≠burn capability separation (`SpendCapability`), and
> replay-as-noop. Consumers are the classic lineage (AG classic `la_cli`
> gate; transition-kernel Stage 3 chain). The freeze policy below stands:
> new slices land only on a real consumer trigger.

A reference boundary for the **Spendability Authority** role (mnemonic: *Scrooge*) — a
small, conserved core that fences exactly one failure class: turning valid context into
reusable capacity.

The load-bearing distinction:

> **Validity is contractible; spendability is linear.**
> `valid(x) ∧ valid(x) ≡ valid(x)`, but `[A] ⊬ A ⊗ A`.

Validation may mint *eligibility*. Only the accountant may mint or consume *capacity*.
Eligibility is a request, not payment. An agent holding a perfectly good warrant still
cannot spend a budget that is already gone.

This is **v0** — an in-memory, non-production reference crate. It exists so other tools
in the constellation can conform to a callable contract instead of each re-implementing
spend accounting (and re-introducing double-spend). It is **frozen as a reference
boundary**; see [Status](#status).

## What it does

- **Conserves spendability.** Finite stock per scope; grants draw it down, consumption
  draws the token down. Two linear boundaries: stock→token at grant, token→effect at
  consume.
- **Enforces exactly-once consumption.** Each consumption event id consumes at most
  once; replays are refused, not silently re-run.
- **Records everything.** An append-only ledger of bodied receipts — including the
  custodial `deposit` that mints stock — so no spend (or refusal) is silent.
- **Lets a witness testify.** A read-only witness reads the ledger and confirms no
  double-spend occurred. It has no path to allocate or consume.

## What this is not

- **Not general agent safety.** "Is this wise / correct / acceptable?" stays with the
  semantic governor, the admission gate, and the witness. This is a turnstile, not a
  cathedral.
- **Not authorization, not a policy engine.** It does not judge capability and does not
  set budgets. It enforces budgets others set. *Budgets are set by custody, spent by
  accounting; agents author neither.*
- **Not persuadable.** It accepts only sealed references it can check mechanically —
  opaque token ids, event ids, idempotency keys. It never accepts a model-generated
  summary as authority. The model can complain in twelve paragraphs; it still returns
  `AlreadyConsumed`.

## Invariants

1. **No minting by the persuadable.** Only the accountant issues or consumes capacity;
   the token type has no public constructor.
2. **Eligibility is contractible; capacity is linear.** Citing the same valid warrant
   forever cannot refill a spent budget.
3. **Custody must not be silent.** Every event, including deposits, is appended to a
   retrievable ledger with a body.

## Quick start

```bash
cargo test        # 17 tests: v0 boundary, file-write workload, contention workload
cargo clippy --all-targets -- -D warnings
```

The crate is primarily a library others call. As of the first consumer trigger
(Agent Governor bootstrap-lab effect gate, 2026-06-16) there is also one thin
transport binary, `la_cli` — a stdin/stdout line protocol over the existing
decisions, adding **no policy** (see [`docs/LA_CLI_PROTOCOL.md`](docs/LA_CLI_PROTOCOL.md)).
The library remains the boundary; the binary only lets an out-of-process consumer
reach it.

## Architecture

```
            eligibility (contractible)            capacity (linear)
  proposer ───────────────────────────►  ACCOUNTANT  ─────────────────►  effect
   (no token vocabulary)            request_capacity │ consume        (only a Consumed
                                                     │                  verdict crosses)
                                                     ▼
                                        append-only ledger ──► witness (read-only,
                                         (bodied receipts)      testifies; cannot spend)
```

- `src/lib.rs` — the conserved core: `request_capacity` / `consume` / `inspect_token` /
  `revoke` / `deposit`, the append-only `ledger`, and the read-only `witness`.
- `tests/v0_boundary.rs` — the boundary properties (eligibility ≠ capacity, exactly-once,
  receipts-are-not-tokens, freshness, scope, revocation, replay).
- `tests/file_write_workload.rs` — WL-001: a real `std::fs::write` gated by the accountant.
- `tests/contention_workload.rs` — WL-002: concurrent consumers race one token through a
  serialization point; exactly one effect crosses.

Design docs live under [`docs/`](docs/) — start at [`docs/README.md`](docs/README.md).
The authoritative behavior spec is
[`docs/architecture/V0_BOUNDARY.md`](docs/architecture/V0_BOUNDARY.md); the role and its
place in the constellation are in [`docs/architecture/ROLE.md`](docs/architecture/ROLE.md).

## Status

**Frozen as a reference boundary.** Two workloads have been bound and both pass (WL-001
file-write; WL-002 contention). No further crate slices land without a *consumer
trigger* — a real agent stack wanting `consume()` at its tool-call dispatcher. Candidate
surfaces (deployment shim, external custody anchor, request-keyed testimony) are recorded
under [`docs/working/`](docs/working/) but not built. See
[`docs/working/specimens/workload-specimens.md`](docs/working/specimens/workload-specimens.md).

The freeze has one carve-out — a **refusal-only preflight door** (`preflight::preflight_consume`):
a consumer may knock and receive a structured *not thawed* refusal; it mutates nothing.
*Doors are allowed; hidden rooms are not* (boundary doc §2d). The arithmetic core is also
machine-checked: a Lean model + Rust differential test (`verification/`) prove and
cross-check the conservation identity and replay-refusal — hardening, not thawing.

## License

Licensed under [Apache-2.0](LICENSE).
