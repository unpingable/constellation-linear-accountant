# Linear Accountant / Spendability Authority

*Internal mnemonic: "Scrooge." Formal name: **Linear Accountant** or **Spendability Authority**.*

## 1. Status / discipline

- **Candidate role definition.** Not ratified doctrine.
- Not an implementation plan. Not a new repo. Not a Lean task. Not a paper.
- Names a missing constellation role so it can be reviewed; naming is not building.
- **Capability inventory ≠ work commitment.** Listing what this role *would* own does not commit anyone to build it.
- This note itself states the candidate invariant: *validity is not spendability*.
- Per-tool interaction contracts live in `HANDOFF_PACKETS.md` (handoff-shape only; sequenced *after* this note, *before* any reference harness).
- The stack has **two** core invariants: (1) *validity is not spendability* — this note, the accountant; (2) *custody must not be silent* — `../working/decisions/custody-legibility.md`, the witness/override rule. This note covers invariant 1; the residual-sovereign problem is invariant 2's.
- Sharper resting form — **three kinds of refusal, never conflated:** *spend is counted* (the accountant — arithmetic), *capability is checked or judged* (semantic layer + `../working/decisions/capability-composition.md` — a hardness gradient, not a kernel; composition is minting), *custody is held* (witness/override — legibility, not elimination).

## 2. Abstract

The Linear Accountant owns spendable capacity. It receives eligibility requests from semantic governors and admission gates, and in response mints the spendability objects — leases, tokens, budget units, reservations — that downstream effects must consume. It records consumption atomically and exactly-once, so that the same grant cannot be spent twice. It does **not** accept semantic validity as payment: a premise being valid, admissible, or well-witnessed is a *reason to ask*, never a *unit of capacity*. The accountant is the only party that may mint or consume capacity.

## 3. Core invariant

- **Validation may mint eligibility; it may not mint capacity.**
- A valid premise may *request* spendable capacity, but only the Linear Accountant may *mint or consume* it.
- A receipt/testimony object is **not** spendable unless explicitly typed as a spendable token.
- Semantic context may **observe** accountant state but may not mutate, regenerate, or summarize it into authority.

Formal backing: validity facts are *contractible* (`[A] ⊢ A` freely, `[A] ⊢ A, A, …`); spendable resources are *linear* (`[A] ⊬ A ⊗ A`). A contractible fact must not cross into a non-contractible role without an explicit allocator / lease / token / budget boundary.

## 4. Role boundaries in the constellation

Roles, not repos. Several may co-reside in one process today; the boundary is in *authority*, not deployment.

| Role | Owns | Explicitly does NOT |
|---|---|---|
| **Wicket** | admission gate / receiver policy kernel; decides if a claim/request is *admissible* | admission ≠ spendable-mutation authority |
| **WLP** *(retired 2026-09-20)* | was: transport envelope / claim shape. No named consumer; there is no current WLP compatibility obligation. The role slot stays open; WLP does not fill it | the envelope is not money |
| **Agent Governor** | semantic governor; **governs the request *before* it becomes spend** — actor/scope/standing/evidence/policy/posture → *eligible to ask for a spendability class*; shapes the budget request; *requests* spendability. Necessary as a **role, not that exact repo forever** | may not mint, consume, or set budget |
| **Linear Accountant** | spendable capacity: budgets, leases, quotas, one-shot tokens, retry allowances, blast-radius slots, idempotency keys; mints & consumes exactly-once | — |
| **Execution layer** | performs the effect; consumes token/lease/budget atomically; emits receipt | may not self-issue capacity |
| **NQ** | witness/testimony layer; can testify about double-spend, lease reuse, quota overrun, missing consumption evidence | does not allocate or enforce capacity |
| **Nightshift** | temporal **circuit breaker** / revalidation scheduler; decides whether a stale warrant must be rechecked before a workflow continues (proceed/defer/revalidate) | does not own spendability; does not mint |

**AG vs Wicket** (they are not the same gate). Wicket says *"this intent is admissible
to this receiver under this policy"* — narrow, receiver-side. AG says *"given actor,
scope, standing, evidence, current policy and operational posture, this request is
eligible to ask for this class of spendability"* — broader semantic coordination,
upstream of spend. Then the accountant says *"fine — there is or is not coin."*

**The minimal loop omits AG.** `Wicket → Linear Accountant → execution → NQ` is enough
for a *pre-budgeted* toy (budget already known, hard-coded) — which is exactly what
specimen WL-001 is. The **real** pipeline needs the AG role, because someone must answer
what the accountant must never answer: who is this actor, what are they doing, is it in
scope, what budget class applies, who may allocate/raise budget, what standing/evidence
supports it, should this be deferred/escalated/narrowed/denied. Without AG you either
hard-code budgets or quietly shove governance into Wicket / Linear / Nightshift / the
execution wrapper — **the old sin in new shoes** (governance hiding in the dumb layers).

