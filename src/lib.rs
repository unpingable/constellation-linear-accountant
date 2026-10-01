//! # Linear Accountant v0 — Spendability Authority (reference boundary)
//!
//! **Status: FROZEN as a reference boundary (2026-06-04).** Both questions a reference
//! must answer are answered on contact: WL-001 (`tests/file_write_workload.rs`) — that
//! eligibility alone cannot execute; WL-002 (`tests/contention_workload.rs`) — that the
//! consume mechanism is correct under real contention through one serialization point.
//! No further crate changes without a *consumer trigger*: a real agent stack wanting
//! `consume()` at its tool-call dispatcher. See `docs/working/specimens/workload-specimens.md`.
//!
//! **Carve-out (the door).** A frozen boundary may expose a *refusal-only* preflight so a
//! prospective consumer can discover the request shape and receive a structured "not yet"
//! ([`preflight`]). It must not mutate state. *Doors are allowed; hidden rooms are not.*
//! The first real use of that door is the visible consumer boundary that fires the thaw.
//!
//! **Non-production.** In-memory, single-writer, no persistence, no distribution.
//! Executable form of `docs/architecture/V0_BOUNDARY.md`. Other constellation
//! tools conform to this contract; they do not co-design it.
//!
//! ## Three kinds of refusal — never conflated
//!
//! > Spend is counted. Capability is checked or judged. Custody is held.
//!
//! This crate implements exactly the **spend** axis. It conserves spendability; it
//! does not judge capability (that is the semantic layer / the composition problem —
//! see `docs/working/decisions/capability-composition.md`) and it does not hold custody (that is the
//! witness/override problem — see `docs/working/decisions/custody-legibility.md`).
//!
//! ## Stance: small, hostile, stupid
//!
//! A tiny goblin with an integer. It does not inspect reasoning, parse justification,
//! or adjudicate validity. It accepts only **sealed references** it can check
//! mechanically; it never accepts a model-generated summary as authority. The model
//! can complain in twelve paragraphs; the goblin still returns `AlreadyConsumed`.
//!
//! ## Two linear boundaries, enforced by finite stock
//!
//! 1. **stock → token** at grant — a depletable pool per scope. Citing the same
//!    eligibility forever cannot refill it.
//! 2. **token → effect** at consume — drawn down atomically, exactly-once per event id.
//!
//! ## Custody legibility (in-process down-payment)
//!
//! Every event — including the custodial `deposit` that mints stock into existence, now
//! **citing the [`BudgetAdmissionRef`] it was minted against** (carried and recorded,
//! never evaluated) — is appended to a retrievable [`ledger`](InMemoryAccountant::ledger).
//! Receipts have bodies. A read-only [`witness`] can testify, from the ledger, that no
//! double-spend occurred. The genuinely hard half — an anchor *outside the actor's
//! unilateral control* — is deliberately NOT here.

use std::collections::HashMap;
use std::collections::HashSet;

/// Caller-supplied logical time. No ambient `now()`.
pub type Tick = u64;

/// A minted spendability handle. Opaque; only the accountant issues it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TokenId(u64);

/// Evidence handle. Type-distinct from [`TokenId`]: a receipt can never be presented
/// where a token is. Indexes into the [`ledger`](InMemoryAccountant::ledger).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ReceiptId(u64);

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RequestId(pub String);

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct EventId(pub String);

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Scope(pub String);

/// A sealed pointer to a budget admission decided elsewhere, cited by [`deposit`] when it
/// mints stock into existence. The mint boundary's answer to invariant 4: a mint carries a
/// sealed reference and a typed descriptor, the same way a grant carries
/// `eligibility_reference`. LA **stores and cites it verbatim; it never parses, evaluates,
/// or judges it** — "this stock was minted against admission R" is a linkage claim, never
/// "R was legitimate authorization" (that is the budget-setting priesthood, out of scope).
///
/// `admission_ref` is the load-bearing sealed pointer: `deposit` refuses to seed stock
/// against an empty one. `basis_kind` is a typed descriptor of the admitting authority — a
/// label carried for the witness, never a policy input LA branches on.
///
/// [`deposit`]: InMemoryAccountant::deposit
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BudgetAdmissionRef {
    pub admission_ref: String,
    pub basis_kind: String,
}

