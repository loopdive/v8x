# Native Promise transport checkpoint, 2026-10-04

## Latest module linking checkpoint: incomplete

The new instance-bound `__v8x_module_namespace_*` imports retrieve evaluated
native Module namespaces with Context/owner checks. The compiler guards legacy
per-source initialization and redirects live imported reads. Native identity,
once-only execution and later named/namespace reads now pass the preceding
assertions. The expanded test still fails **0/1, 51 filtered /52**:
`mutateNamespace` returns **1 instead of 4**. Later named-call, receiver and
same-URL distinct native Module assertions are not reached. Do not suppress this
failure or report the entire shared-module control passing.

Current Node V8 fixture control passes **1/1**, including mutation and receiver
checks. Scoped ordinary adapter run passes **34/34 executed**, 17 ignored,
1 environment-dependent core Script test filtered /52. Cargo formatting passes.
No new unchanged Deno or comparative benchmark result is claimed.

Development graph directory is `/private/tmp/deno-module-linking.pmIWQ4`, built
from the dirty compiler candidate with Binaryen 125 O3 /Wasmtime 47.0.3, not a
clean published compiler pin. Current first/second graph binding digests are
`16173d5026e480a4bd48ea39dc737035903323c69199ae9013700996314520f0` and
`c10d47605cf2ed1389e52c98fc37b1fda72e7c86601eacc166b9a7ccb9dbbea5`.

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-linking.pmIWQ4 target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
node --experimental-vm-modules --test tools/js2wasm/test-shared-module-fixtures.mjs
```

Resume with callable owner routing in compiler `expressions/calls.ts` and
adapter `js2wasm_graph_calls.rs`, then replay all expanded assertions. Compiler
prepared IR M2 initialization is not guarded yet. Malformed bindings,
wrong-Context, cycles/TDZ and failed evaluation need tests. Rebuild clean pinned
packages and the unchanged Deno runner before replaying builtin core import.
The last Deno runner build failed linking (exit 101); the old binary does not
contain this adapter. Inspect linker configuration rather than editing Deno.

Detailed paired handoff:
https://github.com/loopdive/js2/blob/codex/4376-deno-lexical-checkpoint-20261004/plan/agent-context/4376-module-linking-checkpoint-2026-10-04.md
Both existing PRs remain incomplete drafts. The records below are historical.

## Shared dependency regression control

### Within-graph identity fix verified natively

Compiler 1c2f7c35fd3c92cddabd484a07d4881f81ac3446 replaces declaration-keyed
namespace caches with canonical module-symbol caches and handles named imports
of namespace re-exports through that getter. Compiler namespace controls pass
6/6, including gc and standalone identity/liveness checks. Initial six-file run
reports 25 passed /1 failed /26; the same standalone TypeScript namespace
Hole-global failure reproduces on clean ba14fcaedb (7/8) with unchanged tests
and the same harness. No full regression-suite pass is claimed.

The native test now first checks the entry's exported namespace against its
dependency's native namespace. Old packages fail that first check (0/1).
Fresh clean compiler packages make it pass, then retain the measured
cross-graph failure: executions=2, observed=1, same namespace=false, rejected.
Complete regression still fails 0/1, 51 filtered /52; no new Deno test passes.

New package directory: /private/tmp/deno-shared-module-fixed.MBUdR1. Sources,
graph digests, Binaryen/Wasmtime versions and small Context are unchanged.
First native SHA256:
77c6c4c04b5d3ab628eda33fc557329ecdba32c677c1ddc88cca5fc9d7a5d396.
Second native SHA256:
840a62e53aeb58aadca69c58c76a98ed7d7d1c58fd098dc323f50ba6c9cc007c.
Replay the command below with V8X_JS2WASM_AOT_GRAPH_DIR set to that directory.
The clean staged compiler checkout is now 1c2f7c35fd, not ba14fcaedb; historical
artifact build pins remain unchanged. Next implement cross-graph native Module
reuse and live imports together with once-only dependency evaluation.

The generic two-entry control fails **0 passed /1 failed, 51 filtered /52**.
The first entry succeeds and bumps the dependency's mutable count to 2.
Evaluating the second entry then reports executions=2 (expected 1), observed
count=1 (expected 2), different JavaScript namespace identity and a rejected
evaluation Promise. Runtime compilation and interpreter counters remain zero.
This is a semantic defect, not only a namespace publication guard. Do not fix
it by ignoring the guard or aliasing only native wrappers.

The exact same three fixture sources pass **1/1** under Node's V8 module
evaluator. That control also bumps the original dependency again and verifies
both named imports and namespace reads see 3. Those later assertions exist in
the native regression but are not yet reached because the earlier check fails.
This Node control is not a full Deno/V8 comparison or performance benchmark.

Fixtures: tests/fixtures/js2wasm-shared-module/*.js. Rust uses include_str!;
build-shared-module-test-packages.mjs reads the same bytes. Compiler pin
ba14fcaedb, Binaryen 125 O3, Wasmtime 47.0.3; packages are in
/private/tmp/deno-shared-module.SpmGZO. First graph binding is
a43c82a9e3b52c85ddcdbd5f819ca5fd578a0086f11f92256c303661c491df38;
second is 16d940d5d9aa117ff7e1b46c01a4171db60284d446409e562feb40c2faca7da7.
Native hashes and original bytes are recorded in package JSON inventories.

```sh
node --experimental-vm-modules --test tools/js2wasm/test-shared-module-fixtures.mjs
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-shared-module.SpmGZO target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
```

Compiler-free test binary built with js2wasm_deno_poc, js2wasm_gc_copying and
js2wasm_diagnostic_abi, without runtime_compile. Existing positive/throwing AOT
module controls still pass 2/2, 50 filtered /52. Scoped ordinary controls pass
34/34 executed, 17 ignored and 1 filtered /52. The filtered original-core
Script test requires a separate fixtures environment: an unconfigured full
run reports 34 passed /1 failed /17 ignored, not an all-green suite.
Existing extractor/graph/Script controls pass 10/10. No runtime fix or new
Deno pass is credited to this test-only change. Typst remains unavailable.

Next implementation must couple three capabilities: skip already evaluated
dependency initializers, return their canonical namespace by captured native
Module identity, and route all named import reads/calls to original live
bindings. NativeModuleGraph already provides instance-bound Module/Context
identity for import-meta; extend that pattern rather than URL-global state.
Changing only namespace materialization leaves named imports on duplicate
globals. Changing only the initializer adapter leaves uninitialized globals.
Preserve cycles, early TDZ, same-URL distinct Modules and failed evaluations.

## Broader unchanged module population

Selected run: **4 passed /1 failed out of 5**, not full Deno integration.
All runs used unchanged Deno 1d4e6c1, compiler-free binary
`deno_core-87206ac56a2fccad`, fresh full Context from the section below and
new exact-source packages in `/private/tmp/deno-module-population.SdW8YU`.
Metadata resolve passes 1/1 (1.79s); filename/dirname 1/1 (1.54s); repeated
evaluation async 1/1 (1.51s) and sync 1/1 (1.64s). Each has 430 filtered /431.
Removing only the assertion Script directory makes async repeated evaluation
fail 0/1 at check1 with an unknown-Script refusal (1.56s). Assertions are real.

`builtin_core_module` fails 0/1 (1.79s) with its full package installed:
"source module namespace was already bound to another value". The source graph
publishes a second core namespace although the native core Module is already
bound to its original Context namespace. Do not bypass this conflict. The next
implementation must reuse canonical native Module namespaces in linked graphs,
retain live exports/import-star identity, and avoid reevaluating already executed
dependency bodies. Use native Module identity, not URL-only lookup. Cover shared
dependencies across entries, same-URL separate Modules and cycles/early access.

Build-side tools added: `deno-module-fixtures.mjs`,
`build-deno-module-test-packages.mjs`, `test-deno-module-fixtures.mjs`.
Extractor reads pinned original Rust source, preserving literal whitespace and
rejecting missing/duplicate declarations, comments and unsupported layouts.
Five graph packages plus four assertion Scripts are built with clean detached
compiler ba14fcaedb, Binaryen 125 O3 and Wasmtime 47.0.3. No source/test edits,
runtime compilation or interpreter are used. Extractor/graph/Script controls
pass 10/10. Native artifact SHA256 are in per-package JSON inventories.

Rebuild from compiler cwd:

```sh
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-deno-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-promise-full.X2WdwN/deno /private/tmp/deno-module-population.SdW8YU
```

Replay from the patched Deno checkout, changing only the exact test selector:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-promise-full.X2WdwN/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-module-population.SdW8YU/scripts V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-population.SdW8YU/graphs target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::builtin_core_module --nocapture --test-threads=1
```