> NQ testifies. Nightshift times. Wicket admits. Linear counts. **AG governs the request
> before it becomes spend.**

For the toy: no AG. For the real pipeline: yes — *unless something else explicitly
occupies the Agent Governor role.*

## 5. Object vocabulary

- **Eligibility** — a validated *reason to request* capacity. Contractible. Reusable. Not a unit of anything.
- **Spendable capacity** — a finite, linear stock the accountant tracks (budget, slots, count).
- **Token / lease / reservation / budget unit** — a typed spendability object minted against capacity; the only thing execution may consume.
- **Consumption event** — the atomic, exactly-once decrement that turns a token into a spent effect.
- **Receipt** — proof that a consumption event happened. Evidence, not capacity.
- **Testimony** — NQ's after-the-fact assertion about consumption/non-duplication. Evidence, not authority.
- **Replay / reuse** — presenting an already-consumed (or already-minted-and-spent) object again. The thing linearity forbids.
- **Regeneration** — reconstructing capacity from validity/context/history rather than from the accountant's stock. Forbidden; this is minting-by-narration.

## 6. Allowed flow

```
witness/testimony → admission/eligibility → request capacity → mint token/lease
   → execute/consume → emit receipt → witness can testify
```

Invariant across the seam:

- `validity fact → eligibility` is **contractible** (may be observed, copied, re-derived freely).
- `token/lease → consumption` is **linear** (minted once, consumed once, never duplicated).

The boundary between the two halves is the only place capacity is created, and the accountant holds the pen there.

## 7. Forbidden flows

Each is a place where a contractible fact is treated as linear capacity:

- policy allow → "therefore budget exists"
- receiver acceptance → replayable mutation authority
- receipt/testimony → reusable lease
- prior allowance summarized into agent context → fresh capacity
- retry loop reuses stale validation as new spendability
- scope grant bundles eligibility and usage-count with no hard boundary between them
- override validity and TTL decrement share one mutable semantic surface
- NQ testimony treated as allocation authority

## 8. Audit checklist

The discriminator is **convertibility, not co-location.** Validity state and a use-counter living in the same struct is *not* a violation. The violation requires a *conversion path*: some consumer that reads the validity fact (or a co-located counter) and turns it into spend — gating, decrementing, or granting capacity. A counter that is only logged or displayed *testifies* to consumption; it never *becomes* capacity. Co-location is a place to look, never by itself a finding.

For any surface suspected of mixing validity and spendability:

1. What validates the request?
2. What allocates capacity?
3. Are those the same mutable state? *(co-location — a lead, not a verdict)*
4. **Is there a conversion path** — does any consumer read validity/state to gate, decrement, or grant spend? *(the actual cut)*
5. Can validity regenerate spendability?
6. Is consumption atomic?
7. Can the agent/operator mutate the accountant state?
8. Can receipts be replayed as capacity?
9. Is there a CAS / lease / token / reservation boundary?
10. Can NQ testify after the fact that no double-spend occurred?

A surface is a refactor candidate only if (4) is "yes" — a real conversion path exists — *and* (9) is "no." Co-location alone (3 yes, 4 no) is testimony, not capacity; close it as a false positive. Refactor candidacy is an opening trigger (§11), not a mandate.

## 9. Current candidate consumers

Candidates only — listing is not commitment:

- **Agent Governor** — override management, scope grants, dispatcher leases, budgets / per-tool caps, quorum / Neff accounting.
- **Wicket / WLP** — one-shot admission/effect-token semantics; receiver acceptance not replayable.
- **NQ** — testimony schema for budget/lease/quota double-spend.
- **Deployment systems** — blast-radius budgets, rollout slots, maintenance windows, quota units.
- **LinCalc / ContractionHinge** — formal non-duplication witness. Parked.

## 10. Non-goals

Explicitly NOT, here:

- no new daemon
- no repo split
- no implementation
- no Lean module
- no LinCalc
- no paper
- no DOI
- no claim that AG / Wicket / NQ are currently wrong
- no claim that all capacity must be centrally global
- no claim that semantic validators are useless
- no claim that all refusal is linear

## 11. Opening triggers

The role becomes actionable **only if**:

- AG audit finds mixed validity/spendability state needing refactor
- Wicket/WLP needs one-shot effect-token semantics
- NQ needs a testimony schema for double-spend / lease reuse / quota overrun
- deployment safety work needs blast-radius budget accounting
- a real consumer needs non-duplication guarantees

Until one fires: **consumer trigger absent.**

*2026-06-03 — AG axis checked. A fan-out audit flagged scope/reservation/override surfaces; line-level verification (AG custody) killed all on the convertibility cut: no conversion path at any named surface. Trigger #1 did NOT fire. The NQ double-spend testimony schema (trigger #3) remains capability-absent — a gap, not a firing. Role stays parked.*