/// A request for spendable capacity.
///
/// `eligibility_reference` is an **opaque sealed pointer** to a validity fact decided
/// elsewhere — never parsed, never interpreted. There is deliberately no free-text
/// justification field: the seam is too narrow to carry a plea.
///
/// `eligibility_valid_until` bounds the warrant's freshness (checked at request);
/// `expires_after` bounds the minted token's life (checked at consume).
#[derive(Clone, Debug)]
pub struct CapacityRequest {
    pub request_id: RequestId,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub scope: Scope,
    pub requested_capacity: u64,
    pub eligibility_reference: String,
    pub eligibility_valid_until: Tick,
    pub expires_after: Tick,
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ConsumeRequest {
    pub consumption_event_id: EventId,
    pub token_id: TokenId,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub amount: u64,
    pub scope: Scope,
}

/// The outcome of a custodial `deposit`. A mint that cannot cite an admission is refused,
/// fail-closed, and the refusal is recorded — the mint boundary is never silently skipped.
#[derive(Clone, Debug, PartialEq)]
pub enum DepositDecision {
    Deposited {
        scope: Scope,
        amount: u64,
        receipt: ReceiptId,
    },
    Refused {
        reason: String,
        receipt: ReceiptId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum CapacityDecision {
    Granted {
        token_id: TokenId,
        granted_capacity: u64,
        scope: Scope,
        expires_at: Tick,
        receipt: ReceiptId,
    },
    Denied {
        denial_reason: String,
        receipt: ReceiptId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConsumptionDecision {
    Consumed {
        token_id: TokenId,
        consumed_amount: u64,
        remaining_capacity: u64,
        receipt: ReceiptId,
    },
    AlreadyConsumed {
        token_id: TokenId,
        receipt: ReceiptId,
    },
    InsufficientCapacity {
        token_id: TokenId,
        remaining_capacity: u64,
        requested_amount: u64,
        receipt: ReceiptId,
    },
    Expired {
        token_id: TokenId,
        expired_at: Tick,
        receipt: ReceiptId,
    },
    Revoked {
        token_id: TokenId,
        revoked_at: Tick,
        receipt: ReceiptId,
    },
    UnknownToken {
        token_id: TokenId,
        receipt: ReceiptId,
    },
    ScopeMismatch {
        token_id: TokenId,
        expected_scope: Scope,
        requested_scope: Scope,
        receipt: ReceiptId,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum RevocationDecision {
    Revoked {
        token_id: TokenId,
        revoked_at: Tick,
        receipt: ReceiptId,
    },
    AlreadyFinal {
        token_id: TokenId,
        final_status: TokenStatus,
        receipt: ReceiptId,
    },
}

/// A single-use, scoped, target/effect-class-bound, expiring **bounded execution capability** — the
/// "key" the receiver gate validates and burns. Minted by LA against a granted token; it binds the
/// token's opaque `eligibility_reference` verbatim. This is the artifact WLP spec'd by omission
/// (WLP is retired — there is no current WLP compatibility obligation; the lineage is kept, the obligation is not); it
/// lives here (the minting authority), never in the consumer.
#[derive(Clone, Debug, PartialEq)]
pub struct SpendCapability {
    pub capability_id: String,
    pub token_id: TokenId,
    pub scope: Scope,
    pub target: String,
    pub effect_class: String,
    pub eligibility_reference: String,
    pub issued_at: Tick,
    pub expires_at: Tick,
    pub single_use: bool,
}

/// Why a capability could not be minted. Fail-closed: a capability is never issued against a token that
/// cannot back it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CapabilityError {
    UnknownToken,
    TokenExpired,
    TokenRevoked,
    Exhausted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenStatus {
    Active,
    Exhausted,
    Revoked,
    Unknown,
}

/// Read-only snapshot. Observation of remaining capacity is **not** a lease on it.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenView {
    pub token_id: TokenId,
    pub status: TokenStatus,
    pub original_capacity: u64,
    pub remaining_capacity: u64,
    pub scope: Scope,
    pub issued_to: String,
    pub issued_at: Tick,
    pub expires_at: Tick,
    pub revoked_at: Option<Tick>,
    pub consumption_count: usize,
}

/// An entry in the accountant's append-only ledger. The `receipt_id` is monotonic and
/// doubles as a sequence number — a total order without wall-clock time.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceiptRecord {
    pub receipt_id: ReceiptId,
    pub event: Event,
}

/// What the accountant did. The body behind a receipt. Includes the custodial
/// `Deposited` event, so minting stock into existence is not silent.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Deposited {
        scope: Scope,
        amount: u64,
        admission: BudgetAdmissionRef,
    },
    DepositRefused {
        scope: Scope,
        amount: u64,
        reason: String,
    },
    Granted {
        token_id: TokenId,
        granted_capacity: u64,
        scope: Scope,
        expires_at: Tick,
    },
    Denied {
        reason: String,
    },
    Consumed {
        token_id: TokenId,
        amount: u64,
        remaining_after: u64,
        event_id: String,
    },
    ConsumeRefused {
        token_id: TokenId,
        kind: String,
        event_id: String,
    },
    Revoked {
        token_id: TokenId,
    },
    RevokeRefused {
        token_id: TokenId,
    },
    /// A single-use spend capability was minted against a granted token (the bounded execution key).
    /// Minting does not consume capacity; the burn happens at consume (the real effect path).
    CapabilityIssued {
        token_id: TokenId,
        capability_id: String,
        target: String,
        effect_class: String,
    },
}

struct TokenState {
    issued_to: String,
    scope: Scope,
    original_capacity: u64,
    remaining_capacity: u64,
    issued_at: Tick,
    expires_at: Tick,
    revoked_at: Option<Tick>,
    consumed_events: HashSet<String>,
    /// The opaque eligibility reference this grant was issued against. A spend capability binds this
    /// verbatim — nobody recomputes the upstream Standing digest.
    eligibility_reference: String,
}

impl TokenState {
    fn status(&self) -> TokenStatus {
        if self.revoked_at.is_some() {
            TokenStatus::Revoked
        } else if self.remaining_capacity == 0 {
            TokenStatus::Exhausted
        } else {
            TokenStatus::Active
        }
    }
}

/// The in-memory reference accountant. Single writer; not thread-safe by design.
pub struct InMemoryAccountant {
    stock: HashMap<String, u64>,
    tokens: HashMap<u64, TokenState>,
    granted_requests: HashMap<String, TokenId>,
    ledger: Vec<ReceiptRecord>,
    next_token: u64,
    next_receipt: u64,
}

impl InMemoryAccountant {
    pub fn new() -> Self {
        InMemoryAccountant {
            stock: HashMap::new(),
            tokens: HashMap::new(),
            granted_requests: HashMap::new(),
            ledger: Vec::new(),
            next_token: 1,
            next_receipt: 1,
        }
    }

    /// Seed finite stock for a scope, **citing the budget admission that authorized it**.
    /// The only way capacity enters the system — a custodial mint, recorded in the ledger,
    /// never silent. Fails closed on an empty `admission.admission_ref`: the mint boundary
    /// must cite an admission, the same way `request_capacity` refuses an empty
    /// `eligibility_reference`. LA stores and cites the reference; it never evaluates it.
    pub fn deposit(
        &mut self,
        scope: &Scope,
        amount: u64,
        admission: &BudgetAdmissionRef,
    ) -> DepositDecision {
        if admission.admission_ref.trim().is_empty() {
            let reason = "missing admission reference: the mint boundary must cite a budget \
                          admission, never seed stock it cannot attribute"
                .to_string();
            let receipt = self.record(Event::DepositRefused {
                scope: scope.clone(),
                amount,
                reason: reason.clone(),
            });
            return DepositDecision::Refused { reason, receipt };
        }
        *self.stock.entry(scope.0.clone()).or_insert(0) += amount;
        let receipt = self.record(Event::Deposited {
            scope: scope.clone(),
            amount,
            admission: admission.clone(),
        });
        DepositDecision::Deposited {
            scope: scope.clone(),
            amount,
            receipt,
        }
    }

    /// Remaining stock for a scope. Read-only; observing it grants nothing.
    pub fn available(&self, scope: &Scope) -> u64 {
        self.stock.get(&scope.0).copied().unwrap_or(0)
    }

    /// The append-only ledger. Read-only — the witness layer reads this; it cannot
    /// allocate.
    pub fn ledger(&self) -> &[ReceiptRecord] {
        &self.ledger
    }

    /// Look up a receipt's body by id.
    pub fn receipt(&self, id: ReceiptId) -> Option<&ReceiptRecord> {
        self.ledger.iter().find(|r| r.receipt_id == id)
    }

    fn record(&mut self, event: Event) -> ReceiptId {
        let id = ReceiptId(self.next_receipt);
        self.next_receipt += 1;
        self.ledger.push(ReceiptRecord {
            receipt_id: id,
            event,
        });
        id
    }

    pub fn request_capacity(&mut self, req: CapacityRequest, now: Tick) -> CapacityDecision {
        let CapacityRequest {
            request_id,
            actor,
            scope,
            requested_capacity,
            eligibility_reference,
            eligibility_valid_until,
            expires_after,
            idempotency_key,
            ..
        } = req;

        if eligibility_reference.trim().is_empty() {
            let reason =
                "missing eligibility reference: eligibility is required evidence, never optional"
                    .to_string();
            let receipt = self.record(Event::Denied {
                reason: reason.clone(),
            });
            return CapacityDecision::Denied {
                denial_reason: reason,
                receipt,
            };
        }

        let dedupe_key = idempotency_key.unwrap_or_else(|| request_id.0.clone());
        if let Some(&tid) = self.granted_requests.get(&dedupe_key) {
            let (cap, sc, exp) = {
                let s = &self.tokens[&tid.0];
                (s.original_capacity, s.scope.clone(), s.expires_at)
            };
            // Idempotent replay of a request: return the original grant, mint no new
            // capacity. Recorded as a fresh Granted receipt referencing the same token.
            let receipt = self.record(Event::Granted {
                token_id: tid,
                granted_capacity: cap,
                scope: sc.clone(),
                expires_at: exp,
            });
            return CapacityDecision::Granted {
                token_id: tid,
                granted_capacity: cap,
                scope: sc,
                expires_at: exp,
                receipt,
            };
        }

        if now >= eligibility_valid_until {
            let reason = format!(
                "stale eligibility: warrant valid_until {eligibility_valid_until} <= now {now}"
            );
            let receipt = self.record(Event::Denied {
                reason: reason.clone(),
            });
            return CapacityDecision::Denied {
                denial_reason: reason,
                receipt,
            };
        }

        if requested_capacity == 0 {
            let reason = "zero capacity requested".to_string();
            let receipt = self.record(Event::Denied {
                reason: reason.clone(),
            });
            return CapacityDecision::Denied {
                denial_reason: reason,
                receipt,
            };
        }

        let avail = self.available(&scope);
        if avail < requested_capacity {
            let reason =
                format!("insufficient stock: {avail} available < {requested_capacity} requested");
            let receipt = self.record(Event::Denied {
                reason: reason.clone(),
            });
            return CapacityDecision::Denied {
                denial_reason: reason,
                receipt,
            };
        }

        *self.stock.get_mut(&scope.0).unwrap() -= requested_capacity;
        let tid = TokenId(self.next_token);
        self.next_token += 1;
        let expires_at = now.saturating_add(expires_after);
        self.tokens.insert(
            tid.0,
            TokenState {
                issued_to: actor,
                scope: scope.clone(),
                original_capacity: requested_capacity,
                remaining_capacity: requested_capacity,
                issued_at: now,
                expires_at,
                revoked_at: None,
                consumed_events: HashSet::new(),
                eligibility_reference,
            },
        );
        self.granted_requests.insert(dedupe_key, tid);
        let receipt = self.record(Event::Granted {
            token_id: tid,
            granted_capacity: requested_capacity,
            scope: scope.clone(),
            expires_at,
        });
        CapacityDecision::Granted {
            token_id: tid,
            granted_capacity: requested_capacity,
            scope,
            expires_at,
            receipt,
        }
    }

    /// Mint a single-use spend capability against a granted token. Additive: minting does NOT decrement
    /// stock or consume — the burn happens at `consume` (the real effect path, thawed separately). The
    /// capability binds the token's stored opaque `eligibility_reference` verbatim.
    pub fn issue_capability(
        &mut self,
        token_id: TokenId,
        target: &str,
        effect_class: &str,
        capability_id: &str,
        now: Tick,
    ) -> Result<SpendCapability, CapabilityError> {
        let (scope, eligibility_reference, expires_at) = {
            let s = self
                .tokens
                .get(&token_id.0)
                .ok_or(CapabilityError::UnknownToken)?;
            if s.revoked_at.is_some() {
                return Err(CapabilityError::TokenRevoked);
            }
            if now >= s.expires_at {
                return Err(CapabilityError::TokenExpired);
            }
            if s.remaining_capacity == 0 {
                return Err(CapabilityError::Exhausted);
            }
            (
                s.scope.clone(),
                s.eligibility_reference.clone(),
                s.expires_at,
            )
        };
        let cap = SpendCapability {
            capability_id: capability_id.to_string(),
            token_id,
            scope,
            target: target.to_string(),
            effect_class: effect_class.to_string(),
            eligibility_reference,
            issued_at: now,
            expires_at,
            single_use: true,
        };
        self.record(Event::CapabilityIssued {
            token_id,
            capability_id: capability_id.to_string(),
            target: target.to_string(),
            effect_class: effect_class.to_string(),
        });
        Ok(cap)
    }

    pub fn consume(&mut self, req: ConsumeRequest, now: Tick) -> ConsumptionDecision {
        let tid = req.token_id;
        let eid = req.consumption_event_id.0.clone();

        if !self.tokens.contains_key(&tid.0) {
            let receipt = self.record(Event::ConsumeRefused {
                token_id: tid,
                kind: "UnknownToken".into(),
                event_id: eid,
            });
            return ConsumptionDecision::UnknownToken {
                token_id: tid,
                receipt,
            };
        }

        // Replay: a re-sent event id consumes nothing more. Recorded so the witness
        // can see the breaker held.
        if self.tokens[&tid.0].consumed_events.contains(&eid) {
            let receipt = self.record(Event::ConsumeRefused {
                token_id: tid,
                kind: "AlreadyConsumed".into(),
                event_id: eid,
            });
            return ConsumptionDecision::AlreadyConsumed {
                token_id: tid,
                receipt,
            };
        }

        let (revoked_at, expires_at, scope, remaining) = {
            let s = &self.tokens[&tid.0];
            (
                s.revoked_at,
                s.expires_at,
                s.scope.clone(),
                s.remaining_capacity,
            )
        };

        if let Some(rev) = revoked_at {
            let receipt = self.record(Event::ConsumeRefused {
                token_id: tid,
                kind: "Revoked".into(),
                event_id: eid,
            });
            return ConsumptionDecision::Revoked {
                token_id: tid,
                revoked_at: rev,
                receipt,
            };
        }
        if now >= expires_at {
            let receipt = self.record(Event::ConsumeRefused {
                token_id: tid,
                kind: "Expired".into(),
                event_id: eid,
            });
            return ConsumptionDecision::Expired {
                token_id: tid,
                expired_at: expires_at,
                receipt,
            };
        }
        if req.scope != scope {
            let receipt = self.record(Event::ConsumeRefused {
                token_id: tid,
                kind: "ScopeMismatch".into(),
                event_id: eid,
            });
            return ConsumptionDecision::ScopeMismatch {
                token_id: tid,
                expected_scope: scope,
                requested_scope: req.scope,
                receipt,
            };
        }
        if req.amount > remaining {
            let receipt = self.record(Event::ConsumeRefused {
                token_id: tid,
                kind: "InsufficientCapacity".into(),
                event_id: eid,
            });
            return ConsumptionDecision::InsufficientCapacity {
                token_id: tid,
                remaining_capacity: remaining,
                requested_amount: req.amount,
                receipt,
            };
        }

        let new_remaining = remaining - req.amount;
        let receipt = self.record(Event::Consumed {
            token_id: tid,
            amount: req.amount,
            remaining_after: new_remaining,
            event_id: eid.clone(),
        });
        let s = self.tokens.get_mut(&tid.0).unwrap();
        s.remaining_capacity = new_remaining;
        s.consumed_events.insert(eid);
        ConsumptionDecision::Consumed {
            token_id: tid,
            consumed_amount: req.amount,
            remaining_capacity: new_remaining,
            receipt,
        }
    }

    pub fn inspect_token(&self, token_id: TokenId) -> Option<TokenView> {
        self.tokens.get(&token_id.0).map(|s| TokenView {
            token_id,
            status: s.status(),
            original_capacity: s.original_capacity,
            remaining_capacity: s.remaining_capacity,
            scope: s.scope.clone(),
            issued_to: s.issued_to.clone(),
            issued_at: s.issued_at,
            expires_at: s.expires_at,
            revoked_at: s.revoked_at,
            consumption_count: s.consumed_events.len(),
        })
    }

    pub fn revoke(&mut self, token_id: TokenId, _reason: &str, now: Tick) -> RevocationDecision {
        if !self.tokens.contains_key(&token_id.0) {
            let receipt = self.record(Event::RevokeRefused { token_id });
            return RevocationDecision::AlreadyFinal {
                token_id,
                final_status: TokenStatus::Unknown,
                receipt,
            };
        }
        if self.tokens[&token_id.0].revoked_at.is_some() {
            let receipt = self.record(Event::RevokeRefused { token_id });
            return RevocationDecision::AlreadyFinal {
                token_id,
                final_status: TokenStatus::Revoked,
                receipt,
            };
        }
        let receipt = self.record(Event::Revoked { token_id });
        self.tokens.get_mut(&token_id.0).unwrap().revoked_at = Some(now);
        RevocationDecision::Revoked {
            token_id,
            revoked_at: now,
            receipt,
        }
    }
}

impl Default for InMemoryAccountant {
    fn default() -> Self {
        Self::new()
    }
}

/// The witness layer (NQ-shaped). It reads the ledger and testifies. It takes only a
/// read-only slice — it has no path to allocate or consume, structurally.
pub mod witness {
    use super::{Event, ReceiptRecord, TokenId};
    use std::collections::HashSet;

    #[derive(Clone, Debug, PartialEq)]
    pub enum Testimony {
        NoDoubleSpend {
            token_id: TokenId,
            consumption_events: usize,
            total_consumed: u64,
            replays_refused: usize,
        },
        DoubleSpendObserved {
            token_id: TokenId,
            reused_event_id: String,
        },
    }

    /// Testify, from the immutable ledger, whether a token was ever double-spent.
    /// The accountant prevents double-spend mechanically; this independently confirms
    /// it from the record — and counts the refused replays as evidence the breaker held.
    pub fn testify_no_double_spend(ledger: &[ReceiptRecord], token_id: TokenId) -> Testimony {
        let mut seen: HashSet<String> = HashSet::new();
        let mut total = 0u64;
        let mut events = 0usize;
        let mut replays = 0usize;
        for rec in ledger {
            match &rec.event {
                Event::Consumed {
                    token_id: t,
                    amount,
                    event_id,
                    ..
                } if *t == token_id => {
                    if !seen.insert(event_id.clone()) {
                        return Testimony::DoubleSpendObserved {
                            token_id,
                            reused_event_id: event_id.clone(),
                        };
                    }
                    total += amount;
                    events += 1;
                }
                Event::ConsumeRefused {
                    token_id: t, kind, ..
                } if *t == token_id && kind == "AlreadyConsumed" => {
                    replays += 1;
                }
                _ => {}
            }
        }
        Testimony::NoDoubleSpend {
            token_id,
            consumption_events: events,
            total_consumed: total,
            replays_refused: replays,
        }
    }
}

/// The preflight door (refusal-only). A frozen boundary may expose a *refusal* preflight
/// so a prospective consumer can discover the request shape and receive a structured
/// "not yet" — it must NOT mutate state until the named consumer trigger fires. This is
/// the knock-surface whose first real use *is* the forcing case for thaw.
///
/// > Doors are allowed. Hidden rooms are not.
///
/// Nothing here touches stock, tokens, or the ledger: [`preflight_consume`] is a pure
/// function taking no accountant. No deposits, no spend, no persistence, no mutation —
/// the type signature is the promise.
pub mod preflight {
    /// What a consumer presents at a write-tool dispatcher to ask whether a spend *could*
    /// be authorized. Opaque, mechanical fields only — no free-text justification, so the
    /// seam stays exactly as narrow as the real consume path.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PreflightInquiry {
        pub consumer: String,     // e.g. "maude"
        pub boundary: String,     // e.g. "write_tool_approval"
        pub principal: String,    // workload / bot identity
        pub action_class: String, // e.g. "write_tool"
        pub repo: String,
        pub estimated_units: u64,
        pub basis_receipt: String, // sealed pointer to a standing/watchbill/wicket receipt
    }

    /// Typed refusal identity — never free text. Composes with the typed-refusal
    /// calibration in `WITNESS_CLAIM_SCOPE_GAP`: a refusal's durable identity is a field,
    /// not a prose string a consumer must parse.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum PreflightRefusal {
        /// The consume path is frozen; no spend machinery exists to authorize against.
        ConsumePathNotThawed,
    }

