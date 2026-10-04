# Native Script environment checkpoint, 2026-10-04

## Module evaluation and rejection checkpoint, 2026-10-04

Adapter source 9a4e13a1cdfcd0b22f52caa24a2ba421b50970e1 now returns a cached
rejected evaluation Promise on execution/package failures. Repeated Evaluate
retains Promise identity. Synthetic callback exceptions preserve native payload
identity without leaking a synchronous exception. Uninstantiated API misuse
still throws synchronously; its Rust unit passes 1/1 (27 filtered /28).
Promise reactions now capture throws with a local TryCatch; otherwise Deno's
outer catcher intercepted the throw and the derived Promise falsely fulfilled.

Compiler-free ordinary controls pass 34, fail zero, ignore 13 /47.
The added AOT execution control passes 1/1 with 46 filtered: before evaluation the
global marker is undefined, after evaluation it is 42, namespace answer is 42,
and repeated evaluation has the same fulfilled Promise. Runtime compilation and
interpreter counts are zero. Three graph packager tests pass 3/3; retained build
controls pass 15/15. Unchanged Deno lazy loading passes 1/1 in 1.62s; WebIDL
17/17 in 24.98s, denominators 431 with 430/414 filtered, not full coverage.

New trusted graph packager invokes the existing compile-graph sidecar build-side,
Binaryen 125 no-inline O3, and Wasmtime 47.0.3 precompilation. It binds exact
entry, ordered source graph and native SHA before loading. Artifacts live at
/private/tmp/deno-module-evaluation.Ma8rec. Positive execution graph digest
e5ea1b29832d0bf7e3ebcc71a36e469d2cd56f3d3ada3811b0fabb843ebda00f,
native SHA 366ffaca93aa539830bebbf09389742f986d22f33fa520a78f1eb32c7d2f4d29,
optimized SHA 87cb6a674fa2d7fdc5cb5be3e203bc334abcbfb509f5ce9778843f417aff20fb.
Compiler implementation remains a675081032, workspace docs head 4e6b00f6c8;
existing full/small Context artifacts are unchanged.

Actual main_and_side_module now fails 0/1 instead of falsely passing with no
trusted graph. With two source-exact optimized packages the main body succeeds
but the side body throws: compile-graph.ts lowers import.meta.main to true for
every graph entry, independent of Deno's loader role. The native
SetHostInitializeImportMetaObjectCallback is a no-op. Its actual thrown error is
then obscured by unsupported native Promise transport into the compiled realm.
These failures are evidence of incomplete semantics, not lost valid coverage.
Deno and vendor tests remain unchanged.

Resume with host-owned import-meta initialization/loader roles, native Promise
transport, and native AOT module exception rooting. run_deferred_module_init still
consumes and renders Wasm payloads as diagnostics; synthetic exception identity
does not prove compiled thrown-object identity. Add positive and negative
controls that observe execution and original rejection reasons. Full graphs,
snapshots, typed persistent references, host services and matched benchmarks
remain required. No new throughput/footprint claim.

## Explicit foreign getter receivers, 2026-10-04

Population evidence caution: unchanged builtin_core_module and main_and_side_module
each report 1/1 while emitting missing trusted graph-artifact errors. Their Deno
code discards the mod_evaluate future and only awaits run_event_loop, so those
results do not prove module bodies executed. Module::Evaluate currently records
failure and returns null when compile_and_instantiate lacks an artifact. Next
audit evaluation-result propagation and add positive execution/negative missing
artifact controls before crediting module conformance; do not count these green
rows as integration coverage. Keep original Deno tests unchanged.

Compiler implementation a675081032 and adapter implementation
1214214dcc0627d9d0cb61c4042a7413173c1558 add the optional
`__v8x_script_get_export_receiver` three-reference export. The old two-reference
getter remains unchanged. New packages forward target, key and receiver to the
existing native Reflect helper; old packages still explicitly refuse alternate
receivers rather than silently substituting the target.

Compiler getter suite passes 56/56, including direct explicit receiver identity,
thrown receiver identity and subsequent ordinary reads. TypeScript 7 and scoped
Biome lint pass. The compiler-free native foreign-read control passes 1/1
(42 filtered /43) with both new receiver and thrown-receiver cases, plus the old
package refusal control; it asserts zero runtime compilations and interpreter
instances. Ordinary adapter run remains 31 passed, zero failed, 12 ignored /43.
Unchanged actual Deno lazy loading still passes 1/1, 430 filtered /431, in 1.72s.

Build-side `build-foreign-get-test-packages.mjs --calls-only` rebuilt optimized
Binaryen/Wasmtime packages in /private/tmp/deno-reentrant-script.MZdH2Q.
The new factories have marker 91/92 to distinguish them from the preserved old
receiver-less factory. Existing Context artifacts are unchanged. Compiler tests
require the fork itself to receive --experimental-wasm-exnref; the repository
Vitest config overrides parent execArgv, so a temporary derived config supplied it.
Unflagged runs failed validation, not JavaScript semantics. The default TS5
typecheck is not the configured gate; `pnpm run typecheck` (TS7) passes.

A candidate `const receiver={marker:91}` top-level Script still fails the existing
persistent lexical guard requiring dynamic externref storage. This is a real
remaining typed-reference planning gap, not resolved by getter routing. Native
receiver tests use an existing Context-owned marker, not an inferred private slot.
Full graphs/population, snapshots, Context-internal foreign reflection, non-Script
owner routing, host services and matched benchmarks remain incomplete. This
checkpoint changes neither Deno source nor tests and makes no speedup claim.

## CompileFunction and real lazy loading now pass

Implementation: 5cae5514f0598aeffe2ce36869669a01d0c7ff87, with Script-only routing
guard 0bd28340669fc2936c6de70e81e5aad343b8b8ab. The guard preserves existing
non-Script Module linking; it does not implement Module owner routing. Compiler runtime pin
9bfee5a9c6893bc17313c226363648ebe1ccb6b3 is unchanged. This supersedes the old
missing-CompileFunction checkpoint. Existing drafts remain incomplete.

