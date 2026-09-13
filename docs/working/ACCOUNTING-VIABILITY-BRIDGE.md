# Execution spending and what remains affordable

Status: **formalized bridge / invariant candidate**. This note changes no
runtime API, policy, frozen reference boundary, or component ownership.

Linear Accountant constrains execution spend. Viability constrains the state
execution is allowed to leave behind. An action can be affordable and stay
within its authorized budget while leaving too little capacity to discharge
a mandatory obligation.

For example, starting with three units, an allowed two-unit spend leaves one.
If an existing mandatory obligation requires two units, that residual state
is not viable even though the spend itself was affordable and within budget.

## The bounded formal bridge

A scalar Lean model establishes both that separation and this sufficient rule:

```text
spend + requiredReserve <= available
```

Here `requiredReserve` is the sum of all currently mandatory scalar demands.
The rule implies `requiredReserve <= available - spend` under additive,
single-resource, no-replenishment assumptions. The budget check is independent
of the reserve premise. This is not a result about future arrivals, deadlines,
observation uncertainty, nonfungible resources, scheduling or recovery.

Bounded conformance relates the existing single-token `consume` behavior to
scalar subtraction: a successful consumption subtracts its amount from the
token's remaining capacity; an identical event replay spends nothing further.
Expiry, revocation, scope and capacity checks remain separate conditions.
The frozen `OBLIGATION-VIABILITY-V0` experiment supplies a corresponding bounded
reserve/completion-viability policy case. Its ordinary reservation result does
not establish a universal reservation algorithm or runtime controller.

## Existing responsibilities, not a new office

| Surface | What this relationship does—and does not—establish |
| --- | --- |
| Linear Accountant | Accounts for execution spend; does not determine mandatory obligation costs or protect their reserve. |
| Standing / admission | Establishes applicable entitlement or admissibility; that alone does not establish residual viability. |
| AG / Docket | Authorizes exact work and tracks execution outcomes; currently has no live numeric obligation-cost, reserve-admission or viability verdict. |
| OBLIGATION-VIABILITY-V0 | Frozen executable policy fixture, not an installed cross-stack controller. |
| Baby River | Related resource/state-transition and measurement research, not a reserve admission implementation. |

The bridge is currently operational only in these **bounded constituent
semantics**. An end-to-end runtime viability guarantee would require an
explicitly owned obligation-cost / protected-reserve admission predicate,
connected to the relevant effect boundary and independently verified. This
note neither assigns that future responsibility nor adds a component or office.

The existing [verification scope](../../verification/README.md) still applies:
the arithmetic model does not prove all Rust numeric behavior, deployment,
authority or cross-component execution correct.

## Deferred research notes only

- Observable viability / indistinguishability would need an observation
  relation and a precisely bounded class of observation-based procedures.
- Renewal-rate versus state-drift sufficiency would need time, refresh/drift
  semantics and compatible units.

Neither is an immediate corollary of this scalar bridge. Neither is expanded
as part of the current documentation/unification campaign.