*2026-06-03 — Deadlock break. "Don't build until a consumer asks" stalled because consumers can't target a role that isn't callable. Resolution: a role others must not accidentally re-implement may have a **reference boundary** built ahead of consumers. Shipped Linear Accountant v0 as a running, non-production crate (this repo) — interface + finite-stock model + 11 passing tests. See `V0_BOUNDARY.md`. This is a turnstile, not a cathedral: it fences one failure class (validity→reusable capacity), not semantic safety. Real per-tool integrations remain gated on their own triggers.*

*2026-06-03 — Budget setting vs enforcement split (the irreducible priesthood). The accountant ENFORCES budgets (arithmetic: is there budget, can this consume, already spent); it must never SET them (how much, who allocates, what risk is worth it, when to raise — semantic/political). Rule: budgets are set by custody, spent by accounting; agents may request both but author neither. Wicket/AG return here as the budget ADMISSION surface (witnessed: who/whom/scope/until/basis/approval), not the spender. The value of the accountant is that its stupidity forces the budget question into the open instead of hiding inside "the agent decided it was fine." v0 gap: `deposit` records the act (Event::Deposited{scope,amount}) but not the basis — a bare mint, not a witnessed grant; budget-setting-witness fields are candidate, not built. See `V0_BOUNDARY.md` §2c.*

*2026-06-03 — Deployment deflation recorded. The doctrine is big because it classifies category errors; the artifact is small. What ships: a strongly-consistent stateful service with the four-verb API (the v0 crate is already that core), atomic consume = a conditional write, wrapped at the tool-call dispatcher (the MCP policy-gateway slot already exists; we're the conserved core it lacks). "OPA with memory / Vault for actions." Rollout = shadow/observe → one boring effect class → ratchet. Four honest hard parts: effect taxonomy, atomic consume (= WL-002's falsification test), fail-open politics (the toggle emits a receipt day-one — runtime form of custody-legibility), ownership (platform/ops, = the custody-root assignment). Most concrete consumer trigger to date: an agent stack wanting `consume()` on its dispatcher in observe mode. Spec: `../working/decisions/deployment-shape.md`. Not opened; consumer-trigger still gates. "Nobody deploys a constitution; they deploy a gateway with a consume() call."*

*2026-06-03 — First workload bound. `tests/file_write_workload.rs`: a real `std::fs::write` gated by the accountant — full path (propose→eligibility-ref→request→token→consume→effect→receipt→replay-denied→witness-testifies) against an actual effect, 15 tests green. Findings (see `../working/specimens/workload-contact-notes.md`): (a) v0 API needed ZERO changes — the execution boundary is pure consumer code, the propose/dispose seam held; (b) contact forced one real gap — ledger events aren't keyed by request/correlation id, so witnessing "did request R double-spend?" requires scanning for the Granted event; that's the first concrete shape for NQ's testimony schema (trigger #3), candidate not built. "Eligibility contractible / capacity linear" is now empirically true: same warrant cited twice, second denied by spent budget. Recorded as specimen WL-001 in `../working/specimens/workload-specimens.md`; the request_id gap is classified as a candidate testimony/query-shape gap (witness layer), NOT an accountant correctness bug — the accountant is not to be changed for it. Next workload is a separate slice.*

*2026-06-03 — Contact. Parallel synthesis loop (ChatGPT/paper-Claude/DeepSeek/Gemini) converged and called "stop synthesizing, make contact." Closed the deltas: (a) the contact loop now runs end-to-end in code through a read-only witness (`witness::testify_no_double_spend`) — eligible→token→consume→receipt→replay-denied→testify; (b) closed the in-process half of invariant 2 — `deposit` is recorded, receipts have bodies via an append-only `ledger()` (external anchor still deferred); (c) filed `../working/decisions/capability-composition.md` naming the capability axis (composition is minting; membership allowlist forgeable by composition; Tier 0–3 hardness gradient) as candidate — no flow checker built. 14 tests. Resting doctrine is now three-axis: spend counted / capability judged / custody held, never the same refusal. Next crack should come from a workload, not more synthesis.*

*2026-06-03 — Spine revision. Reframed: sovereignty did not disappear, it **relocated from semantics to arithmetic** — "the sovereign cannot be semantic." Architecture rests on three pillars: conservation (provable, Pillar 1), narrow seams (security ≈ message impoverishment, Pillar 2-engineering), freshness (temporal breaker, Pillar 3). Burden split made explicit: conservation is proof, eligibility-separation is posture — we do NOT claim semantic governors are impossible. Closed the temporal gap in code: added `eligibility_valid_until` + a stale-warrant refusal distinct from "ineligible" (12 tests now). Nightshift reclassified as the temporal circuit breaker / revalidation scheduler. Inviolable rule: **no minting by the persuadable.** Standing watch-point: keep the descriptor typed — free text in the seam reintroduces Pillar-2 dependence.*

## 12. Final keeper

> The Linear Accountant counts spendability, not stories.
> Eligibility is a request. It is not payment.
> Only the spendability authority may mint or consume capacity.
