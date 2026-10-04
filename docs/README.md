# Linear Accountant Documentation

Where to start, by what you're trying to do.

## I want to understand what this is

- [`architecture/V0_BOUNDARY.md`](architecture/V0_BOUNDARY.md) — the authoritative
  behavior spec: the spine, the three pillars, the inviolable rule, the interface.
- [`architecture/ROLE.md`](architecture/ROLE.md) — what the Spendability Authority role
  is, what it is not, and where it sits in the constellation (AG / Wicket / NQ /
  Nightshift / execution).
- [`architecture/HANDOFF_PACKETS.md`](architecture/HANDOFF_PACKETS.md) — the per-tool
  packet contracts other constellation tools would use *if/when* a consumer trigger fires.

## I want to account a model decider's inference spend

- [`INFERENCE_ACCOUNTING.md`](INFERENCE_ACCOUNTING.md) — `la_inference`: durable
  reserve / begin-call / settle / reconcile over the v0 core, the no-storage rule, and
  crash recovery. Tested, not covered by the v0 Lean proof.

## I want to know what was actually proven

- [`working/specimens/workload-specimens.md`](working/specimens/workload-specimens.md) —
  the workload registry: one entry per real workload bound to the accountant, what it
  proved on contact, and the freeze decision.
- [`working/specimens/workload-contact-notes.md`](working/specimens/workload-contact-notes.md)
  — detailed prose for the first workload (WL-001).

## I'm scoping future work

These are **candidate, non-binding** notes — surfaces named early so they don't get
retrofit-bolted later. A record is not authorization to build.

- [`working/decisions/deployment-shape.md`](working/decisions/deployment-shape.md) — how
  this actually ships: a gateway with a `consume()` call. The four honest hard parts.
- [`working/decisions/custody-legibility.md`](working/decisions/custody-legibility.md) —
  the second invariant (custody must not be silent); the external anchor still deferred.
- [`working/decisions/capability-composition.md`](working/decisions/capability-composition.md)
  — the capability axis (composition is minting); why it is a different refusal from spend.

## Naming convention

Where a doc lives tells you what it's for:

| Lifecycle | Location | Mutability |
|---|---|---|
| Ratified design (built against) | `architecture/` | kept current |
| Candidate / non-binding doctrine | `working/decisions/` | mutable until ratified, then promoted or retired |
| Workload contact records | `working/specimens/` | append-on-contact |

Two rules:

1. **Promote into `architecture/` only when ratified.** Until then a note sits under
   `working/`.
2. **Don't promote by duplication.** If a `working/` note becomes architecture, move it;
   don't clone it into a parallel canon.
