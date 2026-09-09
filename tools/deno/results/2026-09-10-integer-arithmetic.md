# Arithmetic bridge validation experiment

Rejected as a performance optimization. Five alternating pairs measured
67.2 ms for the existing artifact and 66.9 ms for the arithmetic candidate.
That 0.3 ms difference does not establish a meaningful speedup. All 10/10
paired runs returned the exact unchanged Deno example output. The precompiled
core has exactly the same 40,108,424-byte size in both variants.

Seven rotated fresh processes per engine passed 21/21:

| Metric | V8 | QuickJS | js2wasm candidate |
| --- | ---: | ---: | ---: |
| Deployment payload | 37.3 MiB | 5.4 MiB (7.0× smaller) | 47.1 MiB (1.3× larger) |
| Peak process RSS | 20.2 MiB | 17.8 MiB (1.1× smaller) | 44.5 MiB (2.2× larger) |
| macOS physical footprint | 6.4 MiB | 10.7 MiB (1.7× larger) | 5.9 MiB (1.1× smaller) |
| Launch, example, exit | 8.2 ms | 23.1 ms (2.8× slower) | 65.9 ms (8.0× slower) |

The candidate remains 2.9× slower than QuickJS. First launches of
556.5/302.7/793.4 ms are retained, not excluded. No task build, test, profile,
or commit hook overlapped timing. Filesystem caches were not flushed.
These are startup-inclusive process measurements, not warm request throughput
or marginal tenant memory. Scope is pinned deno_core hello_world, not full Deno.

## Change and correctness

Runtime candidate ee6e73d replaced five floor-based integer validation
conditions with remainder checks. Existing handle, buffer, view and descriptor
bounds were retained. The fixture and nine native bridge tests passed; both
optimized-core tests passed, including unknown-source refusal without native
ops. Precompile passed 1/1 and exact native replay passed before timing.

The prior implementation also passed the expanded fixture with replaced realm
Math. That control does not demonstrate a new correctness benefit and must not
be cited as one. The arithmetic production changes are restored to the previous
implementation. Additional NaN, infinity, fraction, signed-zero and descriptor
flag assertions are retained; the inconclusive Math replacement probe is removed.

## Reproduction and deployment

Local native macOS Wasmtime lane, compare-bulk-transfer.mjs with five alternating
pairs and measure-core-processes.mjs with seven rotated rounds. Both paired
sides use the same preserved comparison-module-eval executable and shared
libraries. Native implementation is unchanged at 5400404. Control runtime
artifact builder f6960db, candidate ee6e73d; compiler in both is
b8b0bcaf2dc95810931fef63499a8d828c117ed8. Deno source remains unchanged at
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44.

Binaryen 125 O3, --no-inline, --pass-arg=no-inline@__new_*, --all-features,
--disable-custom-descriptors, -g. Wasmtime 47.0.3, copying GC, Cranelift speed.
No compiler or interpreter is deployed. Apple M4, macOS 26.6.2, ARM64.

Payload is 49,404,984 bytes: precompiled core 40,108,424 bytes (38.3 MiB),
native shared runtime 6,388,528 (6.1 MiB), Rust std 1,216,576 (1.2 MiB),
executable 1,691,456 (1.6 MiB). Candidate core SHA256:
80c43ffe5b31eb586030adc421da2bd9352c324a62f7b6bac8443b094653236a.
Control core SHA256:
7948b4e70c5cb054b05b371a8dbd9820b3f78152ecfff1d795da1fc4fd65142c.

Same-date integer-arithmetic-processes.json retains all 21 runs and hashes;
counters.json retains output and OS counters; ab.json retains all 10 paired
rows. Legacy scalar/bulk labels mean control/candidate Wasm, not different
native implementations. Both provider selectors are none, both collectors
copying. Artifacts and bundles remain under /private/tmp/deno-profile.RCcI44.

The retained production baseline remains the module-scope optimization.
Remaining work is the shared-realm dispatch and primordial bootstrap cost,
not more source-level integer-check substitutions without generated-code proof.