The generic factory binds exact body, individual parameter names and resource.
Build-side Function parsing rejects invalid bodies and parameter source fragments
without executing the body. Runtime lookup requires trusted native packages.
Caller access supports reentrant instantiation without borrowing DenoRuntime again;
graph retention and exception roots survive initialization failures. Nested calls
restore outer completion. Bound native functions return no fake V8 code cache.

The first lazy replay stopped at expected foo despite correct original exports
through the native API. src/js2wasm_foreign_get.rs now binds owner-aware get/call
and ownership imports for cross-Script values, excluding the caller's own values.
Return references use Caller roots, not an inner scope dropped before the host
trampoline consumes results. Native getter exceptions preserve payload identity;
undefined results never cause a search through unrelated owners.

Unchanged Deno at 1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44, compiler-free patch:
actual lazy-script 1/1 in 1.72s, missing-script 1/1 in 1.71s, WebIDL 17/17 in
24.89s and derived conversions 2/2 in 3.06s. Denominator remains 431, not a full
population pass. Compiler-free adapter: 31 pass, zero fail, 12 ignored /43 plus
four explicitly executed native tests (3/3 with 40 filtered, then 1/1 with 42 filtered). These cover
callback loading, deferred Function bodies/parameters, original lazy exports,
foreign getters/calls, exception/receiver identity and pre-argument getter errors.
Each asserts zero runtime compilation and zero interpreter instances. Alternate
foreign Reflect receivers are explicitly refused and verified by a negative control.
Runtime-profile ordinary checks: 35 pass, zero fail, 31 ignored, four filtered /70.
Provider-retention controls were corrected after clean baseline f236d22698 showed
the same two failures: Context-only modules need zero interpreter providers, while
the retention fixture must actually import the provider whose lifetime it checks.
Build-side 18/18; Rust binding unit 1/1; compiler-free cargo check and formatting pass.
Site build cannot run because Typst is not installed. No new benchmark was run.

