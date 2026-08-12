# V8, QuickJS, and js2wasm engine comparison

This benchmark runs the same small ES module on three backends:

- real V8 through the official `rusty_v8` 149.4.0 crate;
- QuickJS-ng through v8x's rusty_v8-shaped API; and
- typed TypeScript compiled ahead of time by js2wasm, then loaded by the
  compiler-free v8x/Wasmtime runtime.

Every reported median includes both the absolute value and a value relative to
V8. Footprint ratios divide the engine result by V8, so lower is smaller. Speed
ratios divide V8's elapsed time by the engine result, so higher is faster.

## What it measures

The footprint/startup process creates and retains multiple isolates, compiles
or loads the module, instantiates it, evaluates it, and samples memory after 1,
10, and N live instances. It reports:

- stripped linked benchmark executable size;
- the js2wasm `.cwasm` artifact and combined payload size;
- one live instance's RSS, including shared engine initialization;
- steady RSS and virtual-address-space slopes from instance 10 through N; and
- steady module/isolate creation time.

The separate warm-speed process initializes one instance, warms every engine,
then measures:

- 200,000 calls to a trivial exported numeric function, which emphasizes the
  native host-to-engine call boundary; and
- 10,000,000 iterations of the same numeric loop, which emphasizes generated
  or interpreted code execution.

All engines must return the same checked results. Speed excludes source/AOT
compilation and isolate creation. js2wasm artifact generation is a build-time
step and is also excluded.

The boundary path is not identical: V8 uses rusty_v8, QuickJS uses v8x's
rusty_v8-shaped API, and js2wasm uses a benchmark-only typed Wasmtime export
because the experimental js2wasm backend does not yet expose module namespace
functions through the rusty_v8 ABI. The numeric kernel makes the one-time
export lookup negligible, but these remain engine microbenchmarks rather than
full Deno application benchmarks.

## Run

Initialize the QuickJS, WAMR, and rusty_v8 submodules first. Then point the
development-only artifact-generation step at a js2wasm checkout. Process-memory
and stripping commands currently support macOS and Linux:

```sh
V8X_JS2WASM_COMPILER_SCRIPT=/path/to/js2wasm/examples/v8x-js2wasm-spike/compile-graph.ts \
V8X_JS2WASM_WORKDIR=/path/to/js2wasm \
V8X_BENCH_INSTANCES=100 \
V8X_BENCH_REPEATS=5 \
benchmarks/run-engine-comparison.sh
```

The runner produces a fresh target-specific Wasmtime-precompiled artifact with
the development-only Cranelift feature, then rebuilds the measured js2wasm
runtime without Cranelift or the js2wasm compiler. All three engines use
separate Cargo target directories and fresh processes; run order rotates
between repetitions. The official V8 baseline is pinned by
`benchmarks/v8-baseline/Cargo.lock`.

Raw output, stripped executables, and a generated Markdown summary go under
`target/engine-comparison/`. To reuse a previously generated artifact, set
`V8X_JS2WASM_AOT_MODULE` instead of `V8X_JS2WASM_COMPILER_SCRIPT`. Precompiled
artifacts are unsafe to load from untrusted sources and must match the Wasmtime
version, target, and engine configuration used by this checkout.

## Scope

This is deliberately **not a full Deno benchmark**. QuickJS can boot the Deno
runtime through v8x today; the experimental js2wasm backend currently exposes
only its first typed host seam and cannot bootstrap Deno. Comparing full Deno
on V8/QuickJS against this narrow js2wasm slice would charge Deno's APIs and
JavaScript wrappers only to the interpreter-backed engines.

## Recorded runs

- [2026-08-12, Apple M4/macOS arm64](results/2026-08-12-macos-arm64.md)
