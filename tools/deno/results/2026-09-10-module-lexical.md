# Deno module binding lookup specialization

The same native executable with the new AOT artifact improves the five-pair
median from **66.8 ms to 56.4 ms**, 15.5% less elapsed time. All 10/10 paired
runs produce the exact unchanged Deno example output.

Seven rotated fresh processes per engine pass 21/21:

| Metric | V8 | QuickJS | js2wasm |
| --- | ---: | ---: | ---: |
| Deployment payload | 37.3 MiB | 5.4 MiB (7.0× smaller) | 42.9 MiB (1.2× larger) |
| Peak process RSS | 20.2 MiB | 17.9 MiB (1.1× smaller) | 41.0 MiB (2.0× larger) |
| macOS physical footprint | 6.4 MiB | 10.8 MiB (1.7× larger) | 5.9 MiB (1.1× smaller) |
| Launch, example, exit | 8.1 ms | 23.1 ms (2.8× slower) | 56.2 ms (6.9× slower) |

Still **2.4× slower than QuickJS**, not goal completion. First launches of
548.7/300.6/782.7 ms are retained. Ratios use unrounded values. No task build,
test, profile or commit hook overlaps timing. Filesystem caches are not flushed.
This measures startup-inclusive process time, not warm request throughput,
the full Deno CLI, or marginal tenant memory.

## Cause and fix

The compiler checked the global-eval lexical binding table before reading even
resolved module-private variables. This is unnecessary and incorrect: a global
environment is outside the module environment, so it cannot shadow its bindings.
The generated fallback code also boxed otherwise typed reads and included large
lazy-global setup paths. In the reduced bridge probe, ValueAt alone grew from
roughly 129 to 11,629 WAT lines when the eval provider was present. These are
unoptimized textual sizes, not executed-instruction counts.

Compiler e805e38c6be7f4fd3d79475627db5c3fb14e5d7d excludes checker-resolved,
non-ambient external-module declarations from that global sidecar lookup.
Direct-eval activation lookup still runs earlier. Ambient and unresolved names
keep conservative handling. Actual writes and live imported bindings are retained.
No Deno source or native runtime implementation changed.

The standalone Node bridge probe also improved from roughly 2.3 ms to 0.4 ms
per 1,000 checked number roundtrips with the refusal provider. Node tiering and
the small probe limit this evidence; the native paired measurement above is the
Deno performance result. The probe's named-call regex zeros are not a call census.

## Correctness and limits

All five new tests pass: const/let/var module bindings, an aliased mutable import,
and a positive control that preserves ambient lookup through the global sidecar.
Baseline passes 1/5: the four module cases read the injected global value 900
instead of their module binding. Structural assertions additionally check that
selected module readers lack the global-lookup temporary while the ambient
reader retains it. These are internal ABI collision tests, not a claim that a
real interpreter was invoked.

Seven existing module function visibility/reassignment tests pass. The broader
31-case direct-eval routing/state-pool comparison is 29/31 on both versions,
with identical failures in ambient Object fallback after delete and invoking an
AOT callable from a later provider entry. Thus the combined 36-case comparison
is baseline 30/36 versus candidate 34/36, not a fully green suite. Baseline is
clean compiler b8b0bcaf; candidate is e805e38c. An initial larger combined run
exhausted the 512 MiB worker heap; the comparable routing runs use a single
2048 MiB fork with explicit Wasm exception support. The aborted global-Script
Annex B run is not counted as passing coverage.

The compiler-built bridge fixture passes, and nine native identity, forced-GC,
Unicode, buffer, callback, descriptor and exception tests pass. Both optimized
core tests pass, including unknown-script refusal with zero native-op calls.
Precompile passes 1/1; exact native replay passes before timing. Configured TS7
typecheck passes. No compiler or interpreter is deployed.

## Footprint and provenance

Payload is 44,964,280 bytes (42.9 MiB): precompiled core 35,667,720 bytes
(34.0 MiB), native shared runtime 6,388,528 (6.1 MiB), Rust std 1,216,576
(1.2 MiB), executable 1,691,456 (1.6 MiB). Previous total was 47.1 MiB;
the generated core accounts for the reduction. Raw Wasm falls from 6,946,860
to 6,029,588 bytes; optimized Wasm from 4,812,878 to 4,053,923 bytes.

Local native macOS Wasmtime lane. Harnesses compare-bulk-transfer.mjs (five
alternating pairs) and measure-core-processes.mjs (seven rotated rounds).
Both paired sides use the preserved comparison-module-eval native bundle,
implementation 5400404. Control builder f6960db/compiler b8b0bcaf; candidate
builder 8a3afe0/compiler e805e38c. Deno source unchanged at
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44.

Binaryen 125 O3 with --no-inline, --pass-arg=no-inline@__new_*, --all-features,
--disable-custom-descriptors, -g. Wasmtime 47.0.3 copying GC, Cranelift speed.
Apple M4/macOS 26.6.2/ARM64/Rust 1.95.0. Candidate core SHA256:
56b10ae3f559cfc12f66ae2fcff0ec8bf8a0006d915226c41485c54e4bd0d0bb.
Control core SHA256:
7948b4e70c5cb054b05b371a8dbd9820b3f78152ecfff1d795da1fc4fd65142c.

Same-date module-lexical-processes.json retains 21 runs and hashes; counters.json
retains outputs/OS counters; ab.json retains 10 paired rows. Legacy scalar/bulk
labels mean old/new compiler artifacts, with identical executables, copying GC,
and no providers. Artifacts remain under /private/tmp/deno-profile.RCcI44.

## Remaining gap

A separate exact-output diagnostic, saved in module-lexical-profile.txt, still
spends 23.4 ms in the first core script (primordials) and 4.7 ms in the usage
script. These are single-run, nested inclusive timings, not medians or an
exclusive CPU partition. Realm get costs 2.8 ms across 188 calls and define_data
2.7 ms across 317 calls. Bulk string reads bypass the scalar-call counters.
Core load is noisy in this diagnostic and must not be read as a new regression.

Next trace primordial builtin/property setup and remaining dynamic dispatch.
This fix removes incorrect global lookups for module bindings; it does not
remove legitimate ambient lookups or solve general object Map hash collisions.