Graph digests:

- resolve: 472642c36c38d274179b232c50ddcdded4bd885b826d62729d5f49d2d66363d6
- filename: b048e367b02baca23ae264bc3712a211ad4c91eb20c7735e495439dc09539555
- builtin core: 132783fb316229bbb401075500159535810c5bd7a00af9437a004a77823a98f9
- async repeat: b909e5ec18dc37b20d6020645f948eaf03362d36628e6acff5e0a007e041a129
- sync repeat: 18e0420a743779da09d4b909c3683328a4c57ed80b4d3f433278d483de492906

Compiler entry points: namespace getter construction in
`src/codegen/module-namespace-value.ts`, per-source initializer planning in
`module-init-collection.ts` / `multi-prepared-module-init*`. Adapter:
`collect_graph`, `publish_source_namespaces`, `bind_source_namespace` and
`bind_prelinked_core_namespace`. Preserve the refusal until identity reuse is
implemented and measured. Full population, snapshots, dynamic imports/top-level
await, remaining host/value transport and matched benchmarks remain open.

## Full Context and rooted module exceptions

The full Context was rebuilt from clean detached checkouts: compiler
`b5f6cbae636d22d5c9e7901f779b4f1727003adc`, adapter builder
`7b31b4ef839bbd4646b85441725e65c7f6a95dc6`, original Deno
`1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44`.
Directory `/private/tmp/deno-promise-full.X2WdwN` retains these checkouts,
provenance, raw/optimized/native artifacts and attestation.
Raw SHA256 `c157e9ddab1c7108d3b4c72b93bcf114063194edc260b6b8a7f11263a46cfc59`;
optimized SHA256 `6d3e47c8b926b0e43e0143812101cb8feae3cb78ab1fe39fdfb31f278dec8e79`;
native SHA256 `78aa8a50726f61577cdc54267d912af63acc7b85d24a931fa53e71a63ab2b237`,
44,646,992 bytes. Binaryen 125 O3, all features, no custom descriptors,
debug names and no-inline wildcard. Wasmtime 47.0.3 build-side precompile
passed 1/1, 73 filtered /74, 224.14s. No interpreter provider emitted.

