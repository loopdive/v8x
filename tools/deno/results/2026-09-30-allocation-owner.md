# Allocation-owner adapter checkpoint

Runtime artifacts now opt into compiler allocation stamps. A fresh immutable
token identifies the allocating Wasm instance, independently of handle rooting
and canonical Wasm type identity. The core instance exports its ownership
predicate and `Reflect.get` entrypoint for separately compiled application
graphs. Context import validation admits those two functions.

The current compiler pin is `10588f480b59bb19ae72a8d6bdedf641a4301915`.
It adds receiver-backed live iteration, with 118/118 focused compiler checks.
Native replay of that repair is pending the clean artifact rebuild.
The previous compiler pin was `cc835a68c8c02255569b72271dd2527e076351c6`.
It includes the imported-binding `typeof` fix and the merge of upstream main
at `1df04af5b77a7867c418df02e6ecd4beee33ec48`. Focused compiler controls
pass 5/5 after that merge. Another 15/15 allocation-owner, realm and namespace
controls pass. The original measurements below describe the previous build;
the clean refresh is recorded separately below.
The historical POC compiler/options commitment remains unchanged. AOT runtime
packaging still emits no interpreter; explicitly requested dynamic providers
receive compatible stamped layouts.

Native string and byte-vector readers select suffix field indices from the
generated `__v8x_context_owns` export. Both root types retain length at field 0
and insert their token at field 1. Flat/rope string fields move to 2/3 and
vector data moves to 2. Historical unstamped modules retain the old indices.

Verified locally with Wasmtime 47.0.3, copying GC, macOS aarch64:

- Runtime compile-options tests: 9/9.
- Full stamped context-value fixture: Node assertions pass.
- `transfers_context_values_through_embedded_wasmtime`: 1/1, including ordered
  packets, UTF-16 slices/ropes/deep concatenation and values after moving GC.
- `cargo fmt --check`: passes.

The native check compiled raw Wasm with `js2wasm_runtime_compile` enabled.
It does not establish compiler-free replay or full Deno conformance. Clean
core and application artifacts must be rebuilt together. The application
check must assert successful evaluation, not merely an upstream test whose
evaluation future is dropped. The compiler graph builder must also opt into
the same stamp and owner-import options before packaging linked graphs.

Reproduction uses the compiler checkout's tsx loader:

## Actual application-import verification

The core-routing regression now evaluates a separately compiled application
that imports `core`, `internals` and `primordials`, checks their presence with
`typeof`, and exports the result of `ArrayPrototypeReduce([1,2,3], ...)`.
Evaluation must fulfill and the exported answer must equal 6. The interpreter
is not linked. A rejected application does not count as successful core boot.

The first run failed despite owner-aware getters returning the correct
objects. The compiler incorrectly treated imported TypeScript alias symbols
without a `valueDeclaration` as undeclared in both `typeof` lowering forms.
Runtime imports must remain bound; erased type-only imports must not.

After that compiler fix, native build-time packaging passes 1/1 and the
compiler-free test binary passes 30/30 with six ignored, including the actual
core-import application test (1.78 seconds for the complete bounded run).
Compiler environment variables were removed for replay. Core uses clean
compiler revision `694a8a51df50aef18cf2747acd8020b7024af574`; the application
graph was generated from the current compiler working tree with the `typeof`
fix, not a clean release pin. Source and artifact hashes are checked by the
existing graph sidecar before loading its trusted precompiled module.

The clean stamped core artifact is in
`/private/tmp/deno-owner-release-build.tyjx9A`. Raw Wasm is 2,770,538 bytes,
SHA-256 `ccba4931c9aec828f2437929cb83e2964d85b796715cbf1a1b76eff97d3809c9`.
Precompiled Wasmtime output is 52,016,768 bytes, SHA-256
`e04081dd01a7a7a9358eb73b3f68e77e2aa271ae494c918b3f4616ee8f444dc2`.
Precompilation passed 1/1 in 285.87 seconds on the local machine. These are
artifact sizes, not per-instance memory measurements.

Clean packaging of the fixed application compiler, the full unchanged Deno
test harness, general classic Script execution and performance measurements
remain open. No Deno source or vendored upstream tests were changed.

## Clean fixed-compiler refresh

Clean detached inputs in `/private/tmp/deno-import-release-build.AWkP87`:
compiler `cc835a68c8c02255569b72271dd2527e076351c6`, adapter
`a4748fd143971462ffdfb274e2045ef680e8d9cb`, Deno
`1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44`. The compiler's subsequent
remote-merge reconciliation `c6fc8aa81fa4dbfb418d5a986faebcceb0f10e90`
has the identical Git tree, `e76a5a4f9c7a38f36d633ab11ec039bf70350b7f`.

