# QuickJS and js2wasm engine comparison

This benchmark compares the two v8x backends at the smallest common layer:
one process creates and retains multiple rusty_v8-shaped isolates, executes a
semantically identical 32-iteration program (JavaScript in QuickJS, typed
TypeScript compiled ahead for js2wasm), and measures the process after 1, 10,
and N live instances.

It reports:

- the stripped linked benchmark executable size;
- the js2wasm `.cwasm` artifact and combined benchmark payload size;
- the first live instance's RSS and virtual-address-space cost;
- the steady per-instance RSS and virtual-address-space slope from instance 10
  through N; and
- total and steady instance-creation time.

This is deliberately **not a full Deno benchmark**. QuickJS can boot the Deno
runtime through v8x today; the experimental js2wasm backend currently exposes
only its first typed host seam and cannot bootstrap Deno. Comparing full Deno
QuickJS against this narrow js2wasm slice would attribute all of Deno's APIs and
JavaScript wrappers to the QuickJS engine.

## Run

Initialize the QuickJS, WAMR, and rusty_v8 submodules first. Then point the
development-only artifact generation step at a js2wasm checkout. The process
memory and stripping commands currently support macOS and Linux:

```sh
V8X_JS2WASM_COMPILER_SCRIPT=/path/to/js2wasm/examples/v8x-js2wasm-spike/compile-graph.ts \
V8X_JS2WASM_WORKDIR=/path/to/js2wasm \
V8X_BENCH_INSTANCES=100 \
V8X_BENCH_REPEATS=3 \
benchmarks/run-engine-comparison.sh
```

The runner first produces a trusted, target-specific Wasmtime-precompiled
artifact with the development-only Cranelift feature. It then builds and
measures a fresh `engine_js2wasm` runtime without Cranelift or the js2wasm
compiler. QuickJS and js2wasm use separate Cargo target directories. Raw output
and stripped binaries go under `target/engine-comparison/`.

To reuse a previously generated artifact, set `V8X_JS2WASM_AOT_MODULE` instead
of `V8X_JS2WASM_COMPILER_SCRIPT`. Precompiled artifacts are unsafe to load from
untrusted sources and must match the Wasmtime version, target, and engine
configuration used by this checkout.

## Recorded runs

- [2026-08-12, Apple M4/macOS arm64](results/2026-08-12-macos-arm64.md)