The compiler-free Deno binary was rebuilt against the current adapter:

```sh
RUSTFLAGS='--cfg tokio_unstable' cargo test --offline -p deno_core --lib --no-run
```

Unchanged main/side passes 1/1, 430 filtered /431 (latest 1.94s).
With graph packages absent it fails 0/1 with the actual missing-artifact error
in `exception_message`, no longer an unsupported Promise conversion error.
Lazy loading passes 1/1 and WebIDL 17/17. These remain subset evidence, not
full population credit. Replay from the patched Deno checkout:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-promise-full.X2WdwN/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-reentrant-script.MZdH2Q/deno-scripts V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-promise-full.X2WdwN/graphs-owned-get target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::main_and_side_module --nocapture --test-threads=1
```

For the negative control omit only V8X_JS2WASM_AOT_GRAPH_DIR. For WebIDL use
`webidl::tests::` instead of the exact main/side selector.

`run_graph_module_init` now roots a pending externref payload in the Context
keeper before returning an error. The native module rejection captures that
handle into the original exception value. A new throwing graph test verifies
the rejected Promise result, Module exception and escaped global object have
the same identity. Initially marker reads returned NaN/42 because the namespace
matcher stays unready after an abrupt initializer. The compiler sidecar now
exports a getter for allocation-proven objects; native dispatch checks the
graph's ownership predicate before using it. Old packages with proven ownership
but no getter fail loudly. No bypass is allowed for foreign objects.

Native positive/throwing AOT module controls pass 2/2, 48 filtered /50, including
marker 42, cached evaluation Promise and zero compilation/interpreter counts.
Compiler namespace controls pass 4/4, including foreign-instance allocation
refusal with the same GC layout. TS7 passes; scoped lint retains one existing
explicit-any warning. Ordinary adapter suite 34 passed, 0 failed, 16 ignored
/50; scoped units 15/15, 16 filtered /31. Formatting and diff checks pass.

Candidate graph packages in `graphs-owned-get` were built from the working
compiler with this sidecar change, not the clean full-Context compiler pin.
Each inventory records exact source and optimized/native SHA256. The source-bound
throw digest is `053279d44c82aaa1fedc04b365481ae3cfe294b5ebf6816dc4293753f0cfa8fe`.
Native control:

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-promise-full.X2WdwN/graphs-owned-get target/debug/deps/js2wasm_spike-e7e456f13e693536 aot_module_ --ignored --nocapture --test-threads=1
```

