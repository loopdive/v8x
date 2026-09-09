# Deno core fresh-process comparison

7 runs per engine, rotated order; every run must exit 0 with the exact upstream example output. Medians below.

| Metric                   |       V8 |                 QuickJS |                    js2wasm |
| ------------------------ | -------: | ----------------------: | -------------------------: |
| Deployment payload       | 37.3 MiB |  5.4 MiB (7.0× smaller) |   546.1 MiB (14.7× larger) |
| Peak process RSS         | 20.2 MiB | 17.6 MiB (1.1× smaller) |   494.3 MiB (24.5× larger) |
| Start, run example, exit |  57.7 ms |   44.8 ms (1.3× faster) | 13482.2 ms (233.6× slower) |

Payload is the stripped executable plus required js2wasm core/provider precompiled artifacts. Peak RSS is whole-process high-water memory, not additional-instance memory. Elapsed time includes process launch, initialization, the upstream op/printing/error workload, and exit; it excludes all build/AOT compilation. Filesystem caches are not flushed. This is not warm-kernel throughput or a multi-tenant density measurement.

Scope: pinned deno_core hello_world, not the full Deno CLI/API surface. Detailed inputs, hashes, per-run outputs and timings are retained in the linked raw JSON and local logs.

## Interpretation

This full-core checkpoint is much larger and slower than the earlier engine-only benchmark. These are actual measurements, not an estimate of what an optimized Deno integration could achieve. The js2wasm deployment comprises **2.3 MiB executable + 117.7 MiB core artifact + 426.2 MiB interpreter artifact**. No compression or artifact-format stripping was applied; the source Wasm was optimized with Binaryen before Wasmtime precompilation.

Timings vary substantially: V8 9.0–432.6 ms, QuickJS 25.7–193.1 ms, js2wasm 13,101.3–16,468.5 ms. These seven-run medians are ballpark process measurements, not precise steady-state engine-speed ratios. The first exploratory five-run set overlapped publication checks and is not used here. The reported set ran after this task's builds, optimizers and push checks had stopped.

The workload creates one `deno_core::JsRuntime`, runs the upstream `hello_world` source, sums an array through a Rust op, prints the result, handles its intentional invalid-argument exception, and exits. It does not measure warm calls, async completion, the full Deno CLI or additional-instance memory. The js2wasm runtime is compiler-free but still uses the diagnostic fail-loud ABI feature; unsupported APIs are not represented by this example.

## Inputs and reproduction

- Apple M4, macOS 26.6.2 (25G83), Rust 1.95.0.
- Deno `1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44`, deno_core 0.407.0, rusty_v8 149.4.0.
- v8x `0eaa1d8cb4aaeeb3ce159951e1bc1e652f36aecf`; QuickJS submodule `034f2aba47ab6e7aacd869af7aa796b110d0ea2e`.
- js2wasm `bda15bdf70baefc3d7620f32a03dc3660c2fd005`, Wasmtime 47.0.3, Binaryen 125.
- Build identical pristine Deno source with `cargo build -p deno_core --example hello_world --release`, using separate Cargo target directories. V8 uses the unchanged dependency; QuickJS selects v8x features `simdutf,quickjs`; js2wasm selects `simdutf,engine_js2wasm,js2wasm_diagnostic_abi`.
- The core/provider pair was built using `tools/js2wasm/build-deno-core-artifact.mjs --profile=runtime`, optimized with `wasm-opt --no-inline -O3 --pass-arg=no-inline@__new_* --all-features --disable-custom-descriptors -g`, then precompiled with the runtime's exact-artifact helpers.
- No Deno `libs/core` source changes. The example output is checked against `tools/deno/js2wasm-poc-expected.stdout`.

Run the committed harness after building those binaries and artifacts:

```sh
DENO_BENCH_REPEATS=7 \
DENO_BENCH_OUTPUT=/path/to/new-results \
DENO_BENCH_V8=/path/to/v8/hello_world \
DENO_BENCH_QUICKJS=/path/to/quickjs/hello_world \
DENO_BENCH_JS2WASM=/path/to/js2wasm/hello_world \
DENO_BENCH_CORE_AOT=/path/to/core-O3.cwasm \
DENO_BENCH_PROVIDER_AOT=/path/to/provider-O3.cwasm \
node tools/deno/measure-core-processes.mjs
```

[Raw measurements and SHA-256 inputs](2026-09-09-core-processes.json). Original stdout/stderr, stripped executables and the exploratory run remain under `/private/tmp/deno-comparison.WyCNkv` on the measurement host. No executable artifacts are committed.

Compiler checkpoint PR: https://github.com/loopdive/js2/pull/5784  
Runtime PR: https://github.com/loopdive/v8x/pull/2
