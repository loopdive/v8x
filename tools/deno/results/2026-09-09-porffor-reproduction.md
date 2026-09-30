# Porffor PR reproduction: no comparable measurement yet

Inspected littledivy/v8x PR76, "feat: porffor backend", at exact head
1a7dce790d75414fa72a784c5b84a251ffeb94c6. The PR describes a scaffold with
75 implemented entries, 745 stubs, and one live isolate per process.

The linked smoke target covers primitive values, strings, objects, and collection.
It is not the unchanged Deno-core hello_world workload. Function creation,
script compilation/execution, and module APIs remain null-returning stubs.
The stub-only feature configuration excludes the smoke tests entirely, so a
zero-test pass or a small stub executable is not an engine measurement.

The required Porffor dependency could not be reproduced from public source:

- build.rs requires PORFFOR_DIR/runtime/index.js and invokes c --lib.
- The linked Rust backend expects porf_embed_* symbols.
- littledivy/porffor has only a public main branch, at
  61adb24cb90a0248f45cd9445d4c7118ba950cf9 when inspected.
- That checkout has runner/index.js, not runtime/index.js, and no embedding
  directory or porf_embed implementation. The exact generator command fails
  with a missing-module error.
- PR discussion contains no dependency link; GitHub code search for
  porf_embed_init returned zero results. Search absence alone is not proof that
  no copy exists; the exact required public checkout is what is missing here.

An attempted release linked smoke build also stopped earlier in automatic
vendor setup: applying existing rusty_v8 patches reported failures and its ICU
submodule fetch was blocked by network access. No Porffor smoke test executed.
Resolving vendor setup alone would not supply the missing embedding compiler.

Therefore footprint, speed and ratios against V8/QuickJS/js2wasm are **not
measured**. The PR's approximately 500 KB object estimate is author-reported,
not a reproduced complete deployment size, and must not be compared with the
391 MiB Deno-core deployment. Different functionality is included.

Needed next: the Porffor fork/commit containing the --lib target and matching
porf_embed_* ABI. With that, first reproduce all five smoke tests and compare
their restricted workload separately. A full Deno comparison additionally
requires implementing the currently stubbed script/module/host-function paths.

Local inputs: /private/tmp/v8x-porffor-benchmark-20260909 and
/private/tmp/porffor-embedding-20260909. Logs:
/private/tmp/deno-profile.RCcI44/porffor-smoke-build.log and porffor-generator.log.
No upstream source was modified and no upstream comments were posted.
