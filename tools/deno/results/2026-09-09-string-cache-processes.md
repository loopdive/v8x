# AOT Deno: transient packets and short-string reuse

The unchanged Deno-core example now takes **79.8 ms**, still **3.6× slower
than QuickJS**. This is progress, not the performance goal being achieved.

Seven rotated fresh processes per engine pass 21/21 exact-output checks.
Apple M4, macOS 26.6.2, ARM64, Rust 1.95.0. No task-owned build, test or hook
overlaps either measurement. Filesystem caches are not flushed.

| Metric | V8 | QuickJS | js2wasm |
| --- | ---: | ---: | ---: |
| Deployment payload | 37.3 MiB | 5.4 MiB (7.0× smaller) | 56.5 MiB (1.5× larger) |
| Peak process RSS | 20.2 MiB | 17.8 MiB (1.1× smaller) | 52.1 MiB (2.6× larger) |
| macOS physical footprint | 6.4 MiB | 10.7 MiB (1.7× larger) | 5.9 MiB (1.1× smaller) |
| Launch, example, exit | 8.1 ms | 22.2 ms (2.7× slower) | 79.8 ms (9.8× slower) |

All first-launch outliers are retained: V8 487.6 ms, QuickJS 284.1 ms,
js2wasm 813.2 ms. Ratios use unrounded values. These are startup-inclusive
process timings, not warm request throughput or marginal per-tenant memory.
Physical footprint excludes clean file-backed pages and is not RSS.

## What changed and isolated measurements

1. Runtime b8addd8 keeps private one-shot transfer packets out of the canonical
   object-identity Map. Persistent buffers keep normal identity handling.
   Native calls fall back to the old export when loading older artifacts.
   Five alternating pairs, same native executable, old versus new core:
   **138.1 ms to 120.1 ms**, 13.0% lower elapsed time, 10/10 correct.
   Independent three-engine medians: V8 8.3 ms, QuickJS 22.6 ms, js2wasm
   121.6 ms. All 21/21 checks pass.
2. Runtime 2f90064 reuses already-rooted short-string handles within each realm.
   At most 512 entries, at most 64 UTF-16 units each; longer strings bypass the
   cache. No raw GC addresses or cross-realm handles are retained.
   Five alternating pairs, identical precompiled core, preserved b8addd8
   executable versus 2f90064 executable: **120.5 ms to 79.4 ms**, 34.1% lower
   elapsed time, 10/10 correct. This isolates the native cache contribution.

The larger problem remains: compiler object Map keys hash to a single bucket,
so remaining canonical object lookups still scan. These changes avoid work in
two adapter paths, not a general fix for object hashing. Prior scaling evidence
is in 2026-09-09-map-scaling.json. The entire remaining gap has not been
attributed to that one cause.

## Payload and scope

59,245,592 bytes deployed: precompiled core 49,971,320 bytes (47.7 MiB),
shared runtime 6,366,240 bytes (6.1 MiB), shared Rust std 1,216,576 bytes
(1.2 MiB), stripped executable 1,691,456 bytes (1.6 MiB). Rounded components
need not sum exactly. Most remaining size is generated core machine code.
No interpreter/provider is deployed and no compiler runs during the example.
This is a pinned closed-world AOT example, not general dynamic eval support,
the full Deno CLI, or complete Deno compatibility.

## Correctness and provenance

Transient packet fixture checks 512 unique packets, retirement, invalid sizes,
and persistent buffer/view identity. Nine native realm tests pass with copying
GC. The final string-cache source passes the focused native test with two
forced collections, cache eviction, lone surrogates and canonical identity,
both on copying/new artifact and DRC/legacy artifact. Two full-core tests pass,
including unknown-source refusal without host-op calls.

Deno source remains unchanged at 1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44.
Compiler is 72f281a632ac2b86e6fa2aaf5b76df2a7fd7b5f4.
Core generated from clean runtime b8addd8, Binaryen 125 O3 with recorded
no-inline settings, Wasmtime 47.0.3 copying GC and Cranelift speed.
The cache candidate is runtime 2f90064 and reuses that exact core.
Core SHA256: 1b8e5fa99c38eacc549f07b00296bc026015d23b6ae43c77dec779504c21273e.

Raw evidence: same-date transient-* and string-cache-* JSON files. Each
processes file records all 21 runs and executable/library/artifact hashes;
counters retain stdout and OS output. Each ab file retains all 10 runs.
Legacy A/B labels scalar/bulk mean control/candidate, not scalar versus bulk
for these experiments. Both providers are none, both collectors copying.
Controls and artifacts remain under /private/tmp/deno-profile.RCcI44.