Artifacts /private/tmp/deno-reentrant-script.MZdH2Q: context-fixtures/context.cwasm
is the small fixture, 19,669,600 bytes, SHA
bfb2c7162c54121d704c0d4ed59b0c307044aa1492c44168bff259909e9e2eba.
Binaryen 125 optimized; Wasmtime 47.0.3 precompile 1/1 in 108.83s. Full Deno
Context remains /private/tmp/deno-call-order-native.dBQZ3T/deno-core.cwasm.
deno-scripts/ copies the original packages plus two exact Function factories;
deno-lazy-function-inputs.json records source hashes and bodies produced by Deno's
actual public wrap_lazy_ext_script. Build tools include a Rust wrapper linked to
the pinned unchanged deno_core library, not a copy of its wrapping implementation.

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-call-order-native.dBQZ3T/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-reentrant-script.MZdH2Q/deno-scripts /private/tmp/deno-upstream-conformance.H6HA4g/deno/target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::test_lazy_loaded_script --nocapture --test-threads=1
```

Compiler-free controls use target/debug/deps/js2wasm_spike-e7e456f13e693536;
packaging/runtime-profile controls use js2wasm_spike-13b131f10cc30c9d. Set
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR to context-fixtures and Script package directory
to the artifact root, except original-lazy-export control uses deno-scripts.
Invoke each artifact-backed control with --exact NAME --ignored --nocapture.

Resume with a three-argument owning getter for alternate Reflect receivers;
retain refusal until semantics are implemented. Cover Context-internal foreign
reflection and non-Script graphs. Then full unchanged population/module graphs,
snapshots, context extensions/cache semantics, macro-generated inputs, host
capabilities, BigInt/unpaired UTF-16, shared libraries and fresh comparative
benchmarks. No interpreter or Deno/test source rewrite was introduced. Preserve
pre-existing .tmp/ files. All earlier sections describe their own checkpoints.

## Stop checkpoint

Wrapped up at user request with existing draft PRs
https://github.com/loopdive/v8x/pull/2 and
https://github.com/loopdive/js2/pull/6468. Implementation/test heads before this
docs checkpoint are adapter 4978e6bd83b411a7019a042d2380beaad0a597b6 and compiler
b87dfe8cc95a8c9ceaab63b02f19bc109cdcf027. No CompileFunction code was started.
No new benchmark or full-population result is claimed.

Next factor same-store Script instantiation for both DenoRuntime and CallerRealm,
using RealmAccess and realm_callback_access::with_owner during callbacks. Keep
StoreData.aot_call_graphs retention, exception roots and outer completion state
correct across nested function-factory execution. Then implement the vendored
CompileFunction ABI using trusted packages binding exact body, parameter names
and origin. Materialize a native owning-graph callable; verify Script closure
classification. Reject unsupported context extensions and absent packages loudly.

Entry points: src/js2wasm/mod.rs, src/js2wasm_spike.rs,
src/js2wasm_realm_values.rs, src/js2wasm/realm_callback_access.rs,
src/js2wasm/realm_objects.rs, src/js2wasm_script_packages.rs and
tools/js2wasm/script-packages.mjs. Add native function/parameter/exception/reentry
controls, replay unchanged test_lazy_loaded_script, then WebIDL, conversions and
lazy-script-not-found regressions. Full graphs, snapshots, host capabilities,
transport and matched comparative benchmarks remain required. Do not modify Deno
or vendor tests, introduce an interpreter or silently emulate missing behavior.
Preserve pre-existing .tmp/ files. Exact measured pins and artifacts follow.

## Fresh native evidence at the synchronized compiler

Clean compiler 9bfee5a9c6893bc17313c226363648ebe1ccb6b3; builder adapter
f236d22698bfe60bd82e4bbea9415c2e70176dd4; unchanged Deno
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44. Artifacts are in
/private/tmp/deno-call-order-native.dBQZ3T. The existing native replay binary
deno_core-87206ac56a2fccad is reused (no Rust adapter change this checkpoint).

Unchanged lazy-script-not-found now passes 1/1 in 1.68s; WebIDL 17/17 in 23.95s;
derived conversions 2/2 in 2.96s. The real lazy-script test aborts at missing
v8__ScriptCompiler__CompileFunction (exit 134). Deno compiles a strict function
body returning the original IIFE with one __bootstrap parameter and no context
extensions. Implement generic trusted-AOT function-body packaging and parameter
bindings, then return a native owning-graph callable. Preserve cache, closure,
object and exception identity. Compilation/execution occurs within a Rust host
callback: use active CallerRealm rather than reborrow DenoRuntime's RefCell.
No runtime compiler, interpreter or Deno source rewrite should be introduced.

Raw Context: 2,724,683 bytes, SHA
5e1e00422b3979354bc8f64772423f8323a48870b3fd23143b5685914f2bb3a6.
Binaryen 125 optimized: 2,019,396 bytes, SHA
02cf9fc795a8e946ffee2c11783456508b2bdcb44606541400407a5056d8006c.
Wasmtime 47.0.3 precompile 1/1 in 211.59s; native 44,121,952 bytes, SHA
cbb13265e36297f5bedca29b7e31d4439568dc08bc3a239082faab8054037aad.
All 16 literal module-test Scripts and five WebIDL Scripts package; conversions
have 4 packaged and 2 unresolved macro inputs /6 sites (exit 1, not success).
Default-off compiler controls match pre-fix merged compiler bytes 3/3. Full
population, snapshots, graphs, host operations, transport and benchmarks remain open.

## Continuation: main sync and method ordering

Runtime compiler pin: 9bfee5a9c6893bc17313c226363648ebe1ccb6b3, after merging
origin/main 39fd7b7d44. New compiler controls exposed arguments running before
foreign getters. Captured frame-local callees now fix the measured dot/computed
closed-dispatch paths. Compiler controls pass 140/140 (54 focused and 86 persistent,
including two existing expected failures); constructor/expression merge controls
pass 30/30. TypeScript passes; lint has no errors. Spread and independent IR call
ordering still need coverage. Native rebuild/replay is in progress, not credited.

## Wrap-up: owner-aware method call

Runtime compiler pin: ffea2d022fde6c76b189b0958ca91c4b86fc3369.
The foreign method call control is no longer an expected failure. Context-owned
callees dispatch through __v8x_context_call; the method is resolved once and its
receiver is retained. Full compiler focused suite passes 41/41; TypeScript and
scoped lint pass. These are compiler controls, not native lazy-loader evidence.

Resume with caller-owned callback and getter/argument ordering controls, then a
fresh clean pinned Context/Script rebuild using Binaryen 125 and Wasmtime 47.0.3.
Replay unchanged lazy-loader, WebIDL and conversions before claiming native
progress. Latest native evidence is 17/17 WebIDL, 2/2 derived conversions and
0/1 lazy-loader. Full 431-test run aborted at unsupported SnapshotCreator without
a summary. Complete module graphs, macro-generated inputs, host operations,
snapshots, transport gaps and matched V8/QuickJS/Porffor benchmarks remain open.
Existing adapter PR https://github.com/loopdive/v8x/pull/2 and compiler PR
https://github.com/loopdive/js2/pull/6468 remain drafts, not merge-ready.
Historical sections below describe their stated revisions, not this new pin.

## Ambient global read diagnosis, 2026-10-04

Current runtime compiler pin: 825eb75e74db913184480cf9182793db8549e0e4.
Focused compiler suite: 36 positive controls and one expected failure out of
37, not 37 functional wins. Typecheck passes. No new native Context replay yet.

The compiler's injected ambient Deno namespace bypasses symbol-less linked
global lookup and falls through to null. The candidate compiler now resolves
unimplemented ambient host names through the Context after native intrinsic
paths decline. Three controls read Deno, Deno.core and loadExtScript correctly.
The foreign method call still fails "called value is not a function" and has
an explicit expected-failure control. This does NOT verify native lazy loading.
Script packaging now supplies the already-exported __v8x_context_call terminal;
the Context owner ABI guard requires it. Further owner-aware callable routing
is needed; a call terminal alone does not prove [[Call]] support.

## Broader Script input inventory, 2026-10-04

Runtime compiler pin advances to 0c4fc2beae2a95b068b05dc1bf524bf5f9d02a20.
Context/native replay at that fresh clean pin is not yet measured.

New build-core-script-packages.mjs inventories literal and unresolved execute_script
calls in pinned Rust sources. It records every call and packaging failure; strict
literalScripts callers still refuse unsupported expressions. Reports include
compiler revision/diff fingerprint and packager binary hash, with unique run files
and a latest alias. Inventories describe textual call sites, not expanded Rust
macro populations or all module graph inputs. Build-side controls pass 18/18.

Initial convert.rs plus modules/tests.rs inventory: 22 call sites, 14 packaged,
6 failed packaging and 2 unresolved macros. The compiler now initializes
function-only completion Scripts and retains a completion sink export through
Binaryen DCE. Focused compiler tests pass 33/33; persistent Script tests 86/86.
After those local compiler edits, 16/16 module-test literal Scripts package.
This is a candidate packaging result, not a fresh clean-pin Context replay.

Four conversion literal packages allow unchanged derive_from_struct and
derive_from_tuple_struct to pass 2/2 (429 filtered of 431). The unchanged
test_lazy_loaded_script_not_found executes but fails 0/1 with
"Cannot access property on null or undefined", not its required lazy-load error.
Do not equate packaged module-test Scripts with native module conformance.
Next trace that host path, build module graph packages, expand macro-generated
sources/resource names without runtime compilation, and implement snapshot support.

## Verified native reflection replay, 2026-10-04

Unchanged pinned Deno WebIDL now passes **17/17**, 0 ignored and 414 filtered
out of 431, in 21.79 seconds. Dictionary record conversion now returns foo: 1.
Compiler c98082082b163165ed6c5ba7f726c24d01ee1ba7; Context builder/adapter
5c565213ecaa5df5a9c4424b8b65ffbe66e5f1bf; Deno
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44. Runtime profile has no compiler.

Artifacts: /private/tmp/deno-reflection-native.Zfl1zI. Raw Context 2,716,198
bytes, SHA256 62c279c95f6c342efe226b2194ae4e6382476e6a2aaedfa2d8f75c8a4d561eac;
Binaryen 125 optimized 2,013,627 bytes, SHA256
bd65b037c9b8c99d64bbd2e3b96da13bf4c970dcf95c92bfdbdcd6bdf2d28fa3.
All six Symbol global exports were inspected in the real Context. Wasmtime
precompilation passed 1/1 in 209.36 seconds. All five original WebIDL Scripts
were Binaryen optimized and precompiled; source/specifier bindings were checked.

The first optimized Script packaging attempt correctly exposed a too-strict
import equality assertion. Binaryen removes unused imports and reorders survivors.
Packaging now checks an identity/count subset, while still requiring the native
Script ABI; added, duplicated or changed capabilities are rejected. A new Context
builder guard requires six actual Symbol globals instead of trusting options.
Build-side controls pass 15/15. Canonical-index Rust control passes 1/1.

Full unchanged 431-test replay terminated with exit 134 at
modules::tests::dynamic_imports_snapshot: unresolved
v8__SnapshotCreator__CONSTRUCT. No full-population pass/fail summary was produced.
Earlier conversion/module tests refused because only the five WebIDL Scripts
were packaged; cancel_try_future also failed PermissionDenied vs Interrupted in
the restricted environment. These are distinct observations, not 431 measured
adapter failures. Next package broader unchanged Script/module inputs and handle
snapshot creation (or establish its compile-time replacement) without skipping
tests or adding an interpreter. The run was not killed or timed out.

Native Script ABI controls passed 2/2, including rejection of invalid signatures
and interpreter imports. An earlier incorrect test-name filter executed zero;
it is not counted as verification.
The clean packaging checkout could not fetch headers under network restrictions;
the same committed adapter was built using its existing local pinned vendor.
The Context used the clean committed builder; Script packaging used the documented
import-subset fix before commit. Full host support and fresh benchmarks remain open.

## Native reflection continuation, 2026-10-04

Runtime compiler pin: `c98082082b163165ed6c5ba7f726c24d01ee1ba7`.

The adapter now routes native own-name, own-symbol and descriptor queries to the
allocation's owning Script. It applies writable/enumerable/configurable filters
and canonical array-index conversions without executing getters or copying a
Script object into a Context object. Missing exports for a matching owner fail
loudly. Context bridge exports provide equivalent native reflection operations.
Compiler-free cargo check passes. Fresh native reflection/filter execution tests
and unchanged Deno replay have NOT yet been run for this checkpoint.

The compiler now actually implements shared Symbol state: six mutable native
globals. Previous compiler options requested this state but did not implement it.
Focused compiler controls pass 31/31, including cross-Script identity and closed
computed Symbol fields. Verify all six exports in the next full Context build.

Script packaging now invokes Binaryen before Wasmtime and records raw/optimized
hashes and the optimizer version. End-to-end packaging has NOT been rerun; its
strict optimized-import equality check may need validation against actual output.
Rebuild clean pinned Context and all five original Scripts, rebuild unchanged
Deno tests and replay WebIDL next. Old native results below remain 16/17, not a
claim that record conversion has been fixed. Full conformance, UTF-16 key fidelity,
Context exception identity, host capabilities and comparative benchmarks remain
open. Neither existing draft PR is merge-ready. Preserve the historical POC pin.

## Final checkpoint and resume handoff, 2026-10-04

Published implementation: compiler `dbe49bf307d635bd5c838ac6b36051597c5aa253`;
adapter `33af9d76e8154954b50944f55408a30685c8cfe9`.
Existing drafts: https://github.com/loopdive/js2/pull/6468 and
https://github.com/loopdive/v8x/pull/2. The compiler PR is stacked on
`codex/4376-deno-callback-construction-20260930`, not main.
Neither draft represents complete Deno integration or is merge-ready.

Explicit Reflect receivers now reach linked Array own/index/custom-prototype
accessors and native iterator prototype accessors. The old two-argument vec
reader ABI remains unchanged; the three-argument reader is default-off.
Focused getter controls pass 21/21. Four execution suites pass 111/111,
including two existing expected failures. Source-preservation controls are NOT
green: 38 pass and 53 fail out of 91 on both baseline `73c8c2369` and candidate,
with identical per-test statuses. Do not rebaseline those failures silently.
Build-side adapter controls pass 15/15; compiler typecheck and commit gates pass.

A fresh clean pinned full Context uses the implementation commits above and
Deno `1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44`.
Raw Wasm: 2,710,850 bytes, SHA-256
`e0cd4196f7e5f49e0edc01503d738c08ada13da337bd897f670a3a6a28862e1f`.
Binaryen 125 optimized Context: 2,009,537 bytes, SHA-256
`b1ccafb7b2cc226f1b7134bf7cd7845e10cf3e25acc62bf1b43f104ef245f969`.
Wasmtime 47.0.3 precompilation passes 1/1 in 207.48 seconds; native SHA-256
`ee1de8727239277aa14d0aa45858c35610333a43166951364af3a11aef68e418`.
These are artifact/build measurements, not runtime RSS or throughput.
The five original WebIDL Script packages are precompiled but not Binaryen
optimized; do not describe all artifacts as wasm-opt optimized.

Fresh unchanged native WebIDL replay: **16 pass, 1 fail, 0 ignored, 414 filtered
out of 431**, 21.13 seconds. Dictionary array conversion now returns the correct
`b: [65535]`. The remaining failure is object-record conversion:
`f: {}` instead of `f: {"foo": 1}`. Trace native property enumeration/ownership
and record conversion next; this is a failure location, not a proven root cause.
Do not change Deno tests or substitute a source-specific workaround.

Local artifacts and provenance:
`/private/tmp/deno-array-native-build.lcT9ej`.
Compiler-free unchanged Deno test checkout:
`/private/tmp/deno-upstream-conformance.H6HA4g/deno`.
Replay from that checkout:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-array-native-build.lcT9ej/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-array-native-build.lcT9ej/webidl-scripts target/debug/deps/deno_core-87206ac56a2fccad webidl::tests:: --nocapture --test-threads=1
```

