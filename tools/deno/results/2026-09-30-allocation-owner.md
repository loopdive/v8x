# Allocation-owner adapter checkpoint

Runtime artifacts now opt into compiler allocation stamps. A fresh immutable
token identifies the allocating Wasm instance, independently of handle rooting
and canonical Wasm type identity. The core instance exports its ownership
predicate and `Reflect.get` entrypoint for separately compiled application
graphs. Context import validation admits those two functions.

The compiler pin is `694a8a51df50aef18cf2747acd8020b7024af574`.
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

```sh
node --experimental-wasm-exnref --import "$JS2_CHECKOUT/node_modules/tsx/dist/loader.mjs" tools/js2wasm/test-context-value-bridge.mjs "$JS2_CHECKOUT" .tmp/owned-values-context.wasm --allocation-owner
V8X_JS2WASM_CONTEXT_VALUES_WASM=.tmp/owned-values-context.wasm cargo test --no-default-features --features js2wasm_deno_poc,js2wasm_gc_copying,simdutf,js2wasm_runtime_compile --test js2wasm_spike transfers_context_values_through_embedded_wasmtime -- --ignored --exact --nocapture
```
