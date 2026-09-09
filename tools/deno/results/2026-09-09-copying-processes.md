# AOT Deno with copying GC

Opt-in Wasmtime copying GC improves the five-pair median from **252.7 ms to
133.9 ms (1.9× faster)**. Both sides use the same new native executable and
the same optimized Wasm input; only collector selection and the necessarily
different precompiled artifact change. All 10/10 runs produce exact output.

Seven rotated fresh processes per engine also pass 21/21. Apple M4, macOS
26.6.2, ARM64, Rust 1.95.0. No task-owned compilation, tests or hooks overlap
measurements. Filesystem caches are not flushed. Medians:

| Metric | V8 | QuickJS | js2wasm copying AOT |
| --- | ---: | ---: | ---: |
| Complete deployment | 37.3 MiB | 5.4 MiB (7.0× smaller) | 55.9 MiB (1.5× larger) |
| Peak process RSS | 20.2 MiB | 17.9 MiB (1.1× smaller) | 51.9 MiB (2.6× larger) |
| macOS physical footprint | 6.4 MiB | 10.8 MiB (1.7× larger) | 5.8 MiB (1.1× smaller) |
| Launch, example, exit | 8.2 ms | 22.1 ms (2.7× slower) | 135.0 ms (16.4× slower) |

Still **6.1× slower than QuickJS**, not goal completion. This is the unchanged
Deno-core example, not warm request throughput, full Deno, or marginal tenant
memory. First launches are retained outliers: 519.4/303.8/871.6 ms. Ratios use
unrounded numbers. Physical footprint is macOS accounting, not RSS.

## Why this helps

The old Cargo feature set enabled only `gc-drc`. Wasmtime therefore selected
deferred reference counting even though its preferred collector, when available,
is copying. Copying avoids that collector's reference-counting bookkeeping and
supports cyclic collection. The measured compiled core shrinks from 97.4 MiB
to 47.1 MiB; full deployed files total 58,620,560 bytes (55.9 MiB), including
the executable and shared native libraries. The core alone is 49,348,336 bytes.
This is a real generated-code change, not stripping an unused interpreter file.

Build `js2wasm_gc_copying` and set `V8X_JS2WASM_GC_COLLECTOR=copying` for both
precompilation and replay. DRC remains the explicit default, preserving old
artifacts. The frozen POC replay configuration rejects copying. Unknown selector
values fail. Deserializing a DRC artifact into a copying engine also fails with
the collector mismatch before any example output. Both negative checks pass.
No null collector, skipped collection, or address-based identity hash is used.

Nine existing native realm tests pass with copying: identities, prototypes,
descriptors, rejected-transfer retry, errors, symbols, coercion and shared buffers.
The strengthened realm test additionally forces two collections, then verifies
strings, object/callable identity and signed zero. It passes separately with
copying and DRC. This covers actual collection, not just allocation.

## Provenance and next bottleneck

Runtime implementation checkpoint 6e088a7; benchmark native code is that change
atop dfae0d2, before the test-only forced-collection addition. Wasmtime 47.0.3.
Both collector artifacts use core-aot-v2-O3.wasm, built from runtime 90d0fbe,
compiler 72f281a632ac2b86e6fa2aaf5b76df2a7fd7b5f4, Deno
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44. Binaryen 125 O3 with the previously
recorded no-inline settings, Cranelift speed. Native runtime remains compiler-free.
V8 149.4.0; QuickJS-ng 034f2aba47ab6e7aacd869af7aa796b110d0ea2e.

Raw files/hashes and 21 rows: 2026-09-09-copying-processes.json. All raw stdout
and OS counters: copying-counters.json with the same date prefix. Paired data:
copying-ab.json; legacy `scalar` means DRC and `bulk` means copying. Both provider
arguments are `none`. Collector choices are recorded explicitly in each harness.
Local artifacts/data: /private/tmp/deno-profile.RCcI44/core-aot-copying-speed.cwasm,
comparison-copying and copying-ab. No benchmark rebuild is hidden in elapsed time.

Separate source review found object Map keys still hash to one bucket. A
correctness-checked standalone WasmGC probe in Node (not the Deno benchmark)
used 100/200/400/800 keys and 20 lookup rounds. Object medians were
0.93/3.60/13.31/56.33 ms; number medians 0.17/0.18/0.22/0.25 ms. All 48/48
checksums passed. Compiler checkpoint 824ea7b; no Wasm imports. Raw data in
2026-09-09-map-scaling.json, probe in compiler .tmp/deno-map-scaling.mts.
This supports investigating object collisions, not attributing the entire
remaining Deno gap to them. The short native sample captured zero stacks and
is not used as hotspot evidence. Moving GC rules out persistent raw-address keys.
