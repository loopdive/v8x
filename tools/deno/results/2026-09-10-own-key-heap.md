# Open-object enumeration heap sort

The full unchanged-Deno gain is small: five alternating pairs measure
**55.8 ms before and 54.8 ms after**, 1.8% less elapsed time. All 10/10
paired runs produce the expected output. This does not explain most of
the remaining bootstrap gap.

Seven rotated fresh processes per engine pass 21/21:

| Metric | V8 | QuickJS | js2wasm |
| --- | ---: | ---: | ---: |
| Deployment payload | 37.3 MiB | 5.4 MiB (7.0× smaller) | 42.9 MiB (1.2× larger) |
| Peak process RSS | 20.2 MiB | 17.9 MiB (1.1× smaller) | 41.0 MiB (2.0× larger) |
| macOS physical footprint | 6.4 MiB | 10.8 MiB (1.7× larger) | 5.9 MiB (1.1× smaller) |
| Launch, example, exit | 8.0 ms | 22.5 ms (2.8× slower) | 54.4 ms (6.8× slower) |

Still **2.4× slower than QuickJS**, not goal completion. First launches of
539.6/300.7/784.4 ms remain in the data. No task compilation, tests, profile
or commit hooks overlapped timing. Caches were not flushed. This is process
startup plus the pinned deno_core hello_world example, not full Deno, warm
request throughput, or marginal tenant memory. Ratios use unrounded values.

## Mechanism and isolated measurement

The open-object enumeration helpers used selection sort on the compacted
property entries. Doubling the property count approximately quadrupled the
probe cost. The candidate uses in-place max-heapsort with the existing key
comparator: numeric keys first, then string insertion sequence. Compaction,
enumerable filtering, separate symbol handling and the trailing null slots
are unchanged. Fresh instruction trees are emitted for each helper/use.

The optimized standalone WasmGC probe, executed in Node 24.4.1, measures five
alternating rounds per variant, five checksum-validated scans per round:

| Properties | Selection sort | Heap sort |
| --- | ---: | ---: |
| 100 | 0.4 ms | 0.3 ms |
| 200 | 0.5 ms | 0.3 ms |
| 400 | 1.5 ms | 0.4 ms |
| 800 | 5.7 ms | 0.9 ms |
| 1,600 | 24.1 ms | 1.9 ms |

At 1,600 properties this is 12.8× faster, but it is not a Deno speedup.
All 250 timed scans and 20 warmup scans pass their checksum. Source, binaries'
hashes and all rows are retained in own-key-heap-probe.json; the replay runner
is tools/deno/measure-own-key-sorting.mjs. Both binaries were optimized with
the same Binaryen flags as the Deno artifacts. Earlier unoptimized probe
numbers (26.2/1.7 ms) are diagnostic only and are not the table above.

## Correctness

The new exact-order test checks 45 cases against native JavaScript across nine
sizes (0 through 700) and five APIs: keys, own property names, Reflect string
keys, values and entries. It covers numeric/string ordering, hash-table growth,
deletion/reinsertion, hidden properties and symbol exclusion in string-only
APIs. Both baseline and candidate pass the final 45-case test.

The first broader Reflect.ownKeys oracle exposed missing symbol keys on both
versions. That separate pre-existing merge path is not fixed or certified here;
the final Reflect case deliberately contains only strings. The original
diagnostic is retained under the clean control checkout's .tmp and its failure
is recorded in the issue handoff. An initial fixture used JS filename defaults
with TS annotations; corrected explicit heap-order.ts runs are the evidence.

The 42 existing enumeration tests pass 40/42 on both versions, with the same
two descriptor-materialization failures. This is not a clean full-suite claim.
Configured TS7 typecheck passes. The compiled bridge fixture and nine native
identity, buffer, descriptor, exception, symbol and forced-GC tests pass.
Both optimized-core tests pass, precompile passes 1/1, and exact native Deno
replay passes before benchmarking. No Deno source change or interpreter.

## Provenance and footprint

Control compiler e805e38c6be7f4fd3d79475627db5c3fb14e5d7d; candidate
8fd489a918dee3be51bb1e75d191f9815a830eb0. Runtime builder control 8a3afe0,
candidate a1a711f. Native implementation 5400404 and the same preserved
comparison-module-lexical executable/libraries on both paired sides. Deno
source unchanged at 1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44.

Binaryen 125 O3, --no-inline, --pass-arg=no-inline@__new_*, --all-features,
--disable-custom-descriptors, -g. Wasmtime 47.0.3 copying GC, Cranelift speed.
Apple M4/macOS 26.6.2/ARM64/Rust 1.95.0. No compiler or provider is deployed.

Payload remains 44,964,280 bytes: precompiled core 35,667,720 (34.0 MiB),
shared native runtime 6,388,528 (6.1 MiB), Rust std 1,216,576 (1.2 MiB),
executable 1,691,456 (1.6 MiB). Raw Wasm is 6,030,374 bytes, optimized Wasm
4,054,555 bytes. Candidate core SHA256:
24b22a1ee113e7737802630a91c1f075fcd96a4beaaa03652a363c195ac58356.
Control core SHA256:
56b10ae3f559cfc12f66ae2fcff0ec8bf8a0006d915226c41485c54e4bd0d0bb.

Same-date own-key-heap-processes.json retains all 21 runs/hashes; counters.json
retains outputs/OS counters; ab.json retains all 10 paired rows. Legacy
scalar/bulk labels mean old/new compiler artifacts, same native executable,
copying GC, both providers none. Artifacts remain under
/private/tmp/deno-profile.RCcI44/core-own-key-heap-* and comparison-own-key-heap.

The separate exact-output profile still records 22.6 ms in primordial setup
and 4.5 ms in the usage script. These are single-run inclusive/nested timings,
not medians or an exclusive CPU breakdown. Continue tracing builtin/property
and bound-function setup; enumeration sorting was not the dominant cause.