    /// The door's answer. v0 has exactly one outcome: a structured "not thawed" refusal
    /// that names the trigger which would change it. `mutation_performed` is always
    /// `false` — the field exists to make that promise legible to a caller.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct PreflightDecision {
        pub refusal: PreflightRefusal,
        pub consumer_trigger_required: bool,
        pub expected_trigger: String,
        pub observed_boundary: String,
        pub mutation_performed: bool,
        pub freeze_basis: String,
        pub allowed_next_step: String,
    }

    /// Knock on the door. Returns the structured "not thawed" refusal, echoing the
    /// observed boundary so that a real integration becomes a *visible* consumer boundary
    /// — the forcing case for thaw. Performs no mutation of any kind.
    pub fn preflight_consume(inquiry: &PreflightInquiry) -> PreflightDecision {
        PreflightDecision {
            refusal: PreflightRefusal::ConsumePathNotThawed,
            consumer_trigger_required: true,
            expected_trigger: "real agent stack requesting consume() at its tool-call dispatcher"
                .into(),
            observed_boundary: format!("{}.{}", inquiry.consumer, inquiry.boundary),
            mutation_performed: false,
            freeze_basis: "README/CLAUDE.md freeze (v0 reference boundary)".into(),
            allowed_next_step:
                "operator may thaw when dispatcher integration is visible, or by fiat".into(),
        }
    }
}
