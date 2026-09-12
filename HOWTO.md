# Linear Accountant: local first run

This repository is a frozen, in-memory reference boundary. The supported first
run verifies conservation and the two bound workloads; it does not install a
service or grant authorization.

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

To inspect the thin line protocol without adding policy, follow
[`docs/LA_CLI_PROTOCOL.md`](docs/LA_CLI_PROTOCOL.md). This deterministic stdin
fixture stays in one in-memory process and performs no external effect:

```sh
printf '%s\n' \
  'cmd=deposit	scope=tutorial	amount=1	admission_ref=fixture-budget' \
  'cmd=request_capacity	request_id=fixture-1	actor=operator	action=demo	target=tutorial	scope=tutorial	requested_capacity=1	eligibility_reference=fixture-eligibility	eligibility_valid_until=100	expires_after=100	tick=1' \
  'cmd=consume	consumption_event_id=fixture-consume	token_id=t0	actor=operator	action=demo	target=tutorial	amount=1	scope=tutorial	tick=2' \
  | target/debug/la_cli
```

A passing run establishes the tested capacity-conservation and replay-refusal
properties only.

Public source: <https://github.com/unpingable/constellation-linear-accountant>

Verification note (2026-09-12): `cargo test --offline --locked -j1` passed 38
tests, `cargo clippy --offline --locked -j1 --all-targets -- -D warnings` and
`cargo fmt --check` passed, and the stream above completed with deposited,
granted, and consumed decisions. These are local in-memory qualification
results only. The `la_cli` protocol and accepted fields were checked against
`src/bin/la_cli.rs` and `docs/LA_CLI_PROTOCOL.md`.
