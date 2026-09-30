# Allocation-owner adapter checkpoint

Runtime artifacts now opt into compiler allocation stamps. A fresh immutable
token identifies the allocating Wasm instance, independently of handle rooting
and canonical Wasm type identity. The core instance exports its ownership
predicate and `Reflect.get` entrypoint for separately compiled application
graphs. Context import validation admits those two functions.

The compiler pin is `cc835a68c8c02255569b72271dd2527e076351c6`.
It includes the imported-binding `typeof` fix and the merge of upstream main
at `1df04af5b77a7867c418df02e6ecd4beee33ec48`. Focused compiler controls
pass 5/5 after that merge. Clean artifacts for this new pin are not yet verified;
the measurements below describe the previous build.
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

```sh
node --experimental-wasm-exnref --import "$JS2_CHECKOUT/node_modules/tsx/dist/loader.mjs" tools/js2wasm/test-context-value-bridge.mjs "$JS2_CHECKOUT" .tmp/owned-values-context.wasm --allocation-owner
V8X_JS2WASM_CONTEXT_VALUES_WASM=.tmp/owned-values-context.wasm cargo test --no-default-features --features js2wasm_deno_poc,js2wasm_gc_copying,simdutf,js2wasm_runtime_compile --test js2wasm_spike transfers_context_values_through_embedded_wasmtime -- --ignored --exact --nocapture
```
