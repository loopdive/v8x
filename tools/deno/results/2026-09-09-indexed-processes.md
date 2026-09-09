# Deno core: indexed realm handles

The updated compiler/adapter pair reduces startup-inclusive example time by
92.2% in a five-pair alternating A/B: 12,092.6 ms to 944.3 ms, or 12.8× faster.
All 10/10 runs exit 0 with the exact unchanged Deno example output. This is a
large improvement, not parity with QuickJS or V8.

## Fresh three-engine comparison

Seven rotated fresh processes per engine, 21/21 correct. Same upstream
deno_core hello_world, Apple M4, macOS 26.6.2, ARM64, Rust 1.95.0.
No task-owned builds, tests or commit hooks ran during the reported timing runs.
Filesystem caches were not flushed. Medians and ratios from unrounded values:

| Metric | V8 | QuickJS | js2wasm |
| --- | ---: | ---: | ---: |
| Complete deployment | 37.3 MiB | 5.4 MiB (7.0× smaller) | 396.7 MiB (10.6× larger) |
| Peak process RSS | 20.2 MiB | 17.8 MiB (1.1× smaller) | 341.1 MiB (16.9× larger) |
| macOS physical footprint | 6.4 MiB | 10.7 MiB (1.7× larger) | 9.5 MiB (1.5× larger) |
| Process launch, example, exit | 9.0 ms | 24.4 ms (2.7× slower) | 905.4 ms (100.7× slower) |

Against QuickJS, js2wasm remains 37.1× slower and its complete deployment is
74.1× larger. This does not measure warm request throughput or additional-tenant
memory. Physical footprint excludes clean file-backed pages; RSS is not private
memory. First launches were outliers: V8 578.5 ms, QuickJS 309.5 ms, js2wasm
1,622.4 ms. All are retained, not silently discarded; this is not a cold-cache test.

## What changed

Every value crossing the adapter receives a canonical numeric handle. Previously
finding an existing handle scanned all retained values and repeatedly dispatched
JavaScript equality. The adapter now uses a native Map index. Negative zero has
a separate slot because Map normally merges signed zero; object identity and NaN
canonicalization are retained. Transient transfer packets are removed from the
index as well as the root table when consumed.

The index's empty-string identity test exposed a compiler bug: string Map hashing
used the full backing array, ignoring a string view's offset and logical length.
The fix hashes precisely that view. The sliced-key regression failed on the
control and passes with the fix. Nine compiler regression cases pass, including
transfer-buffer string construction. The earlier five-file Map/Set suite passed
40/40 before the final two cases were added. The compiled adapter growth fixture
and all ten native adapter tests pass. TypeScript7 and coercion/oracle checks pass.

Deno source is unchanged. The native executable and shared libraries are identical
on both sides of the A/B; both core and provider artifacts change. The compiler
also includes intervening inherited-field and object-parameter correctness fixes.
Therefore 12.8× is the measured combined compiler/adapter improvement, not an
isolated attribution of every saved millisecond to the Map index.

## Remaining cost

| js2wasm deployment component | MiB |
| --- | ---: |
| Deno example executable | 1.6 |
| Shared v8x/Wasmtime runtime | 6.0 |
| Shared Rust standard library | 1.2 |
| Precompiled Deno core | 90.2 |
| Precompiled runtime-eval provider | 297.7 |
| Total | 396.7 |

Exact deployment: 415,949,272 bytes. Core and provider together are 387.9 MiB,
97.8% of deployment. The index is not a size optimization; compiled deployment
has grown from the earlier 391.2 MiB checkpoint. Shared code remains file-backed,
but multi-process physical sharing has not been measured. A shared native library
alone cannot remove the much larger generated core/provider code.

A separate instrumented replay also produces exact output. Inclusive timers show
core loading 7.4 ms, provider loading 25.3 ms (inside store/instances 51.4 ms),
module initialization 3.4 ms, core scripts 56.4 ms combined, and usage-script
execution 454.8 ms. These are one diagnostic observation, not benchmark medians;
nested scopes must not be added. The usage path invokes the runtime-eval provider
for the script submitted by Deno. Remaining priorities are profiling that
parse/interpreter/callback path and reducing generated provider code, alongside
generic adapter export-call overhead. Neither full Deno support nor warm V8 parity
is established by this example.

The paired A/B RSS medians are 337.9 MiB control and 341.1 MiB candidate;
physical-footprint medians are both 9.5 MiB at one decimal. No memory saving is claimed.

## Reproduction and provenance

Lane: standalone WasmGC, compiled ahead of time, compiler-free Wasmtime replay.
Harnesses: tools/deno/compare-bulk-transfer.mjs and measure-core-processes.mjs.
Both assert exact stdout and successful exit. The paired harness's legacy labels
`scalar` and `bulk` mean **previous bulk implementation** and **new indexed pair**
in indexed-ab.json; neither side of this new A/B is the old scalar-string adapter.
Its optional final argument selects the candidate provider.

Control runtime 77985de, compiler bda15bdf70baefc3d7620f32a03dc3660c2fd005.
Candidate runtime ce590c968a1b36859437d74a14e4a96c5616bbb9, compiler
72f281a632ac2b86e6fa2aaf5b76df2a7fd7b5f4, both clean detached artifact builds.
Deno 1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44, deno_core 0.407.0;
V8 149.4.0; QuickJS-ng 034f2aba47ab6e7aacd869af7aa796b110d0ea2e.
Binaryen 125: --no-inline -O3 --pass-arg=no-inline@__new_* --all-features
--disable-custom-descriptors -g, then Wasmtime 47.0.3 Cranelift speed.
Both native artifacts target this macOS ARM64 machine; the raw provenance file's
Linux target expectation is CI configuration, not this native benchmark's target.

All paths, sizes, SHA-256s and individual timings are in indexed-processes.json;
all stdout and raw OS counters are in indexed-counters.json. The paired A/B
retains its exact stdout and raw counters in indexed-ab.json. Files have the
same 2026-09-09 prefix as this report. Loader-relative dylibs are copied, stripped,
and signed by the harness and counted in deployment.

Local artifacts: /private/tmp/deno-profile.RCcI44/core-indexed-speed.cwasm and
provider-indexed-speed.cwasm. Quiet paired run: indexed-ab-quiet/results.json.
Three-engine run: comparison-indexed. Raw-source manifest: indexed-provenance.json.
An initial exploratory A/B overlapped commit hooks and is excluded from the
reported A/B; its 10/10 correct runs are retained separately in indexed-ab/.

Porffor remains unmeasured because the public PR lacks its required embedding
compiler dependency and does not implement the Deno workload. See the separate
2026-09-09-porffor-reproduction.md, not a stub-only performance comparison.
