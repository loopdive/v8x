# Deno private-module call specialization

The same native executable with the new compiler artifact improves the
five-pair median from **77.7 ms to 65.9 ms**, 15.2% less elapsed time.
All 10/10 paired runs produce the exact unchanged Deno example output.

Seven rotated fresh processes per engine pass 21/21:

| Metric | V8 | QuickJS | js2wasm |
| --- | ---: | ---: | ---: |
| Deployment payload | 37.3 MiB | 5.4 MiB (7.0× smaller) | 47.1 MiB (1.3× larger) |
| Peak process RSS | 20.2 MiB | 17.8 MiB (1.1× smaller) | 44.4 MiB (2.2× larger) |
| macOS physical footprint | 6.4 MiB | 10.7 MiB (1.7× larger) | 5.9 MiB (1.1× smaller) |
| Launch, example, exit | 8.3 ms | 23.1 ms (2.8× slower) | 66.3 ms (8.0× slower) |

Still **2.9× slower than QuickJS**, not goal completion. First launches are
retained outliers: 613.0/315.5/817.1 ms. Ratios use unrounded values.
No build, test, profiling, or commit hook overlaps the timing runs.
Filesystem caches are not flushed. This is startup-inclusive process time,
not warm request throughput, the full Deno CLI, or marginal tenant memory.

## Cause and change

Linking runtime eval conservatively made private module function bindings live:
calls had to fetch the current callable, box arguments, build arrays, and enter
generic closure dispatch. Unknown indirect eval was treated as potentially
replacing private functions in other ES modules, although indirect eval cannot
resolve their lexical bindings.

Compiler b8b0bcaf2dc95810931fef63499a8d828c117ed8 uses the existing source-owned
eval inventory to exclude that impossible rebinding. Direct eval in the same
module, script scope, unknown source ownership, and actual assignment syntax
retain conservative handling. This is a scope proof, not disabling eval or
assuming that user functions never change. Deno and native runtime source
remain unchanged in the paired test.

A reduced bridge probe supported this diagnosis before the full rebuild.
Five rounds of 1,000 checked numeric round-trips, in standalone WasmGC in Node,
showed roughly 9.1 ms with an unused indirect-eval entry versus 2.2 ms after
the change. That is not a Deno timing. The probe's named-call regex returned
zero because the WAT printer uses numeric call indices; those zeros are not
a valid call census and are not used as evidence.

## Correctness

Seven new scope/alias tests pass. They cover indirect eval, Function construction,
an eval alias, direct eval, script/unknown ownership, identical source basenames,
and preserved actual reassignment through calls and aliases. Structural checks
verify the private function has no unnecessary mutable global while the
reassigned function still has one.

Comparison suite: candidate 19/22, baseline 12/15, with the same three existing
failures in default-import reassignment/module initialization and two Annex-B
cases. Thus all seven added tests pass and the 15 old tests retain their
12-pass/3-fail split. This is not a clean full-suite claim.
Vitest needs --experimental-wasm-exnref in its worker execArgv, not merely
on the parent process. Initial runs without that flag had additional harness
failures; corrected runs are the reported comparison. Configured TS7 typecheck
passes. An accidental generic tsc invocation used the wrong configuration and
is not counted as the configured check.

The compiler-built bridge fixture passes. Nine native tests pass with copying
GC, including forced collection, Unicode, identity, buffers, callbacks and
exceptions. Both optimized-core tests pass, including unknown-source refusal
with no native-op calls. Native precompile passes 1/1 and exact Deno replay
passes before timing. No interpreter or compiler is deployed.

## Footprint, provenance, and remaining work

Deployed files total 49,404,984 bytes: core 40,108,424 bytes (38.3 MiB),
native shared runtime 6,388,528 bytes (6.1 MiB), Rust std 1,216,576 bytes
(1.2 MiB), executable 1,691,456 bytes (1.6 MiB).
Total drops from 56.6 MiB to 47.1 MiB; generated core accounts for the reduction.
Raw Wasm shrinks from 8,697,058 to 6,946,860 bytes, optimized Wasm from
6,328,469 to 4,812,878 bytes.

Runtime builder pin f6960db; native implementation unchanged at 5400404.
Control compiler 72f281a632ac2b86e6fa2aaf5b76df2a7fd7b5f4, candidate b8b0bcaf.
Deno 1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44, unchanged source.
Binaryen 125 O3 with recorded no-inline flags, Wasmtime 47.0.3 copying GC and
Cranelift speed. Apple M4/macOS 26.6.2/ARM64/Rust 1.95.0.
Candidate core SHA256:
7948b4e70c5cb054b05b371a8dbd9820b3f78152ecfff1d795da1fc4fd65142c.

Same-date module-eval-processes.json retains 21 rows and artifact hashes;
counters.json retains output/OS counters; ab.json retains all 10 paired rows.
Legacy scalar/bulk labels mean old/new compiler artifact, same executable,
both providers none, both collectors copying. Bundles/artifacts remain under
/private/tmp/deno-profile.RCcI44/comparison-module-eval and core-module-eval-*.

The refusal provider still triggers shared-realm value handling, and object
Map collisions remain. Neither is yet an exclusive attribution of the remaining
gap. Continue profiling and proving narrower AOT paths rather than assuming
this scope fix solves all dynamic dispatch.