Next: repair record conversion; run the full unchanged 431-test population;
complete native host capabilities, BigInt/UTF-16 transport, public Program
completion and AOT routing; factor shared code; then measure matched footprint
and performance against V8, QuickJS and Porffor. Full-population timeouts have
not been approved; never kill a test without approval. Preserve unrelated
workspace dirt. Typst rendering and fresh comparative benchmarks are unverified.

PR: https://github.com/loopdive/v8x/pull/2 (draft).
Compiler dependency: https://github.com/loopdive/js2/pull/6468 (draft),
commit `3d4c1dfdaf61f101cb07c7139b5a3ed65052d520`.

The retained Context linker now accepts `__v8x_context_lexical`. Runtime
and namespace-test Context builders export the compiler's lexical provider.
The runtime compiler pin advances independently of the historical POC pin.
The generated provider is included in the production graph-input provenance.
Missing Context exports remain a loud linker error, not an interpreter fallback.

The new fixture compiles one Context and eight independent Scripts. It checks
cross-Script object conversion, exact BigInt postfix results, declaration
preflight, const-write rejection, and isolation between two Contexts. The
native test also covers inferred number/boolean constants and foreign global
callable aliases. It deserializes separately precompiled artifacts, uses the existing
retained-Context linker, and asserts zero runtime compilations and zero
runtime-eval provider instantiations. This fixture contains explicit `any`
annotations and is not unchanged Deno conformance or the public Script path.

