# Native Promise transport checkpoint, 2026-10-04

Unfinished implementation, not merge-ready. Paired PRs:
https://github.com/loopdive/v8x/pull/2 and https://github.com/loopdive/js2/pull/6468.
Compiler checkpoint: 74ed7007fb. Preceding verified adapter: 37923f2.

## Implementation

Rust-created Promises have a transport path into a real compiled Promise in
the owning Context. Regular realm bindings retain original native identity;
a mirror registry retains the resolver packet and synchronizes settlement
through the active owner or callback caller. Ordinary compiled Promise
resolution queues reactions. Exact mirrored rejection events are suppressed
during synchronization; the original native settlement remains the notification
source. Other Promise events are not suppressed. No interpreter was added.

Entry points: `src/js2wasm/realm_native_promises.rs`, realm object/value
transport, native settlement in `src/js2wasm/mod.rs`, Context value bridge.

## Verification

Compiler-free cargo check, cargo formatting and diff whitespace checks pass.
Runtime compiler option controls: 11/11. Staged-core runner: PASS, including
pending state, fulfillment/rejection, stable compiled identity, asynchronous
reactions, immutable settlement and invalid settlement flags. This exercises
the compiled helpers, not the Rust mirror transport.
Ordinary compiler-free adapter suite: 34 passed, 0 failed, 14 ignored /48.
No new native Promise integration pass, unchanged Deno pass or benchmark.
Earlier Deno evidence remains main/side 1/1, lazy/missing script 2/2 and WebIDL
17/17, subsets of 431 tests, before this checkpoint.

Existing Context artifacts lack the three new Promise helper exports:
`/private/tmp/deno-call-order-native.dBQZ3T/deno-core.cwasm` and
`/private/tmp/deno-reentrant-script.MZdH2Q/context-fixtures/context.cwasm`.
No replacement artifact was produced. Rebuild before claiming native ABI proof.

## Next steps

1. Audit failure rollback. Bindings are published before settled payload
   conversion; unsupported payloads may leave partial mirrors. Native state
   is also settled before synchronization can fail. Define cleanup explicitly.
2. Reuse `build-namespace-test-context.mjs ... --rejection-events`, with the
   updated bridge and real rejection dispatcher. There is no compiler option
   `standalonePromiseRejectNotifyImport`; the redundant unrun builder using
   it was removed. Optimize with Binaryen 125 and precompile with Wasmtime 47.0.3.
3. Publish test Context owner identity before mirror use. Production graph
   publication already does this. Add native controls for pending/pre-settled
   transport, exact payload identity, callback order, duplicate settlement,
   one unhandled rejection and late-handler notification. Verify zero runtime
   compiler calls and interpreter instances.
4. Rebuild a clean pinned full Deno Context. Replay unchanged missing-graph
   rejection and positive main/side tests. Never modify Deno test sources.
5. Continue original AOT thrown-payload rooting, full population/module coverage,
   snapshots, host services, shared libraries and matched V8/QuickJS/Porffor
   measurements. Full Deno integration remains unfinished.

Typst is unavailable; site rendering is unverified. Preserve adapter `.tmp/`
and unrelated compiler worktree changes. Earlier detailed artifact receipts
and commands are retained in `SCRIPT-ENVIRONMENT-HANDOFF.md` and compiler issue
4376, "Spike v8x as a rusty_v8-compatible js2wasm backend for a compiler-free Deno runtime".
