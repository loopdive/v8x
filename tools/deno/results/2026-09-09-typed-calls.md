# Typed bridge-call cache: no measured improvement

The proposed native typed-call cache did not improve the unchanged Deno
example. It is removed from the final code; the opt-in diagnostic accounting
is retained. Do not use the candidate as a new performance baseline.

Five alternating pairs, all 10/10 exact-output passes: preserved runtime
2f90064 versus candidate ba1cd2c, identical core-transient-copying-speed.cwasm,
copying GC, no provider. Median launch/example/exit: **80.7 ms versus 81.1 ms**.
The difference is small and does not establish a regression or a speedup.

Seven rotated processes per engine pass 21/21. Medians: V8 8.0 ms,
QuickJS 22.2 ms (2.8× slower than V8), candidate js2wasm 80.3 ms
(10.1× slower than V8). Candidate deployment 56.5 MiB, peak RSS 52.2 MiB,
macOS physical footprint 5.9 MiB. This remains about 3.6× slower than QuickJS.
First-launch outliers remain in the data. No profiling, task-owned tests,
builds or hooks ran during timing. Filesystem caches were not flushed.
This measures full fresh-process execution, not warm requests or tenant density.

## Diagnostic finding

Before caching calls, one exact-output diagnostic replay counted 1,814 realm
export calls. Inclusive timings include work inside Wasm, and calls can nest.
They must not be summed into an exclusive CPU breakdown or treated as medians.

| Export | Calls | Inclusive time |
| --- | ---: | ---: |
| define_data | 317 | 8.0 ms |
| get | 188 | 7.3 ms |
| utf16_unit | 434 | 6.1 ms |
| set | 129 | 3.5 ms |
| string_from_buffer | 128 | 2.8 ms |
| host_function | 104 | 2.2 ms |
| object | 106 | 2.1 ms |
| kind | 214 | 1.7 ms |

Enable V8X_JS2WASM_PROFILE_REALM=1 to emit counts and inclusive time on realm
destruction. With profiling absent, no timing map is allocated. Early missing
export failures are not counted; successful lookup followed by a trapping Wasm
call is counted. This diagnostic is not a complete native or compiler profiler.

The typed cache removed repeated lookup, allocation and validation, yet did
not move whole-process time. That rules it out as a demonstrated improvement
for this workload. It does not prove the entire bridge cost is Wasm execution.
Next concrete target: replace per-character UTF-16 reads with a validated bulk
transfer. The current reader makes one length call and one call per code unit.
Also retain investigation of object Map collisions; no stable identity hash
has been implemented.

## Correctness and reproduction

Candidate nine native tests pass, covering identity, prototypes, descriptors,
exceptions, buffers, reentrant host calls and forced copying GC. Added temporary
signature mismatch/failed-cache-insertion tests also pass. Cache removal keeps
the established dynamic-call implementation and exception rendering.

Compiler pin 72f281a632ac2b86e6fa2aaf5b76df2a7fd7b5f4; unchanged Deno
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44; core generated at runtime b8addd8,
Binaryen 125 O3 and Wasmtime 47.0.3 copying/speed. Apple M4/macOS 26.6.2,
ARM64/Rust 1.95.0. No interpreter deployed.

Raw typed-calls-processes.json records hashes and all 21 rows; counters.json
retains output and OS counters; ab.json retains all 10 paired rows.
Legacy scalar/bulk labels mean control/candidate. The diagnostic-only source
was runtime 248c159 plus the RealmCallProfile addition included in ba1cd2c.
Raw diagnostic output is 2026-09-09-realm-profile.txt.
Local control/candidate bundles remain in comparison-string-cache and
comparison-typed-calls under /private/tmp/deno-profile.RCcI44.