## Reproduce

Build raw fixtures with Node supporting TypeScript imports, using an absolute
compiler checkout path and a fresh output directory:

```sh
node --experimental-wasm-exnref --import "$JS2_CHECKOUT/node_modules/tsx/dist/loader.mjs" tools/js2wasm/build-script-environment-test-artifacts.mjs "$JS2_CHECKOUT" "$FIXTURE_DIR"
cargo test --offline --no-default-features --features js2wasm_spike,js2wasm_gc_copying,js2wasm_diagnostic_abi --test js2wasm_spike --no-run
```

Use the test executable printed by Cargo. For each of `context`, `script-0`
through `script-10`, run the packaging helper with the corresponding paths:

```sh
V8X_JS2WASM_DENO_CORE_WASM="$FIXTURE_DIR/context.wasm" V8X_JS2WASM_DENO_CORE_AOT_OUTPUT="$FIXTURE_DIR/context.cwasm" "$PACKAGING_TEST_BINARY" --exact precompiles_exact_deno_core_artifact --nocapture
cargo test --offline --no-default-features --features engine_js2wasm,js2wasm_gc_copying,js2wasm_diagnostic_abi --test js2wasm_spike --no-run
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR="$FIXTURE_DIR" "$DEPLOYMENT_TEST_BINARY" --exact precompiled_scripts_share_context_lexicals_without_interpreter --ignored --nocapture
node --test tools/js2wasm/test-runtime-compile-options.mjs
```

The packaging test is not ignored. Passing `--ignored` to it selects zero
tests and is not evidence of successful precompilation. The deployment test
is explicitly ignored by default because its trusted local artifacts must
be generated first. No Deno or rusty_v8 vendored test was changed.

## Remaining integration work

Completion continuation: the Context exports a native reference sink, reset
and rooted-handle getter. `DenoRuntime::instantiate_script` resets completion,
executes an independent artifact in the retained Context and returns its native
root handle. The source stays unwrapped. The real Context value bridge roots
returned objects without JSON copying and normalizes foreign Script undefined
singletons to handle zero. Strong roots still live until Context teardown; this
does not implement per-handle release.

One Context plus eleven Scripts (twelve artifacts) pass Node/Wasm controls at
`/private/tmp/deno-script-completion.8HrA0L`. Both Cargo profiles build; the full
existing Context-value-bridge probe passes its identity, scalar, callback,
buffer and root-growth assertions. Runtime option checks pass 10/10. All twelve
native packaging invocations pass (each 1/1, 61 filtered). Compiler-free replay
passes 1/1 (36 filtered, 0.11 seconds), asserting zero runtime compilations and
zero runtime-eval provider instantiations. The ordinary compiler-free adapter
suite reports 30 passes, zero failures and seven explicit ignored tests out
of 37; those ignored tests are not conformance credit. The first Context
precompilation took 126.95 seconds, not a deployment startup measurement.
The compiler completion suite passes 36/36 ordinary tests; its persistent Script
and older result regressions pass 89/89 including two existing expected failures.

This is not public Script dispatch yet: the private instantiation method is
exercised by the fixture. Production public Run still needs exact-source-bound
independent AOT package lookup and exception/result handle adoption. The full
Context has not been rebuilt and unchanged Deno conformance is not credited.
Both PRs remain draft. The older nine-artifact results below are historical.

Latest scalar/callable continuation: one Context and eight Scripts (nine
artifacts) pass raw controls and all nine packaging runs. The final compiler-free
native test passes 1/1 (36 filtered, 0.07 seconds), now also covering inferred
numeric const rejection and a boolean-constant callback through a foreign alias.
Fixtures are at `/private/tmp/deno-script-scalars.6Tj5CN`. The compiler focused
suite reports 86/86 including two existing expected failures; its wider five-file
run has the same three recorded TDZ failures. Mutable/reference-typed planning
remains open; const does not prove immutable array elements or object fields.

Earlier native-wiring checkpoint: raw Node fixture checks pass (seven artifacts),
all seven native packaging invocations pass, and the compiler-free deployment
test passes 1/1 with 36 other tests filtered out in 0.07 seconds. The nine
runtime compile-option tests pass 9/9. Both Cargo feature profiles build.
Artifacts are under `/private/tmp/deno-script-native.X4IpnK`; their raw hashes
and exact sources are recorded in `test-inputs.json`. They are trusted local
fixtures, not production attested packages. `make -C site all` fails because
`/opt/homebrew/bin/typst` is absent; documentation rendering is unverified.

Wire independent Script artifacts into public Script compilation/run,
preserving completion values and declaration semantics. Typed lexical
bindings, foreign accessors and broader callable transport remain incomplete.
Rebuild the full Deno Context on the new compiler pin and run unchanged Deno
tests before claiming a compatibility gain. Existing full-core replay
measurements use the older compiler pin, not this checkpoint. Factoring
compiler helpers into an external shared library and fresh footprint/speed
benchmarks also remain open. Do not advance suite baselines from this fixture.

## Historical public Script package checkpoint (2026-10-04)

