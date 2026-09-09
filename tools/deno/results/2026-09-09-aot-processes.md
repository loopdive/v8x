# Deno core: AOT program, no interpreter deployment

The unchanged upstream hello_world completes without an interpreter provider.
Five alternating pairs, identical native executable and compiler revision:
indexed/interpreter median863.4ms, AOT median253.3ms, **3.4× faster**. All10/10
exact stdout/exit checks pass. Raw A/B labels `scalar`=indexed/interpreter and
`bulk`=AOT; candidate provider `none` explicitly omits its environment variable.

Seven rotated fresh processes per engine also pass21/21. AppleM4, macOS26.6.2,
ARM64, Rust1.95.0. No task builds/tests/hooks overlap timing. Medians:

| Metric | V8 | QuickJS | js2wasm AOT |
| --- | ---: | ---: | ---: |
| Complete deployment | 37.3 MiB | 5.4 MiB (7.0× smaller) | 106.2 MiB (2.9× larger) |
| Peak process RSS | 20.2 MiB | 17.8 MiB (1.1× smaller) | 92.7 MiB (4.6× larger) |
| macOS physical footprint | 6.4 MiB | 10.7 MiB (1.7× larger) | 6.0 MiB (1.1× smaller) |
| Launch, example, exit | 8.3 ms | 22.6 ms (2.7× slower) | 255.2 ms (30.8× slower) |

AOT remains11.3× slower than QuickJS and19.8× larger in deployment. This is
not warm throughput or tenant-density evidence. macOS physical accounting is
not marginal tenant memory. First runs are retained outliers (530.7/296.8/985.4ms);
filesystem caches were not flushed. Ratios use unrounded values.

Deployment shrinks from indexed396.7MiB to106.2MiB (73.2% reduction).
The297.7MiB interpreter provider is absent. Remaining files: executable1.6MiB,
v8x/Wasmtime dylib6.0MiB, Rust standard library1.2MiB, compiled core97.4MiB.
Exact total111401176bytes; core102154008bytes. Core grows relative to the
previous90.2MiB core; compilation/linkage changes are combined, not isolated.
Most remaining deployment is generated core code/metadata, not the native library.

The exact pinned source is extracted from the upstream Rust example and compiled
inside the main Wasm, not replaced by a handwritten sum or printed answer.
Native op registration and calls remain. Scope is explicitly closed-world:
unknown/repeated scripts are refused. This does not implement general script
lexical environments. Generic dynamic-code branches use existing compiler
refusal code locally, with no parser/interpreter. Symbol state is local.
Import census:16native v8x:deno functions, zero other dependencies.

Validation: four generator tests; two compiled Wasm tests (census and unknown
source refusal with zero host-op calls); native replay without provider;31/31
measured runs. Binaryen125 --no-inline -O3 --pass-arg=no-inline@__new_*
--all-features --disable-custom-descriptors -g, then Wasmtime47.0.3 Cranelift speed.
Native replay does not compile. Compiler72f281a632ac2b86e6fa2aaf5b76df2a7fd7b5f4.
AOT runtime90d0fbe; indexed controlce590c9, same compiler. Deno
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44, core0.407.0, V8149.4.0,
QuickJS-ng034f2aba47ab6e7aacd869af7aa796b110d0ea2e. Identical native bundle
from comparison-bulk on both A/B sides; hashes are in the processes JSON.

Separate exact-output phase diagnostic: coreload7.6ms, instances1.5ms,
moduleinit2.0ms, core scripts31.1ms combined, usage39.8ms. These are inclusive,
potentially nested single observations, not benchmark medians. Do not sum with
enclosing native-script-run scopes. Interpreter loading is gone. Remaining
targets: op-binding setup, native/GC value transfer, generic calls and core size.

Raw stdout/counters:2026-09-09-aot-counters.json. Files/hashes and21rows:
2026-09-09-aot-processes.json. Ten paired rows:2026-09-09-aot-ab.json.
Harnesses:measure-core-processes.mjs (DENO_BENCH_NO_PROVIDER=1) and
compare-bulk-transfer.mjs (final candidate-provider argument `none`).
Local artifacts under/private/tmp/deno-profile.RCcI44: core-aot-v2{,-O3}.wasm,
core-aot-v2-speed.cwasm, aot-v2-provenance.json; outputs comparison-aot and aot-ab.
Schema2 AOT provenance is separate from the frozen schema1 POC replay lock.