The runtime-profile AOT build emits no runtime-eval provider. Its raw core
is 2,781,977 bytes, SHA-256
`44ac7273811b24b3428b5168ba775e7707a3478e259160f8b5daf919d8c89251`.
Wasmtime precompilation passes 1/1 in 306.70 seconds; output is 52,197,048
bytes, SHA-256
`397aa2387ae42b241e881705b8450a2a87ed4677af58df095907ac7caa7c9f92`.
The accompanying attestation binds both hashes, the engine configuration,
Wasmtime 47.0.3 and the aarch64-apple-darwin target.

Application packaging using that same clean compiler passes 1/1 in 27.27
seconds. Its source-bound graph is stored under `module-graphs/` with its
graph-hash sidecar. Replaying the compiler-free binary
`js2wasm_spike-a7f0ba6d34fa178f` against these fresh artifacts, with compiler,
compiler-script, compiler-ID, workdir and compiler-cache environment variables
removed, passes 30/30 with six ignored and zero filtered in 1.72 seconds.
The core-import application explicitly requires a fulfilled evaluation promise
and exported answer 6. Ignored tests are not credited. This remains a bounded
adapter suite, not the full unchanged Deno harness or complete Deno integration.

## Unchanged core sweep and WebIDL checkpoint

The compiler-free unchanged Deno core population contains 431 tests, including
two upstream ignored tests. The baseline Nextest sweep against the clean core
artifacts completed 211 passing and 216 failing tests. Two lazy TCP-driver
tests remain running after sandbox denial of local sockets; this is not a
completed full-suite result. All six TCP-driver cases pass when rerun with
local networking permitted. No test process was stopped.

Failure text in the baseline includes 147 unknown-classic-script refusals and
32 missing application-graph binding errors. These signatures identify where
execution stopped, not independent root-cause populations. Snapshot and
inspector APIs also remain unimplemented. General Script execution remains
necessary for full integration; accepting arbitrary source or dropping its
semantics is not a fix.

Native NumberValue now shares ToNumber conversion with IntegerValue, using
the existing compiled realm for strings and objects. Primitive controls cover
NaN, negative zero, booleans, null and Symbol/BigInt rejection. Native arrays
are lazily adopted into the supplied context's realm on intrinsic iterator
lookup, retaining the native object binding and respecting an explicit
iterator property instead of implementing a second iterator in Rust.

After these adapter changes, unchanged `webidl::tests::integers`, `sequence`
and `constrained_sequence_one_of` each pass 1/1; all three failed before the
changes. The whole unchanged WebIDL population passes 13/17. The remaining
four stop at unknown-classic-script refusal, before their intended assertion.
No Deno source or tests were changed.

The initial bounded adapter run passed 31/31, with six ignored. A strengthened
live-iteration control exposes an additional defect and now yields 30 passing,
one failing and six ignored. Reading index 1 immediately after an adopted
array's SetIndex correctly returns 3, but the already-created iterator returns
the old 2. The failing assertion is deliberately retained, not weakened or
ignored. The compiler's iterator normalization contains snapshot-copy paths
in `src/codegen/iterator-native.ts`; attribution to the precise carrier path
still requires a compiler-level control. This checkpoint is not merge-ready.
Next work is to preserve the actual iterated receiver and read its current
elements on each step, then rebuild the clean AOT artifacts and rerun both
adapter and unchanged upstream controls. No interpreter was added.

Current replay inputs are the clean artifact directory above, adapter test
binary `js2wasm_spike-8b524eb9ef0b52c1` built with no default features and
`js2wasm_deno_poc,js2wasm_gc_copying,js2wasm_diagnostic_abi`, and unchanged
Deno binary `deno_core-87206ac56a2fccad`, built with
`RUSTFLAGS='--cfg tokio_unstable'`. Artifact environment variables select the
attested core, pinned fixtures and source-bound module-graph directory; no
runtime compiler feature is enabled.

```sh
node --experimental-wasm-exnref --import "$JS2_CHECKOUT/node_modules/tsx/dist/loader.mjs" tools/js2wasm/test-context-value-bridge.mjs "$JS2_CHECKOUT" .tmp/owned-values-context.wasm --allocation-owner
V8X_JS2WASM_CONTEXT_VALUES_WASM=.tmp/owned-values-context.wasm cargo test --no-default-features --features js2wasm_deno_poc,js2wasm_gc_copying,simdutf,js2wasm_runtime_compile --test js2wasm_spike transfers_context_values_through_embedded_wasmtime -- --ignored --exact --nocapture
```
