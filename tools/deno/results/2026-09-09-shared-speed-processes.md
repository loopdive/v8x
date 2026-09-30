# Deno core: optimized AOT and shared runtime

Seven fresh processes per engine, rotated order, identical unchanged upstream
deno_core hello_world. All 21/21 exited successfully with exact expected stdout.
Apple M4, macOS 26.6.2, ARM64. No task-owned build ran during measurement.
Filesystem caches were not flushed. Medians:

| Metric | V8 | QuickJS | js2wasm |
| --- | ---: | ---: | ---: |
| Complete deployment | 37.3 MiB | 5.4 MiB (7.0× smaller) | 391.2 MiB (10.5× larger) |
| Peak process RSS | 20.2 MiB | 17.9 MiB (1.1× smaller) | 336.5 MiB (16.7× larger) |
| macOS physical footprint | 6.4 MiB | 10.8 MiB (1.7× larger) | 9.4 MiB (1.5× larger) |
| Process launch, example, exit | 11.3 ms | 24.0 ms (2.1× slower) | 12,812.5 ms (1129.9× slower) |

Against QuickJS, js2wasm has 73.1× larger deployment, 18.8× larger peak RSS,
1.1× smaller physical footprint, and 534.9× slower fresh-process execution.
Ratios use unrounded measurements.

Physical footprint is macOS memory accounting, not RSS or incremental instance
memory. Clean file-backed mappings are excluded from that footprint. Neither
column establishes the marginal cost of an additional tenant.

Timing ranges were V8 8.4–552.8 ms, QuickJS 23.2–300.1 ms, and js2wasm
12,678.9–13,287.9 ms. The first V8/QuickJS launches were conspicuous outliers.
Do not compare the new V8 median with an older noisy baseline to infer an engine
speed change. The js2wasm median user+system CPU time was 12.76 seconds.
The OS CPU timer rounds too coarsely to report meaningful V8 CPU ratios.
These are startup-inclusive elapsed times, not warmed kernel throughput.

## Deployment breakdown

| js2wasm component | MiB |
| --- | ---: |
| Deno example executable | 1.6 |
| Shared v8x/Wasmtime runtime | 6.0 |
| Shared Rust standard library | 1.2 |
| Compiled Deno core | 85.8 |
| Compiled runtime-eval provider | 296.6 |
| Total | 391.2 |

Exact total: 410,210,336 bytes. The shared library files are copied, stripped,
relocated to loader-relative references, and ad-hoc signed by the harness.
otool confirms the executable and library actually reference those copies.
The new native payload is 8.8 MiB versus the earlier 2.3 MiB static executable.
The dylib retains a broader public API and requires unwind panic handling;
this is not a controlled comparison of linkage alone.

The two compiled artifacts total 382.4 MiB. Their ELF sections contain:

| Content in the two artifacts | MiB |
| --- | ---: |
| Machine code | 311.3 |
| GC stack maps | 56.7 |
| Trap metadata | 13.1 |
| Other sections and alignment | 1.3 |

Cranelift speed optimization reduced these files from 543.8 to 382.4 MiB,
29.7% smaller. The original Wasm was already optimized with Binaryen 125 -O3.
The remaining bulk is real generated code and GC metadata, not debug symbols.
The provider includes the runtime evaluation machinery rather than only the
example's arithmetic. This workload still does not support full Deno.

## CPU and memory diagnosis

A separate diagnostic replay of the same deployed candidate also passed.
During its first three seconds, 1598/2278 main-thread samples were under
v8__Object__Set; a 647-sample branch was in realm_string. These are samples
of an early interval, not percentages of whole-process runtime.
The earlier unstripped profile locates this in Deno op-binding initialization.
The source builds strings using an empty-string Wasm call followed by one
append call per UTF-16 unit. Each call goes through the generic realm export
lookup/call path. Host object graphs are also transferred property by property.

Inclusive phase timers in the diagnostic replay measured core loading 202.7 ms,
provider loading 1123.6 ms, module initialization 14.8 ms, four core scripts
659.8 ms combined, and usage-script execution 1234.6 ms. Provider loading is
nested in store/instance setup (1170.5 ms); do not add nested timers.
Sampling perturbs timings and these observations are not benchmark medians.
Uninstrumented native property setup is not fully covered by these timers.
Disposal was 0.1 ms. Loading and disposal cannot explain the whole CPU gap.

vmmap at about four seconds showed 382.4 MiB of mapped artifact files,
315.5 MiB resident and zero dirty bytes. Physical footprint was about 9.1 MiB
at that instant. Its 4.1 GiB VM_ALLOCATE reservation had only 4.0 MiB resident:
virtual address reservation is not equivalent to physical allocation.
Clean artifact code is shareable, but multi-process physical sharing and
additional-instance costs have not been measured.

The highest-priority follow-up is batching strings and property transfer across
the boundary, then reducing the broadly generated provider/core code. Cached
typed export calls may reduce call overhead, but their benefit needs measurement.
Moving the small native runtime to a dylib does not address either main cost.

## Provenance and scope

Deno commit 1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44, deno_core 0.407.0;
V8 149.4.0; QuickJS-ng 034f2aba47ab6e7aacd869af7aa796b110d0ea2e.
Rust 1.95.0. Wasmtime 47.0.3, compiler-free runtime.
Compiler input bda15bdf70baefc3d7620f32a03dc3660c2fd005, unchanged optimized
Wasm from the previous full-core control, precompiled with Cranelift speed.
Runtime candidate is local profiling/shared-library changes atop
9904a59ab3c8b630bf5ea9b1998084b2256f0c09; V8/QuickJS controls are preserved
from the prior report. File SHA-256s are in the adjacent processes JSON.
Raw OS counters are in the adjacent counters JSON.

Harness: tools/deno/measure-core-processes.mjs. Inputs and outputs retained at
/private/tmp/deno-profile.RCcI44/comparison-shared-speed.
The separate sample, vmmap and inclusive timings are in shared-native-sample.txt,
shared-vmmap.txt and shared-sample.phases in its parent directory.
This is a Deno core integration/startup benchmark, not a full Deno CLI,
steady-state request-throughput or tenant-density benchmark.
