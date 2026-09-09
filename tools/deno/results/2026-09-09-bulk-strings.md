# Deno bulk UTF-16 reads

One-call string reads reduce the five-pair median from **82.0 ms to 75.5 ms**
(7.9% less elapsed time). Both sides use the same native runtime 5400404;
the control core lacks the new export and therefore uses the scalar fallback.
All 10/10 paired runs produce exact output.

Seven rotated fresh processes per engine pass 21/21:

| Metric | V8 | QuickJS | js2wasm |
| --- | ---: | ---: | ---: |
| Deployment payload | 37.3 MiB | 5.4 MiB (7.0× smaller) | 56.6 MiB (1.5× larger) |
| Peak process RSS | 20.2 MiB | 17.8 MiB (1.1× smaller) | 52.2 MiB (2.6× larger) |
| macOS physical footprint | 6.4 MiB | 10.7 MiB (1.7× larger) | 5.9 MiB (1.1× smaller) |
| Launch, example, exit | 8.1 ms | 22.4 ms (2.8× slower) | 75.8 ms (9.3× slower) |

Still **3.4× slower than QuickJS**, not goal completion. Ratios use unrounded
values. First-launch outliers are retained: 593.4/302.8/776.6 ms. Filesystem
caches are not flushed. Timing excludes compilation, optimization and profiling.
These are fresh-process startup-inclusive timings, not warm request throughput,
full Deno compatibility, or marginal tenant memory.

## Implementation and correctness

The private __v8x_value_string_storage export validates the string handle and
returns its compiler-owned storage. Rust reads through Wasmtime RootScope,
AnyRef, StructRef and ArrayRef APIs, never raw addresses. Flat strings validate
length, slice offset and packed i16 backing storage. Ropes validate child lengths
and are traversed iteratively, including memoized and hashed-string variants.
The result is a native UTF-16 copy, preserving lone surrogates and embedded NULs.
Unknown old artifacts fall back to the established per-code-unit reader.

The expanded fixture passes. Nine native tests pass with copying GC. Additional
DRC/new-artifact and copying/old-artifact tests each pass 1/1. New cases include
slices, doubled strings, 129-part concatenation, empty strings, invalid types,
and repeated forced GC. Two optimized full-core tests pass, including unknown
source refusal without host-op calls. Native precompile passes 1/1.
Initial dependent checks were accidentally started before wasm-opt completed
and failed because the output file did not yet exist; both were rerun after
the optimizer finished successfully. Those startup failures are not code failures.

## Evidence and remaining work

A separate exact-output profile no longer contains utf16_length or utf16_unit
calls. The new bulk calls bypass that scalar-only accounting, so the smaller
count is not a complete count of host/Wasm crossings. Before this change,
434 scalar unit reads cost 6.1 ms in one diagnostic run. Afterward, the usage
script phase was 6.0 ms in one diagnostic run. These are inclusive, non-median
observations and should not be added into an exclusive CPU breakdown.

Property definition and lookup remain prominent in the diagnostic output.
Compiler object Map keys still share a hash bucket. Generated scalar helper
inspection also found large lazy-global-initialization prologues; their actual
runtime contribution is unmeasured. Investigate these rather than claiming
the remaining gap has one proven cause.

Deployment is 59,399,176 bytes: precompiled core 50,102,616 bytes (47.8 MiB),
shared runtime 6,388,528 bytes (6.1 MiB), shared Rust std 1,216,576 bytes
(1.2 MiB), executable 1,691,456 bytes (1.6 MiB). No interpreter or compiler
is deployed. Payload grows slightly with the new export and native reader.

Runtime 54004043458998eddf40be64c67a8bbddaade5c5; compiler
72f281a632ac2b86e6fa2aaf5b76df2a7fd7b5f4; unchanged Deno source
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44. Binaryen 125 O3 with the
recorded no-inline settings, Wasmtime 47.0.3 copying GC/Cranelift speed.
Apple M4, macOS 26.6.2, ARM64, Rust 1.95.0.
Core SHA256 cdefe232abd6f48b98890f3f5afc72ed41a3ddfd21cd933cc29be9dfda3f42fb.

Raw same-date bulk-strings-processes.json contains all 21 rows and hashes,
counters.json contains stdout/OS output, ab.json contains all 10 paired rows,
and profile.txt contains the diagnostic output. Legacy scalar/bulk A/B labels
mean old/new artifact. Both providers are none and both collectors copying.
Artifacts and preserved bundles remain under /private/tmp/deno-profile.RCcI44.