This continuation is intentionally unfinished and belongs in the existing
draft PR https://github.com/loopdive/v8x/pull/2, with compiler companion
https://github.com/loopdive/js2/pull/6468. Compiler HEAD remains
`3d4c1dfdaf61f101cb07c7139b5a3ed65052d520`; the compiler PR targets the
callback-construction branch, not main. No full integration claim is made.

`src/js2wasm_script_packages.rs` defines an independent Script digest domain,
length-framing the exact resource specifier and original source. The proposed
lookup is `V8X_JS2WASM_AOT_SCRIPT_DIR/<digest>.cwasm`. It reuses the trusted
artifact binding reader and its `.graph-sha256` sidecar format, treating the
Script digest as the binding key. This is not a finished packaging format:
there is no Script package writer or independent Script-goal/completion-ABI
validation yet. Native deserialization remains restricted to trusted build
artifacts, never arbitrary tenant-provided native bytes.

`precompiled_bound_file` factors the existing verified-byte loader for both
graph and Script binding keys. `retain_graph_instance` separates retention
from initialization. `instantiate_script` now returns `(normal, handle)` and
calls the native initializer directly so a JS exception can be rooted through
the Context keeper before another error renderer consumes it. A Wasmtime trap
without a pending JS exception remains an infrastructure error. The initial
compile failure was the RootScope API: use
`scope.as_context_mut().take_pending_exception()`, not a RootScope method.

Fresh verification after that fix: the compiler-free feature profile builds;
the native Script fixture passes 1/1 (36 filtered, 0.11 seconds); the ordinary
adapter binary reports 30 passes, zero failures and seven ignored out of 37;
runtime option tests pass 10/10. The fixture reuses the twelve trusted native
artifacts at `/private/tmp/deno-script-completion.8HrA0L`, not freshly rebuilt
full Deno artifacts. Thrown-value preservation and the new package lookup have
not been independently exercised. Cargo warns that `run_aot_script` and its
lookup helpers are unused, correctly reflecting the missing public wiring.

Resume in this order:

1. Add a trusted Script package writer and ABI/Script-goal validation, with
   exact-source/specifier/byte mismatch controls that fail before Script effects.
2. Call `DenoRuntime::run_aot_script` from public `v8__Script__Run` before the
   existing staged paths when a package directory is configured. A missing or
   mismatched configured package must not fall back to matching or interpreting
   source text. With no directory configured, preserve existing behavior.
3. Adopt normal values and thrown values through the existing realm wrappers,
   preserving object identity rather than JSON conversion or a replacement Error.
   Cover throw identity, repeat execution, Context isolation and zero runtime
   compilations/eval instances through the public rusty_v8 surface.
4. Configure or explicitly reject the compiler's pure source Program completion
   path. Rebuild the full Context with the pinned compiler and run unchanged
   Deno/WebIDL conformance before claiming compatibility gains.

No Deno/vendor test or baseline was modified. Unrelated `.tmp/` content is
excluded. Fresh footprint/speed measurements and shared-library factoring
remain outstanding; do not reuse old measurements as results of this change.

## Public source-bound Script execution continuation (2026-10-04)

The previous unwired checkpoint is superseded. Public `v8__Script__Run` now
selects the generic AOT path first when `V8X_JS2WASM_AOT_SCRIPT_DIR` is set,
using the original source and resource name. It reuses either the Context's
Deno bootstrap owner or its source-module owner, clones source before callbacks,
and adopts results through the existing realm wrapper. Original thrown objects
and primitive values are recorded as exceptions without JSON conversion or a
replacement Error. Unconfigured behavior is unchanged. Missing or mismatched
configured packages do not fall back to source matching or interpreting.

`tools/js2wasm/script-packages.mjs` compiles original sources in Script goal
with deferred initialization, persistent bindings and a native completion sink,
then invokes one real Rust packaging test per artifact. The binding uses the
same length-framed UTF-8 digest in JS and Rust. Native bytes and the original
source/options manifest are published before the binding sidecar. Package
directories are trusted build outputs, not adversarial tenant inputs. The
compiler's virtual filename is `script.ts`; resource names such as `<anonymous>`
are preserved separately in the exact binding because TypeScript cannot load
them as virtual source filenames. No wrapper or indirect eval is introduced.

Before Script instantiation, Rust validates the `() -> ()` initializer and
exactly one `(externref) -> ()` completion sink. This initial Script ABI admits
Context imports only and rejects interpreter-provider imports. Direct additional
Deno host capability imports still need deliberate admission and controls.

Eight new native Script packages were built at
`/private/tmp/deno-public-script-packages-20261004`, paired with the existing
Context at `/private/tmp/deno-script-completion.8HrA0L/context.cwasm`.
The public API test passes 1/1 (37 filtered), covering repeat execution, numeric
and undefined completion, returned object identity, thrown object/number/undefined,
two-Context isolation, wrong resource name, missing source and corrupted native
bytes rejected before effects. It asserts zero runtime compilations and zero
runtime-eval provider instantiations. The byte-mismatch control mutates only
the generated marker-test artifact, so generate a fresh package directory for
reproduction or deployment. An earlier repeat-test failure was the test flipping
a corrupted byte back to its valid original; appending a byte makes repeated
corruption controls stable instead of undoing the corruption.

Build-side tests pass 13/13. Native digest and ABI controls pass 2/2, including
wrong initializer/sink signatures and interpreter import refusal. Ordinary
compiler-free adapter tests report 30 passes, zero failures and eight explicit
ignored out of 38. The original native Script environment fixture still passes
1/1 (37 filtered). These are focused controls, not unchanged Deno conformance.

Reproduce build-side packaging, then run only the ignored public control with
the package directory configured. Do not configure it for the whole ordinary
suite, whose historical paths intentionally use different inputs:

```sh
node --experimental-wasm-exnref --import "$JS2_CHECKOUT/node_modules/tsx/dist/loader.mjs" tools/js2wasm/build-public-script-test-packages.mjs "$JS2_CHECKOUT" "$PACKAGING_TEST_BINARY" "$SCRIPT_PACKAGE_DIR"
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR="$FIXTURE_DIR" V8X_JS2WASM_AOT_SCRIPT_DIR="$SCRIPT_PACKAGE_DIR" "$DEPLOYMENT_TEST_BINARY" --exact runs_source_bound_aot_scripts_through_public_api --ignored --nocapture
node --test tools/js2wasm/test-script-packages.mjs tools/js2wasm/test-runtime-compile-options.mjs
cargo test --offline --no-default-features --features js2wasm_spike,js2wasm_gc_copying,js2wasm_diagnostic_abi --lib script_packages::tests -- --nocapture
```