Next derive the full unchanged test/source population, package its exact module
graphs and continue snapshots and host services. Startup-module exceptions,
arbitrary thrown callables, failure after mirror adoption and full snapshot
semantics remain unverified. No new benchmark. Typst still unavailable.

The sections below are historical and superseded where this section provides
new evidence. Neither PR is merge-ready and the full integration goal remains.

## Native validation continuation

The native mirror now has an executed compiler-free control: 1/1, 48 filtered
/49, covering fulfillment/rejection before and after transfer, exact native
Promise and payload identity, queued compiled reactions, one rejection event,
one late-handler event and repeated refusal of unsupported BigInt payloads.
Runtime compilations and interpreter instances are zero. Existing compiled
rejection controls pass 3/3, 46 filtered /49, including reentry and cross-realm
enqueue ordering. These are adapter controls, not unchanged Deno coverage.

Settled payload graphs are inspected before publication, including Promise
payload edges. Errors, buffers and supported typed arrays remain transferable.
Failed mirror initialization removes its identity binding and registry entry.
Test Context attachment now publishes runtime owner identity. Settling an
already-published mirror with an unsupported value and arbitrary Wasmtime-call
failure rollback still need broader handling; do not claim full transactionality.

Fresh small Context: `/private/tmp/deno-native-promise.6898GB/context.cwasm`,
19,883,016 bytes, SHA256
`52530264bf83966d5049409029fa4b7819dc1e881758c775fd8e3f7234e23123`.
Raw Context SHA256 `507b7f9135de83edafeb26f57e26b3dccbe9cbb55ede856415226577374896d7`;
optimized SHA256 `6f91022d2ed54e8d72074bda2e92a509cc17dccbf0b2826f75dc5b39ff1563c3`.
Built with compiler implementation 74ed7007fb and adapter bridge 830a3f8;
Binaryen 125 `-O3 --all-features --disable-custom-descriptors -g --no-inline=*`,
Wasmtime 47.0.3 build-side precompile test passed 1/1, 73 filtered /74 (108.83s).

Replay command in adapter checkout:

```sh
V8X_JS2WASM_REJECTION_CONTEXT=/private/tmp/deno-native-promise.6898GB/context.cwasm target/debug/deps/js2wasm_spike-e7e456f13e693536 --exact native_promise_mirror_retains_identity_and_single_rejection --ignored --nocapture --test-threads=1
```

Next: rebuild full clean pinned Deno Context with the new bridge, then replay
the original missing-graph rejection and positive main/side tests. The full
Context is still old. No new unchanged Deno result or performance measurement.

## Original checkpoint (historical)

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
