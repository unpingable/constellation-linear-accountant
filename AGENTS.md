# AGENTS.md — Working in this repo

This file is a **travel guide** for AI agents working on Linear Accountant, not a law.
If anything here conflicts with the user's explicit instructions, the user wins.

> Instruction files shape behavior; the user determines direction.

---

## Quick start

```bash
cargo build
cargo test                                  # 17 tests across 3 suites
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

For public documentation, call the repository **Constellation Linear
Accountant** and link
`https://github.com/unpingable/constellation-linear-accountant`. Preserve the
existing crate and `la_cli` names. Publication does not thaw the reference
boundary or authorize a new consumer surface.

Before campaign work, identify the selected campaign, exact source revision,
working directory, output locations, and owning authority boundary. A plan,
receipt, or passing test does not authorize external effects. For a prolonged
run, use the campaign-approved durable producer and persist an inspection and
resume checkpoint before waiting. A fresh supervisor must inspect the original
producer and evidence and resume it when possible; never restart or replace it
merely because supervision was interrupted. If no approved durable mechanism
exists, stop before launch and record that limitation rather than implying the
run can survive supervisor loss.

Primarily a library crate others call. One thin transport binary exists,
`la_cli` (added 2026-06-16 on the first consumer trigger — the Agent Governor
bootstrap-lab effect gate). It is a stdin/stdout line protocol over the existing
`request_capacity`/`consume`/`deposit` decisions and adds **no policy** (each
command maps 1:1 to an accountant method; the seam stays narrow). See
[`docs/LA_CLI_PROTOCOL.md`](docs/LA_CLI_PROTOCOL.md). The library is still the
boundary; the binary only carries decisions to an out-of-process consumer.

## What this is

Linear Accountant is the reference boundary for the **Spendability Authority** role. It
conserves spendability: validity is contractible, capacity is linear. It mints and
consumes *capacity*; it does not judge validity, set budgets, or authorize action.

**Not** general agent safety. **Not** a policy engine. **Not** authorization. **Not**
persuadable. A turnstile, not a cathedral; a cash register, not a judge.

See [`README.md`](README.md) for the full framing and
[`docs/architecture/V0_BOUNDARY.md`](docs/architecture/V0_BOUNDARY.md) for authoritative
behavior.

## Repository layout

```
src/lib.rs                          The conserved core (single-writer, in-memory)
tests/v0_boundary.rs                Boundary properties
tests/file_write_workload.rs        WL-001: a real file write gated by the accountant
tests/contention_workload.rs        WL-002: concurrent consumers race one token
docs/architecture/                  Ratified design (V0_BOUNDARY, ROLE, HANDOFF_PACKETS)
docs/working/decisions/             Candidate, non-binding doctrine notes
docs/working/specimens/             Workload contact records (what each workload proved)
```

## Invariants

If one of these breaks, something is **wrong** — not "could be improved," wrong.

1. **No minting by the persuadable.** Only the accountant issues or consumes capacity.
   `TokenId` / `ReceiptId` have no public constructor; a proposer's vocabulary cannot
   express minting.
2. **Eligibility is contractible; capacity is linear.** The same valid eligibility
   reference, cited twice, must not yield two spends.
3. **Custody must not be silent.** Every event — grant, denial, consume, refusal,
   revoke, and the custodial `deposit` — is appended to the ledger with a body.
4. **The seam stays narrow.** Mint/consume decisions accept only sealed references and
   typed descriptors. No free-text justification field enters a decision. Free text in
   the seam reintroduces the thing the accountant exists to refuse.

## Coding conventions

- Rust 2021 edition. No external runtime dependencies (std only).
- Decisions are mechanical: timestamp / integer / set-membership / equality. Nothing
  parses a story. If a code path starts reasoning about *why*, it has become a judge —
  stop.
- Scope is matched by `==`, never containment. Any scope-hierarchy semantics belongs in
  the eligibility layer, never here.
- Caller-supplied logical time (`Tick`). No ambient `now()`.
- `cargo fmt` canonical; `cargo clippy --all-targets -- -D warnings` clean. Tests before
  commits — never claim tests pass without running them.

## Status claims

This crate is **frozen as a reference boundary**. The freeze is a claim with a basis:
17 passing tests, two bound workloads (WL-001, WL-002). No further crate slices without a
*consumer trigger* — a real agent stack wanting `consume()` at its tool-call dispatcher.

Long-lived docs rot when they carry too many roles at once. Design docs (`docs/architecture/`)
explain why/how the boundary is shaped; workload specimens (`docs/working/specimens/`)
record what contact proved; tests are the evidence. Don't promote a `working/` note into
`architecture/` until it is ratified, and move rather than clone when you do.

Use this repo's local vocabulary (spendability, eligibility, conservation, witness,
custody). Recognition rules travel between projects; vocabulary does not.

## Safety and irreversibility

### Do not do without explicit user confirmation
- `git init`, push to a remote, create/close PRs or issues
- Delete or rewrite git history
- Add runtime dependencies (this crate is std-only by design)
- Change a documented invariant or the public API of the frozen boundary

### Preferred workflow
- Small, reviewable steps; run `cargo test` + clippy + fmt before proposing commits
- New design surfaces get a candidate note under `docs/working/`, not an implementation

## Constellation

Linear Accountant is one role among several. It interfaces with — but does not subsume —
the others.

| Role | What it holds | Repo |
|------|---------------|------|
| Constellation AG | governs the request before it becomes spend (eligibility) | `constellation-ag` |
| Wicket / WLP | admission gate / transport envelope | `wicket`, `wlp` |
| Constellation NQ | witness / testimony | `constellation-nq` |
| Nightshift | temporal operator / revalidation scheduler | `nightshift` |
| **Linear Accountant** | **conserves and consumes spendability (this repo)** | `linearaccountant` |

> NQ testifies. Nightshift times. Wicket admits. Linear counts. AG governs the request
> before it becomes spend.

## When you're unsure

Ask rather than guess, especially around:
- Whether a change opens a new surface (forcing case required) or finishes an
  already-opened one (completeness obligation).
- Anything touching the conservation key, the public API, or a documented invariant.
- Whether a workload is the *next deliberate slice* or scope creep.

## Agent-specific instruction files

| Agent | File | Role |
|-------|------|------|
| Codex / any agent | `AGENTS.md` (this file) | Start here |
| Claude Code | `CLAUDE.md` | Defers here; adds Claude-specific notes |