Next: cover additional native Deno capabilities and wider Script semantics,
including BigInt result adoption and lossless UTF-16 strings; configure or reject
pure source Program completion; build a fresh full Deno Context and package its
unchanged Script inputs; rerun full unchanged WebIDL/deno_core conformance.
General runtime AOT source compilation/cache routing remains open. Do not
claim full integration, advance baselines or publish new performance numbers
from these fixture results. Both existing PRs remain draft.

Site text is updated for this public path. Rendering is still unverified:
`make -C site all` fails because `/opt/homebrew/bin/typst` is absent. No Deno
or vendor test was modified and no conformance baseline was advanced.

## Wrap-up checkpoint: fresh Context and unchanged WebIDL (2026-10-04)

This section supersedes the earlier bootstrap ordering and outstanding fresh
Context rebuild statements. The audited, pinned prelinked core bootstrap now
runs before generic package lookup because it creates the Context owner that
independent Scripts require. Original Script state is cloned before callbacks.
Application packages still run before legacy usage matchers; a missing configured
package does not silently fall back. With the same older artifact, unchanged
`webidl::tests::any` changes from 0/1 to 1/1, isolating this ordering repair.

Fresh build directory: `/private/tmp/deno-current-aot-build.q7MC1w`.
Clean source pins are compiler `3d4c1dfdaf61f101cb07c7139b5a3ed65052d520`,
adapter `064423ac4254cc7d2c74e6ad4e1939f8e85c9e8c`, and Deno
`1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44`. The runtime test binary includes
the bootstrap fix in this checkpoint, which is newer than the adapter build pin.
The raw provenance manifest is not edited to pretend otherwise.

The strict runtime/AOT builder produced `deno-core.wasm` (2,709,108 bytes),
SHA-256 `189b1d4aa344bcfb7aba7ddf6ef952cd40d9d0dbed87f1f7972473c80a1b2fe9`,
without interpreter imports. Binaryen 125 `wasm-opt --no-inline -O3
'--pass-arg=no-inline@__new_*' --all-features --disable-custom-descriptors -g`
produced `deno-core.opt.wasm` (2,008,044 bytes), SHA-256
`008a11dd2cda5df5f39c711e94becc550a7af8a41314499712460bbc505fba73`.
Native packaging passed 1/1 (64 filtered) in 208.38 seconds and produced
`deno-core.cwasm` (43,907,288 bytes), SHA-256
`4fc5bbaa0227c00e3b45ab5a893f34c850ab52c861fb4df318db1f0c6c27d284`.
These are artifact sizes and build cost, not RSS, deployment startup or throughput.
The original raw provenance and optimized native attestation remain separate.

`build-webidl-script-packages.mjs` reads the five original literal Script inputs
from the pinned Git version of `libs/core/webidl.rs`, preserving bytes and resource
names. The count is floored at exactly five. Unsupported expressions fail loudly;
the extractor is not a general Rust parser. Packages and input manifest are at
`deno-current-aot-build.q7MC1w/webidl-scripts`. Packaging now supplies a local
attestation output, as required by a precompiler built with the Deno POC feature.

The unchanged Deno checkout is `/private/tmp/deno-upstream-conformance.H6HA4g/deno`;
only Cargo.toml/Cargo.lock are patched. Test binary:
`target/debug/deps/deno_core-87206ac56a2fccad`. With the fresh Context, both the
unconfigured baseline and the five-package candidate report 13/17 passing,
four failing, zero ignored, 414 filtered out of 431. Baseline took 21.56 seconds;
candidate took 20.99 seconds. These timings are not performance benchmarks.
The candidate replaces unknown-source refusals with real assertion failures:

- `dictionary`: field `b` fails conversion to a sequence.
- `sequence_check_next_method_once`: sequence conversion fails.
- `sequence_next_method_must_be_callable`: expected next-method error is absent.
- `sequence_propagates_next_getter_exception`: expected `boom` is absent.

No passing gain or baseline update is claimed. Public AOT Script fixture remains
1/1 (37 filtered). Combined build-side controls pass 15/15. Previous ordinary
adapter results of 30 passing/eight ignored and native ABI controls 2/2 were not
rerun as a full suite after this ordering fix.

Resume by probing owning-module property/call dispatch, not changing Deno tests.
Scripts are retained in `aot_call_graphs` but do not export
`__v8x_graph_can_access_export`/`__v8x_graph_get_export` or the call dispatch pair.
`realm_objects::get` therefore falls back to the Context getter for foreign
Script-created objects. This is a lead, not an attributed root cause. Well-known
Symbol IDs are already stable (iterator is 1); do not blame separate counters
without a probe. Existing Module dispatch is in `js2wasm_graph_calls.rs` and
compiler `examples/v8x-js2wasm-spike/compile-graph.ts`. Do not append Module
exports to original Script sources: preserve Script goal and declarations.
Investigate compiler-owned native interop exports and existing host inspection
helpers, prove attribution with controls, then rerun the unchanged 17 tests.

Remaining scope: full unchanged deno_core population, native host capabilities,
BigInt/lossless UTF-16 result adoption, pure Program completion, runtime AOT
source routing, shared-library factoring and fresh footprint/performance measures.
No new benchmark is credited. Existing draft PRs are loopdive/v8x#2 and
loopdive/js2#6468; the compiler PR is stacked on
`codex/4376-deno-callback-construction-20260930`, not main. Preserve adapter
`.tmp/` and unrelated compiler worktree edits. No test process is left running.

