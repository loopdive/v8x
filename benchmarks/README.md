# V8, QuickJS, and js2wasm engine comparison

This benchmark runs the same small ES module graph on three backends:

- real V8 through the official `rusty_v8` 149.4.0 crate;
- QuickJS-ng through v8x's rusty_v8-shaped API; and
- typed TypeScript compiled ahead of time by js2wasm, then loaded by the
  compiler-free v8x/Wasmtime runtime.

Every reported median shows the absolute V8 result. The QuickJS and js2wasm
cells put their factor relative to V8 in parentheses after the absolute value,
with the direction stated as smaller/larger or faster/slower. Displayed
measurements and factors are rounded to one decimal place; the raw measurements
used to compute them retain their full precision.

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

- 200,000 calls to a runtime-input adaptation of Deno's `add_js` benchmark;
- 200,000 calls to its original constant-input `addJS(1, 2)` shape, which the
  js2wasm O4 build evaluates ahead of time; and
- 20,000 calls to a 512-round input-dependent kernel with arithmetic, modulo,
  and branching, which cannot be evaluated ahead of time because its seed
  comes from the host.

The `add_js` workload comes from
[`denoland/deno/tests/bench/deno_common.js`](https://github.com/denoland/deno/blob/1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44/tests/bench/deno_common.js#L8-L12).
Every engine must return the same checked results: 42 for the runtime-input
add, 3 for the constant add, and 786,699 for the complex kernel. Speed excludes
source/AOT compilation and isolate creation.

The js2wasm artifact is compiled with optimization level 4. Its compiler first
pre-evaluates proven closed calls, then runs Binaryen `wasm-opt -O4`. The
compiler adapter fails if `wasm-opt` is unavailable or rejects the module, so
the benchmark cannot silently accept an unoptimized artifact. The preserved
raw optimized Wasm makes the distinction inspectable: the constant export is a
literal `3`, while the runtime-input add and the complete mixed loop remain.

The boundary path is not identical: V8 uses rusty_v8, QuickJS uses v8x's
rusty_v8-shaped API, and js2wasm uses a benchmark-only typed Wasmtime export
because the experimental js2wasm backend does not yet expose module namespace
functions through the rusty_v8 ABI. These remain engine microbenchmarks rather
than full Deno application benchmarks.

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

The runner produces optimized raw Wasm through js2wasm and `wasm-opt -O4`, then
creates a target-specific Wasmtime-precompiled artifact with the
development-only Cranelift feature. It rebuilds the measured js2wasm runtime
without Cranelift, Binaryen, or the js2wasm compiler. All three engines use
separate Cargo target directories and fresh processes; run order rotates
between repetitions. The official V8 baseline is pinned by
`benchmarks/v8-baseline/Cargo.lock`.

Raw output, the optimized `.wasm`, the precompiled `.cwasm`, stripped
executables, and a generated Markdown summary go under
`target/engine-comparison/`. To reuse a previously generated artifact, set
`V8X_JS2WASM_AOT_MODULE` and attest its optimization level with
`V8X_JS2WASM_AOT_OPTIMIZE=4`. Precompiled artifacts are unsafe to load from
untrusted sources and must match the Wasmtime version, target, and engine
configuration used by this checkout.

## Scope

This is deliberately **not a full Deno benchmark**. QuickJS can boot the Deno
runtime through v8x today; the experimental js2wasm backend currently exposes
only its first typed host seam and cannot bootstrap Deno. Comparing full Deno
on V8/QuickJS against this narrow js2wasm slice would charge Deno's APIs and
JavaScript wrappers only to the interpreter-backed engines.

## Recorded runs

- [2026-08-12, Apple M4/macOS arm64](results/2026-08-12-macos-arm64.md)
