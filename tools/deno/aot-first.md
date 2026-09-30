# Closed-world AOT Deno example

For the faster measured collector, build the optional `js2wasm_gc_copying`
feature and select `V8X_JS2WASM_GC_COLLECTOR=copying` for precompile and replay.
See [collector comparison](results/2026-09-09-copying-processes.md). Existing
DRC artifacts remain supported by default; collector variants are not interchangeable.

Build with `--profile=runtime --execution=aot`, omitting `--provider-out`.
The builder extracts and hash-verifies the exact upstream hello_world script,
compiles its body into the main core artifact, and stores the expected source
for native execute_script dispatch. No hand-written replacement sum/print code
is used. Core wrappers remain staged around Deno's native op registration.

This is a bounded closed-world program, **not general classic-script compilation**.
The reviewed script has undefined completion and no subsequent observer of its
top-level bindings. It is function-wrapped for AOT staging. Unknown source and
repeat execution are refused, rather than pretending to preserve arbitrary
global-script declaration semantics. A general source registry needs proper
global lexical environments and completion handling before relaxing this scope.

The core owns Symbol state. Generic dynamic-call branches resolve to the
compiler's existing explicit-refusal source inside the same artifact. That
source has no parser or interpreter. Actual dynamic evaluation fails explicitly.
The artifact census permits only native `v8x:deno` function imports and fails
on any residual runtime-eval or other dependency. Schema2 AOT provenance records
`runtime_eval_provider: null`; it is distinct from the frozen schema1 POC lock.

The legacy dynamic mode remains unchanged. It still builds the old interpreter;
connecting the existing QuickJS eval provider to the native loader is a separate
follow-up, not implemented by this AOT slice. No QuickJS speedup is claimed.

Validation so far: four source-generator tests pass, including exact-source
execution/output against a JavaScript control, unknown-source rejection without
side effects, repeated-execution rejection, and changed build-source rejection.
The full AOT core builds and its isolated module-initialization check passes with
16 native host imports, no interpreter import, and no linear memory. Native
replay now passes with exact output and neither provider variable configured.
Both compiled Wasm tests pass, including unknown-source refusal with zero host
op calls. All31/31 measured runs pass. See results/2026-09-09-aot-processes.md.

On 2026-09-30 the clean detached runtime build was verified at compiler
`54eaa2239acd5eb1f383a500bd4d4a3b9dbdb3b2` and runtime
`59ec036ed4ee1f5ef6d3beb3fd03a57c187a53bf`. It emits 2,666,543 bytes with
17 native imports, including the enqueue notification for shared native and
compiled microtask ordering. Its SHA-256 is
`2376bb786df65caa199091ae6b32ff0ff992ae579324e0b0ed25006d747a327d`.
The compiler pin advances independently of the historical POC. This verifies
the raw artifact build, not a complete distribution or full Deno conformance.
Fresh Wasmtime 47.0.3 precompilation of this output and its paired attestation
also pass. Compiler-free replay passes 24/24 core fixture tests with three
explicit ignores and no interpreter provider configured. The source-namespace
multi-graph fixture separately passes using its existing trusted artifacts;
those graphs were not part of this clean package build.

Run the generator tests with DENO_HELLO_WORLD_SOURCE pointing to the pinned
`libs/core/examples/hello_world.rs`, then:

```
node --test tools/js2wasm/test-aot-hello-world.mjs
```
