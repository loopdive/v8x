# Bulk value transfer without Deno source changes

The v8x boundary now sends strings as a temporary UTF-16LE byte packet.
Three Wasm calls create the packet, expose its checked GC storage, and consume
it into a string. Copying bytes uses Wasmtime GC APIs, not one Wasm call per
character. Only the completed string enters the persistent value table;
intermediate prefixes no longer receive handles. Lone surrogates and embedded
NUL are preserved. Old artifacts lacking the new exports keep scalar fallback.

For a newly allocated, unpublished host graph node, four or more data-property
definitions are sent as an ordered packet of key handle, value handle, and flags.
All handles are checked against their owning realm and the packet's u32 range.
Nodes with fewer definitions use the direct path. Seeded objects keep immediate
definitions. Existing-object Set/Get and user callbacks are never deferred.

Deno's initialize_deno_core_ops_bindings interleaves writes with function setup
and, for async ops, calls into setUpAsyncStub. Grouping those separate native
calls would risk observable ordering changes. This implementation batches only
the internal initialization that v8x already owns within one transfer.

## Validation

- Compiled context bridge JavaScript assertions pass.
- Native bulk fixture passes empty/Unicode/lone-surrogate/1024-unit round trips,
  eight-property packets, and duplicate-key ordering.
- Five integration tests pass: identity, descriptors, numeric exceptions,
  failed graph transfer retry, and shared buffers.
- Native error/prototype tests pass, as do non-string function names and the
  linked callback lifetime test using freshly generated bulk context fixtures.
- Legacy scalar fixture passes with the new runtime.
- The unchanged Deno-core hello_world produces exact expected output and exits 0.

The initial optional callback test run lacked its fixture environment and was
not counted as validation. Supplying the separate function-name and linked-module
paths plus the required provider resolved those setup failures.

## Artifact provenance

Five uncontended alternating pairs all passed (10/10). Median startup plus
example plus exit was 13,104.3 ms scalar and 11,648.1 ms bulk: 11.1% less elapsed
time, or 1.1× faster when rounded to one decimal. This is not warm throughput.
The gain is modest and does not close the previously measured V8/QuickJS gap.
Raw stdout, timing and OS memory counters are checked in as
results/2026-09-09-bulk-ab.json. No build or test ran during these timing pairs.

Bulk source checkpoint: 77985de on codex/4376-deno-realm-bootstrap.
Clean detached build checkout: /private/tmp/v8x-bulk-artifact-20260909.
Compiler remains bda15bdf70baefc3d7620f32a03dc3660c2fd005; Deno remains
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44. No Deno source edits.
Core rebuilt, Binaryen125 -O3 optimized, then precompiled with Wasmtime47.0.3
Cranelift speed. The regenerated provider is byte-identical to the prior input
(SHA-256 7a1391606c0b454d5ea3baa9fb2c1d8bfcc6980d99bb2b844bc226cac1e48df1),
so its validated optimized/precompiled output is reused. The redundant provider
optimizer was stopped before measurements; no tests were terminated.

Five alternating fresh-process pairs compare the preserved scalar deployment
with the bulk deployment, using the same provider. This isolates the combined
bulk-string/property implementation, not the contribution of either separately.
The harness is compare-bulk-transfer.mjs and results are retained at
/private/tmp/deno-profile.RCcI44/bulk-ab/results.json.

Follow-up: the value table now has a native Map index, preserving distinct
signed-zero handles and deleting retired packets from both index and roots.
The paired compiler fixes hashing of logical string views. See
results/2026-09-09-indexed-processes.md for the new 12.8× faster A/B and the
remaining V8/QuickJS gap. The measurements above remain the historical bulk-only
checkpoint, not measurements of the index.

Remaining limitations: generic export lookups are not yet cached; packet
construction/decoding costs remain; this is not a general cross-call scheduler
or a full Deno implementation.

Profiling is opt-in with V8X_JS2WASM_PROFILE_PHASES; reported scopes are inclusive
and may nest. Precompilation accepts V8X_JS2WASM_CRANELIFT_OPT=none, speed, or
speed_and_size. The default remains none for compatibility and bounded build
memory; compiler-free replay does not include Cranelift.
