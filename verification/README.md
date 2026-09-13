# Differential oracle — Lean model + Rust cross-check

This directory hardens the **frozen** v0 boundary. It adds no surface to the crate and
opens no spend path; it raises confidence in the code that already exists.

## Why Linear Accountant, specifically

Of the constellation, this is the component where machine-checked proof earns its keep
rather than cosplaying rigor: a tiny state machine, multiset/affine semantics, a core
claim that is a linear-logic fragment, and the highest blast radius if wrong — it is the
thing that makes failure *finite*, so a bug here unmakes finiteness everywhere. So it is
worth proving the ledger conserves, and then differential-testing the implementation
against the proven model.

## The two halves

- **`Ledger.lean`** — the spend ledger modelled as a fold over an event list, with
  machine-checked theorems (zero `sorry`):
  - `conservation` — `minted = available + Σ original` for every event sequence
    (the aggregate identity);
  - `token_balance_preserved` — `original = remaining + consumed` for every live token,
    for every event sequence (the local drawdown identity: conservation holds even if a
    single token's own bookkeeping were wrong, so this is proved separately);
  - `replay_is_noop` — consuming an already-seen event id changes nothing
    (replay-refusal == no-double-consume).

  Mathlib-free: depends only on Lean 4 core.

- **`../tests/differential_oracle.rs`** — the executable twin. A faithful reference
  model (branch-for-branch with `src/lib.rs`) is run against the real
  `InMemoryAccountant` over large randomized event sequences, asserting decision-category
  agreement plus the proven invariants (conservation, `original = remaining + consumed`,
  witness never observes a double-spend) after every operation.

The Lean proves the abstract model has the invariants; the Rust test confirms the
implementation matches the model on randomized inputs. Both are checkable, locally.

## Checking it

For the separate question of whether spending leaves mandatory obligations
affordable, see the [accounting / residual-viability bridge](../docs/working/ACCOUNTING-VIABILITY-BRIDGE.md).
It is a formalized invariant candidate connected to bounded constituent
semantics, not an implemented cross-stack reserve controller or an extension
of this ledger's runtime guarantees.

```sh
# Proofs (exit 0 == all theorems check):
lean verification/Ledger.lean
#   or, as a lake project: (cd verification && lake build)

# Implementation cross-check:
cargo test --test differential_oracle
```

## Scope / honesty

- Single scope. Multi-scope is the disjoint union of independent single-scope ledgers;
  conservation holds per scope, so one scope is faithful (the Rust test exercises two).
- No clock/expiry in the Lean model. Expiry only *blocks* consume; it never restocks in
  v0 (the restock-on-expiry ruling is parked under the freeze). Conservation is stated
  over `original`, which neither consume nor expiry mutates — so the omission is sound.
- `UnknownToken` is not exercised by the differential test: a `TokenId` has no public
  constructor, so a forged id is unrepresentable from outside the crate. That inability
  is a boundary property, not a coverage gap.

## What the proof does NOT claim (assumptions & division of labour)

The Lean theorems are deliberately narrow. To avoid over-reading them:

- **Unbounded arithmetic.** Lean uses `Nat`; the Rust uses `u64`. The conservation proof
  therefore holds *modulo no overflow* — it transfers to the implementation only while
  every running total stays below `2^64`. `deposit`/grant use unchecked `+=`/`-=`
  (`src/lib.rs`), so a deposit that overflows would break the identity (debug: panic;
  release: wrap). v0 is in-memory and non-production with small amounts; the proof does
  not certify behaviour at the `u64` boundary. A bounded/`checked_add` model is future
  work, not claimed here.
- **The Lean `request` models mint-vs-no-op arithmetic only.** The full refusal taxonomy
  — empty-eligibility, stale-warrant, idempotent-replay dedup, zero-amount — is *not* in
  the proof. Each of those is a no-op on stock, so it trivially preserves conservation;
  the Lean captures the only two conservation-relevant outcomes (mint, or nothing). The
  **differential test** is what checks those refusal branches and the idempotent-replay
  grant path (`tests/differential_oracle.rs`) against `lib.rs` decision-for-decision.
- **The Lean `consume` omits the expiry and scope-mismatch refusals** (no clock, single
  scope). Replay/revoked/insufficient are modelled, in `lib.rs` branch order (replay
  first). Expiry and scope mismatch are covered only by the differential test.
- **State-only, not receipt/category fidelity.** `conservation` and `replay_is_noop` are
  about ledger *state*. They do not prove the decision *variant* or receipt body returned
  to a caller is correct — that fidelity is the differential test's job.

So: the proof certifies the arithmetic core (conservation, replay-as-no-op over ℕ); the
differential test certifies the implementation matches the full decision semantics. Read
together, not as one claim. (These boundaries were tightened after an independent review;
they are disclosures, not residual bugs.)
