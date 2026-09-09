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

Run the generator tests with DENO_HELLO_WORLD_SOURCE pointing to the pinned
`libs/core/examples/hello_world.rs`, then:

```
node --test tools/js2wasm/test-aot-hello-world.mjs
```