Reproduce the unchanged candidate with the built binary:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-current-aot-build.q7MC1w/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-current-aot-build.q7MC1w/webidl-scripts target/debug/deps/deno_core-87206ac56a2fccad webidl::tests:: --nocapture --test-threads=1
node --test tools/js2wasm/test-runtime-compile-options.mjs tools/js2wasm/test-script-packages.mjs tools/js2wasm/test-rust-script-literals.mjs
```
## Owning-Script dispatch checkpoint (2026-10-04)

Scripts now optionally export native Get and Call helpers, dispatched only after
their allocation owner admits the value. Computed well-known Symbol methods are
materialized in closed object fields using declaration-proven keys and semantic
method names, rather than TypeScript's escaped physical field names. The adapter
validates helper signatures before execution and implements Uint32Value/Int32Value
with JavaScript truncation/wrapping and exception propagation.

Measured unchanged WebIDL result: **15 passed, 2 failed, 0 ignored, 414 filtered
out of 431**, up from 13/17 in the same subset. Newly passing tests are
`sequence_check_next_method_once` and `sequence_next_method_must_be_callable`.
Remaining failures are `dictionary` (array-valued field b) and
`sequence_propagates_next_getter_exception` (expected TypeError("boom")).
No Deno/vendor test sources or passing baselines were changed.

The Context artifact remains the earlier clean compiler `3d4c1df` / adapter
`064423a` build documented above. Five Script packages use this checkpoint's
compiler changes, and the runtime binary uses its adapter changes. This is not
a newly matched full Context rebuild. Current packages:
`/private/tmp/deno-current-aot-build.q7MC1w/webidl-owned-method-scripts`.
Replay from `/private/tmp/deno-upstream-conformance.H6HA4g/deno`:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-current-aot-build.q7MC1w/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-current-aot-build.q7MC1w/webidl-owned-method-scripts target/debug/deps/deno_core-87206ac56a2fccad webidl::tests:: --nocapture --test-threads=1
```

Checks: compiler completion/getter/persistent/result controls **128/128**,
including two existing expected failures; compiler typecheck and scoped lint pass.
Adapter compiler-free ordinary controls **31 passed, 8 ignored, 0 failed /39**,
native Script ABI controls **2/2**, and build-side controls **15/15**.
A separate runtime-compilation profile run reported 33 passes, four failures and
27 ignored /64: two Context/provider contract failures and two missing configured
precompiler inputs. That run is not a passing compiler-free result.

Node Wasm exception-reference support must be supplied as a fork execArgv array.
A misconfigured Vitest launch supplied the flag as characters and left a worker
waiting (session 78409, parent PID 65733, child 65736). Approval to stop it was
requested but not received; it was not killed. Corrected focused checks finished
separately using compiler `.tmp/deno-4376-vitest-exnref.config.ts`.

Resume with native Script-created array iterator support and exact thrown-error
branding/message transport. Do not mask an owning getter's undefined result by
falling back to another module, because that may overwrite intentional shadowing.
Then rebuild a matched Context and run the full unchanged deno_core population.
BigInt/UTF-16, native capabilities, pure Program completion, runtime AOT routing,
shared-library factoring and fresh performance measurements remain open.
The existing compiler PR is stacked, not main-based; both PRs remain drafts.

## Native Error adoption continuation (2026-10-04)

The unchanged next-getter failure is attributed: a direct same-store probe
observes the native exception and its four-unit message, but the Rust wrapper
used to be an ordinary Object. Message::Get then lost the original text.
The adapter now queries each graph's guarded native Error classifier, reads
native name/message fields without invoking public getters, roots both through
the Context keeper and adopts an Error wrapper retaining the original binding.
Error names use owned strings rather than a fixed static-name shortlist.
These are initial field snapshots for message formatting, not proof that later
public name/message redefinitions are reflected in every cached-message API.

Unchanged WebIDL passes 16/17, with dictionary array field b still failing;
zero ignored and 414 filtered out of 431. Context and Script artifacts are the
same older-Context/newer-Script pair in the preceding section. The native getter
probe returns undefined for an owned `[70000]` array, so native builtin iterator
read support remains necessary. Do not mask intentional own undefined shadows
with an undefined-result fallback to the Context getter.

Expanded public Script controls pass 1/1 (38 filtered): native Error branding,
`Uncaught TypeError: boom`, repeated-read object identity, and rejection of an
ordinary object with matching name/message. The same test preserves existing
completion/mismatch/exception controls and asserts zero runtime compilations
and zero interpreter instances. Eleven trusted packages are at
`/private/tmp/deno-public-error-packages.EnQn7D`; its marker package was deliberately
corrupted by the pre-effect refusal control. Build into a fresh directory for
another complete replay. Compiler-free ordinary checks remain 31 passed,
eight ignored, zero failed /39. Build-side checks pass 15/15.

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-script-completion.8HrA0L V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-public-error-packages.EnQn7D target/debug/deps/js2wasm_spike-e7e456f13e693536 --exact runs_source_bound_aot_scripts_through_public_api --ignored --nocapture --test-threads=1
```

Full unchanged population measurement remains outstanding. Cargo nextest is
not installed here, and the repository's deno_core cell requires it. Its libtest
runner is for the separate cargo-self cells, not a Deno fallback, and its default
watchdog kills timed-out tests. Obtain approval or disable that watchdog before
using those cells. Direct per-test libtest replay remains available for the built
Deno binary. No full-population baseline, matched Context rebuild or fresh
performance result is credited.
# Linked Array provider continuation (2026-10-04)

The runtime and small Context builders now export
`__v8x_context_array_prototype(): externref`. Script packaging imports it and
the runtime allowlist admits it; the builder's owner ABI check requires it.
This is native Context capability wiring, not a runtime compiler or interpreter.
Build-side Script/options/source-literal controls pass 15/15.

Companion compiler changes reuse native identity-keyed prototype edges for
linked arrays, preserve explicit null/custom prototypes and avoid static
getPrototypeOf shortcuts for mutated arrays. Focused controls pass 14/14 and
the four-file compiler regression run passes 104/104 (including two existing
expected failures). These are Node controls, not native integration credit.

Existing Context native artifacts do not export the new provider. Rebuild clean,
pinned Context and exact original Script packages before replaying unchanged
Deno. Explicit Reflect receiver propagation and broader cross-module descriptor
and prototype identity checks remain open. Last native unchanged WebIDL result
is still 16/17; no new full-population or performance result is claimed.
