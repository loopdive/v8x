# Native Promise transport checkpoint, 2026-10-04

## Lifecycle enabled in ordinary test package builders

The shared-module, typed-module, module-evaluation, and selected unchanged
Deno module package builders now explicitly request lifecycle events. Generic
packageGraph and the sidecar still default off. No upstream source/test bytes
or Context artifacts changed. Fresh packages are in
`/private/tmp/deno-lifecycle-rollout.ykaQLB`, built with clean compiler
4a98f06ae2, Binaryen 125 O3 and Wasmtime 47.0.3. Adapter runtime is f2a743a.
Build scripts are the only production workflow change in this slice.

Artifact floors: shared 2, typed 2, evaluation 4, Deno graphs 5, Deno Scripts 4.
Every optimized graph retains enter/complete imports: shared/typed each have
two pairs, evaluation graphs one pair, and Deno graphs one pair except the
core-import graph with two. This verifies optimized hook presence, not native
prepared-IR participation or execution of every packaged core source.
Inventory `.cwasm.json` files retain exact source bytes and native/Wasm hashes;
`.graph-sha256` files retain source binding and native artifact hashes.

Fresh native replay passes 4/4, each 1/1 (54 filtered /55): shared dependency
identity/live exports/single execution, typed dependency numeric reads,
successful module namespace publication, and original thrown-object rejection.
The selected unchanged Deno replay passes 5/5, each 1/1 (430 filtered /431):
builtin_core_module, import_meta_resolve, import_meta_filename_dirname,
evaluate_already_evaluated_module, evaluate_already_evaluated_module_sync.
Node fixture/binding/extractor tests pass 7/7 with --experimental-vm-modules.
Builder syntax and diff checks pass. Site rendering still fails because
`/opt/homebrew/bin/typst` is absent. No full-suite or benchmark claim.

Build from the clean compiler checkout so the top-level tsx import resolves:

```sh
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-shared-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-lifecycle-rollout.ykaQLB/shared
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-typed-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-lifecycle-rollout.ykaQLB/typed
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-module-evaluation-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-lifecycle-rollout.ykaQLB/evaluation
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-deno-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-upstream-conformance.H6HA4g/deno /private/tmp/deno-lifecycle-rollout.ykaQLB/deno
```

Native replay from the adapter checkout, selecting the matching graph directory
and exact test named above (success/rejection both use evaluation):

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-lifecycle-rollout.ykaQLB/shared target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
```

Deno replay from the unchanged test checkout, repeating the exact five names:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-promise-full.X2WdwN/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-lifecycle-rollout.ykaQLB/deno/scripts V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-lifecycle-rollout.ykaQLB/deno/graphs target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::builtin_core_module --nocapture --test-threads=1
```

Next: mixed fresh source prefix followed by a cached failing dependency must
deliver the original JS exception through the capability call, without skipping
prefix side effects. Then native prepared-IR participation, cycles/TDZ,
synthetic/source composition, snapshots, broader Deno conformance, host
integration and matched benchmarks. Both PRs remain incomplete drafts.

## Clean native source failure lifecycle replay

The previously failing source-dependency control now passes 1/1 (54 filtered
/55) with clean compiler 4a98f06ae2 packages. Module/Context lifecycle imports
are validated, completion binds the original live namespace using Caller realm
access before publishing Evaluated, and failure propagates only through
executing sources and their consumers. Completed dependencies are not re-read
from a user-visible registry during failure handling. Untouched siblings and
unrelated same-URL Modules are not marked errored.

The strengthened graph includes a successful prefix, a throwing dependency
behind an intermediate module, and an untouched later dependency. Native
exception, rejection and repeated-evaluation identity are retained. Prefix
namespace stays usable after failure; original-owner bump changes 7 to 8,
snapshot returns an object containing 8, a second graph imports the identical
namespace and initializes at 8, and a later bump produces live 9 while that
initialized export remains 8. Prefix executes exactly once and intermediate/
later side effects remain absent. Runtime compiler/interpreter counters are
zero. Identical V8 fixture control passes 1/1.

Additional fixes: generated graph readiness must be hoisted, not an unentered
lexical TDZ; escaped-value tracking initializes lazily; calls can route through
the original proven allocation owner even before entry export readiness.
Compatible closure types or reachability do not authorize another dispatcher.
This call path was verified with mutations, not just a successful return.

Packages `/private/tmp/deno-lifecycle-clean.EsCrQw` are built from the clean
independent compiler checkout `/private/tmp/deno-promise-full.X2WdwN/js2` at
4a98f06ae2. Binaryen 125 O3, Wasmtime 47.0.3, existing small Context unchanged.
The builder explicitly requests `--module-lifecycle true`; normal sidecar
packaging still defaults off. Earlier failing baseline receipts below are
historical and do not describe this expanded fixture.

| Graph | Binding digest | Native SHA256 | Optimized Wasm SHA256 |
| --- | --- | --- | --- |
| entry | a71944ffa13b7807eb408afa60431e6468d1fd0ccf0f33405ae5b97ab4a40ab4 | ec3c2fc5799e83a28e2f47abb3f5d8c30779f2842220279bbe91d3cd21c3677f | ad745734d5c42d55fb0294a0510eb2ecbeb537063042936c658c617dcf781b32 |
| consumer | 2c254348d8b9195dd5359e9b73d4a0235a2beb5206a57ac908cba7c7a9cd6655 | 525f019878683d3d2a097095c36b3fe8ba09bec679fdf0f0d6ab0cdda1e86608 | e0fd2cfb1bcb98da619784fe656a16b140212125f69e8f8d36c20ed5e917d37f |

```sh
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-failed-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-lifecycle-clean.EsCrQw
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-lifecycle-clean.EsCrQw target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_first_dependency_failure_preserves_execution_states --ignored --nocapture --test-threads=1
```

Missing package directory `missing-graphs` fails 0/1, exit 101, at missing
exact binding and prefix-state floor. Filtered adapter library passes 17/17
(16 filtered /33), including preservation of completed/unentered/unrelated
same-URL Modules. Ordinary native controls pass 35 with 19 ignored and one
input-dependent Script test filtered /55. Typed and shared-owner AOT replays
each pass 1/1 (54 filtered /55). Compiler focused controls pass 18/18 across
four files, with typechecking/format/scoped lint/source ratchets passing;
retirement is still not certified. Selected unchanged Deno controls pass 5/5
against the final rebuilt adapter, each 430 filtered /431, using existing older
graph/Script packages and unchanged Context. The full 431-test population is
not certified. Site rendering remains unavailable because Typst is absent.

Next: enable/rebuild ordinary shared module packages with lifecycle events and
verify broader unchanged Deno modules; fix mixed fresh-prefix/cached-failure
delivery using the original JS exception tag; prove native prepared-IR
participation; general cycles/TDZ, synthetic/source composition, snapshots,
remaining host integration and matched benchmarks. Both PRs remain drafts.

## First source dependency failure: failing lifecycle control

New ignored public test
`shared_modules::aot_first_dependency_failure_preserves_execution_states`
reproduces 0/1 (54 filtered /55) with packages built from clean compiler
c5b251bc5f. The executing prefix remains Instantiated instead of Evaluated
after a later source dependency throws. The final test also requires original
object identity, prefix side effect once, untouched later module, exact cached
entry Promise and zero compiler/interpreter activity; those later assertions
are not yet reached. An earlier assertion order also found Context global Get
returned None for the thrown token. Do not claim the full control passes.
Identical fixture sources under Node V8 pass 1/1 (prefix once, later skipped,
same cached thrown object). Ordinary native controls: 35 passed, 19 ignored,
1 environment-dependent Script control filtered /55.

Build-side builder `tools/js2wasm/build-failed-module-test-packages.mjs` retains
raw source fixtures. Binaryen 125 O3, Wasmtime 47.0.3, existing small Context.
Package directory `/private/tmp/deno-first-failure.8HQICU`; binding digest
be525cd78823a793aa884e091cf1753e2e03edd299e7dd918285299501ab7f43.

```sh
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-failed-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-first-failure.8HQICU
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-first-failure.8HQICU target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_first_dependency_failure_preserves_execution_states --ignored --nocapture --test-threads=1
```

Paired compiler adds default-off evaluationHooks with () -> void imports
`<namespace capability>_enter` / `_complete`. Prepared events are inside the
owner guard before body sealing; legacy enter repeats per source entry and
must be idempotent, complete occurs after the last source entry. Native sidecar
does not enable these imports yet. Next bind exact Module/Context events,
publish completed namespaces before later failure, and propagate active source
errors without changing untouched sibling state. Current namespace registry
is published only at the end of entry initialization. Property-read capability
calls are not reliable execution boundaries and must not be used as events.

## Cached dependency failure checkpoint

Continuation: a fresh leading synthetic dependency now executes its callback
before source graph packaging. The expanded public-API regression initially
failed 0/1 on the unsupported synthetic graph error, then passes 1/1 (53
filtered /54) with the original failure on both dependency and consumer,
cached repeated evaluation, no synchronous delivery and exactly one callback
per dependency. The existing cached direct/transitive cases remain covered.
The consumer enters Evaluating before callbacks to prevent recursive startup,
and ModuleState is reacquired after callback execution. Successful synthetic
graph composition and first source-body failure propagation remain unfinished.
Filtered library 16/16 (16 filtered /32), ordinary native 35 passed /18 ignored
/1 filtered out of 54, and typed-owner replay 1/1 (53 filtered /54) pass on
this continuation. Rebuilt selected unchanged Deno tests also pass 5/5, each
430 filtered /431, with existing packages and Context. No compiler production
change or new artifact is required.

The native regression initially failed 0/1 because a cached synthetic failure
was replaced by "source graph contains a synthetic module". Evaluation now
recognizes the next already-failed dependency before packaging, propagates its
original exception through intermediate modules, and caches rejected Promises.
The final public-API regression passes 1/1 (53 filtered /54). It checks direct
and transitive failure, exact object identity, repeated Promise identity, one
callback total, no synchronous TryCatch delivery, preservation of an unrelated
caught exception, and zero runtime compiler/interpreter activity.

The shortcut must stop at an earlier pending dependency without a known next
failure. It must not skip that dependency's side effects. The ordering unit
control covers pending, evaluated, cyclic-prefix and missing-payload cases.
First-failure propagation during flattened source execution, mixed fresh-prefix
failure execution, general cycles/TDZ, and native prepared-IR participation
still need work. This is not complete Deno integration or a performance claim.

Rebuild and replay from this checkout:

```sh
cargo test --offline --no-default-features --features js2wasm_deno_poc,js2wasm_gc_copying,js2wasm_diagnostic_abi --lib js2wasm:: -- --test-threads=1
cargo test --offline --no-default-features --features js2wasm_deno_poc,js2wasm_gc_copying,js2wasm_diagnostic_abi --test js2wasm_spike --no-run
target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::cached_dependency_failure_rejects_with_original_payload_without_reexecution --nocapture --test-threads=1
target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --skip routes_exact_deno_core_scripts_through_public_script_run --test-threads=1
```

The unfiltered library population is not green: it aborts at the unsupported
diagnostic ABI v8__V8__IsSandboxEnabled. The full 431-test Deno population is
not certified. Keep paired PRs 6468 (compiler, stacked) and v8x 2 as drafts.
Temporary package paths below are replay aids, not deployable release inputs.

Final rebuild receipts: filtered adapter library 16/16 (16 filtered /32);
ordinary native controls 35 passed, 18 ignored, 1 filtered /54; typed and
shared-owner AOT replays each 1/1 (53 filtered /54). Rebuilt unchanged Deno
selected controls pass 5/5, each 430 filtered /431, using the existing clean
graph/Script packages and unchanged Context described below. Formatting and
diff checks pass. Site build fails because /opt/homebrew/bin/typst is absent;
no rendered-site validation is claimed.

## Typed live imports through the native module lifecycle

New test `shared_modules::aot_typed_dependency_reads_original_numeric_export`
passes **1/1**, 52 filtered /53. Raw .ts sources are packaged without JS
transpilation. After first graph evaluation, a native call mutates the original
dependency from 77 to 78. Second graph initialization and its reader return 81.
A second native bump to 79 makes the reader return 82; the initialized export
remains 81. Compiler and interpreter runtime counters are zero.
This prevents copied globals or compile-time folding from passing the control.

The earlier compiler lower-level NaN probe is not reproduced here. A public
compiler control under Node/V8 fails on untransported foreign native string
keys and passes with explicit key transport. Native getter currently forwards
raw references and succeeds on these fixtures. Do not credit a production
transport fix or claim the adapter already translates keys.

Clean compiler c5b251bc5f, packages `/private/tmp/deno-typed-live-module.Ijsoss`,
Binaryen 125 O3, Wasmtime 47.0.3. The small Context is unchanged. Build:

```sh
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-typed-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-typed-live-module.Ijsoss
```

Replay from this adapter checkout:

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-typed-live-module.Ijsoss target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_typed_dependency_reads_original_numeric_export --ignored --nocapture --test-threads=1
```

| Entry | Binding | Native SHA256 | Optimized Wasm SHA256 |
| --- | --- | --- | --- |
| first | 9a739035354e76d4cac68e93c6417e4bfe233b40b18b88107acd46978e532831 | 72631428024e839ad0e613dd5acad9e602a3c85709ca8283e72290d7b28e6da0 | c702978e9c92e9b642905801ebf22abc61b3874925f510dbaf68194ea54d9c57 |
| second | deb7babc6071cd1c536da22163b55ea65a965dc53aa5400a0cdd3b01c5508a58 | 133c6eb2cb01d88377e0ba2dc2b1ea1fe7db31756d915d08ed5058d48569d434 | 1b6e7451e3db572ec63eed4c8bca89f738e4e7b57d0a64cd52a9d57e2934f945 |

Missing graph directory `missing-graphs` under this package root fails **0/1**,
exit 101, on missing exact binding and rejected first evaluation. Existing
shared-module test passes **1/1**, 52 filtered /53, using clean c5b251bc5f
packages `/private/tmp/deno-module-prepared-checkpoint.UMyMtG`.
Ordinary controls pass 34 with 18 ignored and 1 environment-dependent Script
test filtered /53. Without the correct filter that test fails on its missing
V8X_JS2WASM_DENO_CORE_FIXTURES input, not a backend regression.

No native prepared-IR floor, full Deno population, snapshots or fresh benchmark
is claimed. Compiler-free typed live imports are verified for this fixture,
not complete Deno integration.

## Clean selected unchanged Deno replay

All five selected unchanged Deno module tests pass **5/5**, each **1/1** with
430 filtered /431, using five graph and four Script packages rebuilt from clean
compiler `285ac9e6f29c2a1ca82067c4e8e4f63c91571422`. Builder succeeds with
explicit `--import tsx` from the compiler checkout. Packages:
`/private/tmp/deno-module-conformance-clean.agpTzp`, Binaryen 125 O3 and
Wasmtime 47.0.3. Original Deno pin remains
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44 with only Cargo.toml/Cargo.lock dirty.
The full Context artifact is unchanged from its earlier compiler pin.

Tests: builtin_core_module, import_meta_resolve, import_meta_filename_dirname,
evaluate_already_evaluated_module and evaluate_already_evaluated_module_sync.
Native builtin graph SHA256:
`bb694c0e355fc58b4f8ed1d17b8995eeb79c592538bb3395a5e9422a177d6d08`;
optimized Wasm SHA256:
`23cc9d5dad463d3e1107175b3bd3dabe9966d4e4542deacd45d0e41dfb8ca683`.
Individual package inventories retain the other hashes.
Negative builtin control with `missing-graphs` fails **0/1**, exit 101, at
exact-binding loading rather than silently reporting empty success.

Replay from the Deno checkout:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-promise-full.X2WdwN/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-module-conformance-clean.agpTzp/scripts V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-conformance-clean.agpTzp/graphs target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::builtin_core_module --nocapture --test-threads=1
```

This supersedes the development-artifact caveat for these five selected tests
only. No full population, Context rebuild or benchmark is claimed. Integration
remains incomplete and both PRs remain drafts.

## Optional imported references verified

Clean compiler `285ac9e6f29c2a1ca82067c4e8e4f63c91571422` now routes optional
imported calls to their original allocation owner. The entire remaining chain
is guarded, including computed keys and arguments. Parenthesized receivers
remain bound; ending an optional chain restores ordinary null-base throwing
before arguments execute. A non-nullish non-callable still evaluates arguments
before throwing. Expanded compiler controls pass **11/11** across three files.

Expanded native shared-module control passes **1/1**, 51 filtered /52, with
final compiler/interpreter counts zero. Node V8 fixture control passes **1/1**;
ordinary native controls remain 34 passed, 17 ignored, 1 filtered /52. Fresh
packages are `/private/tmp/deno-module-optional.lJ8lb6`, Binaryen 125 O3 and
Wasmtime 47.0.3. Prior package hashes below no longer match these fixtures.

| Entry | Binding digest | Optimized Wasm SHA256 | Native SHA256 |
| --- | --- | --- | --- |
| first | 8370a3b39f5842d43dc786dfc9f81f389cab05bb85988bde9fc0662268a36f8d | ec1fcd105eafbd953bad072b17a87a7b4464beb2a60911d5dd7763012a88e621 | 1d2793796b516a3e01f5c4c41120e7f548f0d0e1d6c2a5771eb68f67a5229cce |
| second | 4b8bb15887a10ddcc8eea4a6d5ca076fde88b36071516c57dff29d79173584d5 | 712b324ac1841249b6470612a13b721abdd667fdc75eeabee70251f9ff264c2d | e1ef52d9ad74f6daef5a0b757dc6fb26c7059abb10d52e8de32bf89e8ecf9baa |

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-optional.lJ8lb6 target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
```

Prepared IR initialization, cycles/TDZ, cached failures, full Deno population,
snapshots and complete host integration remain. No fresh benchmark.

## Imported spread calls verified

Compiler `c6dbe274881465e039490d94ec35bbc76ded7437` fixes imported spread
calls that previously bypassed the original allocation owner. Strict native
iteration builds a local argument vector; inline literals use native vector
carriers. Fresh shared-module packages pass **1/1**, 51 filtered /52, covering
namespace/named, empty/mixed, nested spread arguments and non-iterable rejection.
Final runtime compilations and interpreter instantiations are zero. Node V8
control passes **1/1**; ordinary native controls remain 34 passed, 17 ignored,
1 filtered /52. No new unchanged Deno or performance result is claimed.

Packages: `/private/tmp/deno-module-spread.JjnDxv`, clean committed compiler,
Binaryen 125 O3 /Wasmtime 47.0.3. Fixtures changed, so the previous owner-only
packages below are historical and do not match the rebuilt native test.

| Entry | Binding digest | Optimized Wasm SHA256 | Native SHA256 |
| --- | --- | --- | --- |
| first | 9492db3feaba6bd436243124e9035685d36f9ce5d84f416fe8b0c1768248ec94 | 7dc45e5cb25ca74028b6e4810cc094f9462787f637ca5fddd88967e24e443ebe | 9ab1653d9151c512ea593c3d5c082d08d2d8a8d4ed49e54d027a3f77d454897d |
| second | 709e0943115c3db89bd1ec5118fc813a59b5c76ca4c78f0c2d2c47523d6f1d46 | cbaecf7cf841f7ff0eb893999eafcbcb68a14e4515a1db60441fab47bbd3018c | 444df2544f217ac6fddd005a5c54451886d63d8f7c13b2b6e7951efbc5b7b1f8 |

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-spread.JjnDxv target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
```

Next: optional imported calls, getter/iterator ordering controls, prepared IR
initializer guards, cycles/TDZ and cached failures, then clean unchanged Deno
artifacts and broader tests. Both PRs remain incomplete drafts.

## Owner-aware Module calls verified

### Clean shared-module artifact replay

Clean detached compiler `e840ca2ce08b8bc970c60c907ddd23dd0abae9bf`, adapter
builder/native implementation `90323467c858f278db8294e8d068121f311146da`.
Rebuilt shared packages in `/private/tmp/deno-module-owners-clean.WynXRu`,
Binaryen 125 O3 /Wasmtime 47.0.3. Expanded native control passes **1/1**,
51 filtered /52, including final zero compiler/interpreter counters.
This clean receipt covers the shared-module control, not the five Deno packages.

| Entry | Binding digest | Optimized Wasm SHA256 | Native SHA256 |
| --- | --- | --- | --- |
| first | 16173d5026e480a4bd48ea39dc737035903323c69199ae9013700996314520f0 | 8d0de7aa4e091b97200fe02a98a0843ca6a5f13e709b626c8a51abe5621a3de0 | 7a9715fe5ac0821598149b26df91dbb6c2cb9f4267ec23c62c439c44834f33f8 |
| second | c10d47605cf2ed1389e52c98fc37b1fda72e7c86601eacc166b9a7ccb9dbbea5 | d45414789500a3d28bea6ca2b0ba2a4ed99e581c4a4ba4e541dbb7172996f89d | 886c90659420c8b4f5b4d92108918692adea82b3afe9cb816ed4173ea75ade49 |

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-owners-clean.WynXRu target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
```

Compiler focused controls pass **11/11**; typechecking, scoped lint, formatting
and source ratchet command chain pass. Script/graph/extractor controls pass
**10/10**. Typst remains unavailable. Clean Deno artifact replay and remaining
integration work are still next.

The expanded shared dependency control now passes **1/1**, 51 filtered /52,
including live reads, mutations, namespace/bare receivers, same-URL distinct
Modules and zero compiler/interpreter counts. Compiler function-value reads
retain the original namespace's function. Native owner-aware routing now
recognizes Module graph allocation exports as well as Script allocation exports.
Calls use the owner's receiver state, not a compatible closure layout in a
different graph. New owning getter/call exports require allocation proof.

The five selected unchanged Deno module tests pass **5/5**, each selecting 1
with 430 filtered /431. Builtin core import is newly passing. Removing its
graph package fails **0/1** at loading. An exploratory import_meta_ prefix run
passed two tests then aborted at unsupported SnapshotCreator; no full population
claim is made. Existing positive/throwing native AOT controls remain 2/2 and
ordinary scoped controls remain 34 passed, 17 ignored, 1 filtered /52.

Fresh development packages are `/private/tmp/deno-module-linking.pmIWQ4/deno`.
Graphs use Binaryen 125 O3 /Wasmtime 47.0.3; four assertion Scripts were built
with an explicit Node `--import tsx` loader after the combined builder failed
without it. These use a dirty development compiler, not a clean published pin.
Full/small Context artifacts are unchanged. No benchmark was rerun.

Rebuild the original Deno runner using
`RUSTFLAGS='--cfg tokio_unstable' cargo test --offline -p deno_core --lib --no-run`.
This removes the macOS linker configuration failure without editing source/tests.

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-promise-full.X2WdwN/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-module-linking.pmIWQ4/deno/scripts V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-linking.pmIWQ4/deno/graphs target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::builtin_core_module --nocapture --test-threads=1
```

Next: clean pinned artifact replay, prepared IR initialization, cycles/TDZ,
cached failures, optional/spread call coverage and negative owner/capability
controls, snapshots and the full unchanged population. Both PRs remain drafts.
The failing checkpoint below is historical.

## Latest module linking checkpoint: incomplete

The new instance-bound `__v8x_module_namespace_*` imports retrieve evaluated
native Module namespaces with Context/owner checks. The compiler guards legacy
per-source initialization and redirects live imported reads. Native identity,
once-only execution and later named/namespace reads now pass the preceding
assertions. The expanded test still fails **0/1, 51 filtered /52**:
`mutateNamespace` returns **1 instead of 4**. Later named-call, receiver and
same-URL distinct native Module assertions are not reached. Do not suppress this
failure or report the entire shared-module control passing.

Current Node V8 fixture control passes **1/1**, including mutation and receiver
checks. Scoped ordinary adapter run passes **34/34 executed**, 17 ignored,
1 environment-dependent core Script test filtered /52. Cargo formatting passes.
No new unchanged Deno or comparative benchmark result is claimed.

Development graph directory is `/private/tmp/deno-module-linking.pmIWQ4`, built
from the dirty compiler candidate with Binaryen 125 O3 /Wasmtime 47.0.3, not a
clean published compiler pin. Current first/second graph binding digests are
`16173d5026e480a4bd48ea39dc737035903323c69199ae9013700996314520f0` and
`c10d47605cf2ed1389e52c98fc37b1fda72e7c86601eacc166b9a7ccb9dbbea5`.

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-linking.pmIWQ4 target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
node --experimental-vm-modules --test tools/js2wasm/test-shared-module-fixtures.mjs
```

Resume with callable owner routing in compiler `expressions/calls.ts` and
adapter `js2wasm_graph_calls.rs`, then replay all expanded assertions. Compiler
prepared IR M2 initialization is not guarded yet. Malformed bindings,
wrong-Context, cycles/TDZ and failed evaluation need tests. Rebuild clean pinned
packages and the unchanged Deno runner before replaying builtin core import.
The last Deno runner build failed linking (exit 101); the old binary does not
contain this adapter. Inspect linker configuration rather than editing Deno.

Detailed paired handoff:
https://github.com/loopdive/js2/blob/codex/4376-deno-lexical-checkpoint-20261004/plan/agent-context/4376-module-linking-checkpoint-2026-10-04.md
Both existing PRs remain incomplete drafts. The records below are historical.

## Shared dependency regression control

### Within-graph identity fix verified natively

Compiler 1c2f7c35fd3c92cddabd484a07d4881f81ac3446 replaces declaration-keyed
namespace caches with canonical module-symbol caches and handles named imports
of namespace re-exports through that getter. Compiler namespace controls pass
6/6, including gc and standalone identity/liveness checks. Initial six-file run
reports 25 passed /1 failed /26; the same standalone TypeScript namespace
Hole-global failure reproduces on clean ba14fcaedb (7/8) with unchanged tests
and the same harness. No full regression-suite pass is claimed.

The native test now first checks the entry's exported namespace against its
dependency's native namespace. Old packages fail that first check (0/1).
Fresh clean compiler packages make it pass, then retain the measured
cross-graph failure: executions=2, observed=1, same namespace=false, rejected.
Complete regression still fails 0/1, 51 filtered /52; no new Deno test passes.

New package directory: /private/tmp/deno-shared-module-fixed.MBUdR1. Sources,
graph digests, Binaryen/Wasmtime versions and small Context are unchanged.
First native SHA256:
77c6c4c04b5d3ab628eda33fc557329ecdba32c677c1ddc88cca5fc9d7a5d396.
Second native SHA256:
840a62e53aeb58aadca69c58c76a98ed7d7d1c58fd098dc323f50ba6c9cc007c.
Replay the command below with V8X_JS2WASM_AOT_GRAPH_DIR set to that directory.
The clean staged compiler checkout is now 1c2f7c35fd, not ba14fcaedb; historical
artifact build pins remain unchanged. Next implement cross-graph native Module
reuse and live imports together with once-only dependency evaluation.

The generic two-entry control fails **0 passed /1 failed, 51 filtered /52**.
The first entry succeeds and bumps the dependency's mutable count to 2.
Evaluating the second entry then reports executions=2 (expected 1), observed
count=1 (expected 2), different JavaScript namespace identity and a rejected
evaluation Promise. Runtime compilation and interpreter counters remain zero.
This is a semantic defect, not only a namespace publication guard. Do not fix
it by ignoring the guard or aliasing only native wrappers.

The exact same three fixture sources pass **1/1** under Node's V8 module
evaluator. That control also bumps the original dependency again and verifies
both named imports and namespace reads see 3. Those later assertions exist in
the native regression but are not yet reached because the earlier check fails.
This Node control is not a full Deno/V8 comparison or performance benchmark.

Fixtures: tests/fixtures/js2wasm-shared-module/*.js. Rust uses include_str!;
build-shared-module-test-packages.mjs reads the same bytes. Compiler pin
ba14fcaedb, Binaryen 125 O3, Wasmtime 47.0.3; packages are in
/private/tmp/deno-shared-module.SpmGZO. First graph binding is
a43c82a9e3b52c85ddcdbd5f819ca5fd578a0086f11f92256c303661c491df38;
second is 16d940d5d9aa117ff7e1b46c01a4171db60284d446409e562feb40c2faca7da7.
Native hashes and original bytes are recorded in package JSON inventories.

```sh
node --experimental-vm-modules --test tools/js2wasm/test-shared-module-fixtures.mjs
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-shared-module.SpmGZO target/debug/deps/js2wasm_spike-8b524eb9ef0b52c1 --exact shared_modules::aot_shared_dependency_keeps_namespace_live_exports_and_single_execution --ignored --nocapture --test-threads=1
```

Compiler-free test binary built with js2wasm_deno_poc, js2wasm_gc_copying and
js2wasm_diagnostic_abi, without runtime_compile. Existing positive/throwing AOT
module controls still pass 2/2, 50 filtered /52. Scoped ordinary controls pass
34/34 executed, 17 ignored and 1 filtered /52. The filtered original-core
Script test requires a separate fixtures environment: an unconfigured full
run reports 34 passed /1 failed /17 ignored, not an all-green suite.
Existing extractor/graph/Script controls pass 10/10. No runtime fix or new
Deno pass is credited to this test-only change. Typst remains unavailable.

Next implementation must couple three capabilities: skip already evaluated
dependency initializers, return their canonical namespace by captured native
Module identity, and route all named import reads/calls to original live
bindings. NativeModuleGraph already provides instance-bound Module/Context
identity for import-meta; extend that pattern rather than URL-global state.
Changing only namespace materialization leaves named imports on duplicate
globals. Changing only the initializer adapter leaves uninitialized globals.
Preserve cycles, early TDZ, same-URL distinct Modules and failed evaluations.

## Broader unchanged module population

Selected run: **4 passed /1 failed out of 5**, not full Deno integration.
All runs used unchanged Deno 1d4e6c1, compiler-free binary
`deno_core-87206ac56a2fccad`, fresh full Context from the section below and
new exact-source packages in `/private/tmp/deno-module-population.SdW8YU`.
Metadata resolve passes 1/1 (1.79s); filename/dirname 1/1 (1.54s); repeated
evaluation async 1/1 (1.51s) and sync 1/1 (1.64s). Each has 430 filtered /431.
Removing only the assertion Script directory makes async repeated evaluation
fail 0/1 at check1 with an unknown-Script refusal (1.56s). Assertions are real.

`builtin_core_module` fails 0/1 (1.79s) with its full package installed:
"source module namespace was already bound to another value". The source graph
publishes a second core namespace although the native core Module is already
bound to its original Context namespace. Do not bypass this conflict. The next
implementation must reuse canonical native Module namespaces in linked graphs,
retain live exports/import-star identity, and avoid reevaluating already executed
dependency bodies. Use native Module identity, not URL-only lookup. Cover shared
dependencies across entries, same-URL separate Modules and cycles/early access.

Build-side tools added: `deno-module-fixtures.mjs`,
`build-deno-module-test-packages.mjs`, `test-deno-module-fixtures.mjs`.
Extractor reads pinned original Rust source, preserving literal whitespace and
rejecting missing/duplicate declarations, comments and unsupported layouts.
Five graph packages plus four assertion Scripts are built with clean detached
compiler ba14fcaedb, Binaryen 125 O3 and Wasmtime 47.0.3. No source/test edits,
runtime compilation or interpreter are used. Extractor/graph/Script controls
pass 10/10. Native artifact SHA256 are in per-package JSON inventories.

Rebuild from compiler cwd:

```sh
node --experimental-wasm-exnref --import tsx /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/tools/js2wasm/build-deno-module-test-packages.mjs /private/tmp/deno-promise-full.X2WdwN/js2 /private/tmp/v8x-deno-resume-20260930.o0sxeO/repo/target/debug/deps/js2wasm_spike-13b131f10cc30c9d /private/tmp/deno-promise-full.X2WdwN/deno /private/tmp/deno-module-population.SdW8YU
```

Replay from the patched Deno checkout, changing only the exact test selector:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-promise-full.X2WdwN/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-module-population.SdW8YU/scripts V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-module-population.SdW8YU/graphs target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::builtin_core_module --nocapture --test-threads=1
```

Graph digests:

- resolve: 472642c36c38d274179b232c50ddcdded4bd885b826d62729d5f49d2d66363d6
- filename: b048e367b02baca23ae264bc3712a211ad4c91eb20c7735e495439dc09539555
- builtin core: 132783fb316229bbb401075500159535810c5bd7a00af9437a004a77823a98f9
- async repeat: b909e5ec18dc37b20d6020645f948eaf03362d36628e6acff5e0a007e041a129
- sync repeat: 18e0420a743779da09d4b909c3683328a4c57ed80b4d3f433278d483de492906

Compiler entry points: namespace getter construction in
`src/codegen/module-namespace-value.ts`, per-source initializer planning in
`module-init-collection.ts` / `multi-prepared-module-init*`. Adapter:
`collect_graph`, `publish_source_namespaces`, `bind_source_namespace` and
`bind_prelinked_core_namespace`. Preserve the refusal until identity reuse is
implemented and measured. Full population, snapshots, dynamic imports/top-level
await, remaining host/value transport and matched benchmarks remain open.

## Full Context and rooted module exceptions

The full Context was rebuilt from clean detached checkouts: compiler
`b5f6cbae636d22d5c9e7901f779b4f1727003adc`, adapter builder
`7b31b4ef839bbd4646b85441725e65c7f6a95dc6`, original Deno
`1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44`.
Directory `/private/tmp/deno-promise-full.X2WdwN` retains these checkouts,
provenance, raw/optimized/native artifacts and attestation.
Raw SHA256 `c157e9ddab1c7108d3b4c72b93bcf114063194edc260b6b8a7f11263a46cfc59`;
optimized SHA256 `6d3e47c8b926b0e43e0143812101cb8feae3cb78ab1fe39fdfb31f278dec8e79`;
native SHA256 `78aa8a50726f61577cdc54267d912af63acc7b85d24a931fa53e71a63ab2b237`,
44,646,992 bytes. Binaryen 125 O3, all features, no custom descriptors,
debug names and no-inline wildcard. Wasmtime 47.0.3 build-side precompile
passed 1/1, 73 filtered /74, 224.14s. No interpreter provider emitted.

The compiler-free Deno binary was rebuilt against the current adapter:

```sh
RUSTFLAGS='--cfg tokio_unstable' cargo test --offline -p deno_core --lib --no-run
```

Unchanged main/side passes 1/1, 430 filtered /431 (latest 1.94s).
With graph packages absent it fails 0/1 with the actual missing-artifact error
in `exception_message`, no longer an unsupported Promise conversion error.
Lazy loading passes 1/1 and WebIDL 17/17. These remain subset evidence, not
full population credit. Replay from the patched Deno checkout:

```sh
V8X_JS2WASM_DENO_CORE_AOT_MODULE=/private/tmp/deno-promise-full.X2WdwN/deno-core.cwasm V8X_JS2WASM_AOT_SCRIPT_DIR=/private/tmp/deno-reentrant-script.MZdH2Q/deno-scripts V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-promise-full.X2WdwN/graphs-owned-get target/debug/deps/deno_core-87206ac56a2fccad --exact modules::tests::main_and_side_module --nocapture --test-threads=1
```

For the negative control omit only V8X_JS2WASM_AOT_GRAPH_DIR. For WebIDL use
`webidl::tests::` instead of the exact main/side selector.

`run_graph_module_init` now roots a pending externref payload in the Context
keeper before returning an error. The native module rejection captures that
handle into the original exception value. A new throwing graph test verifies
the rejected Promise result, Module exception and escaped global object have
the same identity. Initially marker reads returned NaN/42 because the namespace
matcher stays unready after an abrupt initializer. The compiler sidecar now
exports a getter for allocation-proven objects; native dispatch checks the
graph's ownership predicate before using it. Old packages with proven ownership
but no getter fail loudly. No bypass is allowed for foreign objects.

Native positive/throwing AOT module controls pass 2/2, 48 filtered /50, including
marker 42, cached evaluation Promise and zero compilation/interpreter counts.
Compiler namespace controls pass 4/4, including foreign-instance allocation
refusal with the same GC layout. TS7 passes; scoped lint retains one existing
explicit-any warning. Ordinary adapter suite 34 passed, 0 failed, 16 ignored
/50; scoped units 15/15, 16 filtered /31. Formatting and diff checks pass.

Candidate graph packages in `graphs-owned-get` were built from the working
compiler with this sidecar change, not the clean full-Context compiler pin.
Each inventory records exact source and optimized/native SHA256. The source-bound
throw digest is `053279d44c82aaa1fedc04b365481ae3cfe294b5ebf6816dc4293753f0cfa8fe`.
Native control:

```sh
V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR=/private/tmp/deno-native-promise.6898GB V8X_JS2WASM_AOT_GRAPH_DIR=/private/tmp/deno-promise-full.X2WdwN/graphs-owned-get target/debug/deps/js2wasm_spike-e7e456f13e693536 aot_module_ --ignored --nocapture --test-threads=1
```

Next derive the full unchanged test/source population, package its exact module
graphs and continue snapshots and host services. Startup-module exceptions,
arbitrary thrown callables, failure after mirror adoption and full snapshot
semantics remain unverified. No new benchmark. Typst still unavailable.

The sections below are historical and superseded where this section provides
new evidence. Neither PR is merge-ready and the full integration goal remains.

## Native validation continuation

The native mirror now has an executed compiler-free control: 1/1, 48 filtered
/49, covering fulfillment/rejection before and after transfer, exact native
Promise and payload identity, queued compiled reactions, one rejection event,
one late-handler event and repeated refusal of unsupported BigInt payloads.
Runtime compilations and interpreter instances are zero. Existing compiled
rejection controls pass 3/3, 46 filtered /49, including reentry and cross-realm
enqueue ordering. These are adapter controls, not unchanged Deno coverage.

Settled payload graphs are inspected before publication, including Promise
payload edges. Errors, buffers and supported typed arrays remain transferable.
Failed mirror initialization removes its identity binding and registry entry.
Test Context attachment now publishes runtime owner identity. Settling an
already-published mirror with an unsupported value and arbitrary Wasmtime-call
failure rollback still need broader handling; do not claim full transactionality.

Fresh small Context: `/private/tmp/deno-native-promise.6898GB/context.cwasm`,
19,883,016 bytes, SHA256
`52530264bf83966d5049409029fa4b7819dc1e881758c775fd8e3f7234e23123`.
Raw Context SHA256 `507b7f9135de83edafeb26f57e26b3dccbe9cbb55ede856415226577374896d7`;
optimized SHA256 `6f91022d2ed54e8d72074bda2e92a509cc17dccbf0b2826f75dc5b39ff1563c3`.
Built with compiler implementation 74ed7007fb and adapter bridge 830a3f8;
Binaryen 125 `-O3 --all-features --disable-custom-descriptors -g --no-inline=*`,
Wasmtime 47.0.3 build-side precompile test passed 1/1, 73 filtered /74 (108.83s).

Replay command in adapter checkout:

```sh
V8X_JS2WASM_REJECTION_CONTEXT=/private/tmp/deno-native-promise.6898GB/context.cwasm target/debug/deps/js2wasm_spike-e7e456f13e693536 --exact native_promise_mirror_retains_identity_and_single_rejection --ignored --nocapture --test-threads=1
```

Next: rebuild full clean pinned Deno Context with the new bridge, then replay
the original missing-graph rejection and positive main/side tests. The full
Context is still old. No new unchanged Deno result or performance measurement.

## Original checkpoint (historical)

Unfinished implementation, not merge-ready. Paired PRs:
https://github.com/loopdive/v8x/pull/2 and https://github.com/loopdive/js2/pull/6468.
Compiler checkpoint: 74ed7007fb. Preceding verified adapter: 37923f2.

## Implementation

Rust-created Promises have a transport path into a real compiled Promise in
the owning Context. Regular realm bindings retain original native identity;
a mirror registry retains the resolver packet and synchronizes settlement
through the active owner or callback caller. Ordinary compiled Promise
resolution queues reactions. Exact mirrored rejection events are suppressed
during synchronization; the original native settlement remains the notification
source. Other Promise events are not suppressed. No interpreter was added.

Entry points: `src/js2wasm/realm_native_promises.rs`, realm object/value
transport, native settlement in `src/js2wasm/mod.rs`, Context value bridge.

## Verification

Compiler-free cargo check, cargo formatting and diff whitespace checks pass.
Runtime compiler option controls: 11/11. Staged-core runner: PASS, including
pending state, fulfillment/rejection, stable compiled identity, asynchronous
reactions, immutable settlement and invalid settlement flags. This exercises
the compiled helpers, not the Rust mirror transport.
Ordinary compiler-free adapter suite: 34 passed, 0 failed, 14 ignored /48.
No new native Promise integration pass, unchanged Deno pass or benchmark.
Earlier Deno evidence remains main/side 1/1, lazy/missing script 2/2 and WebIDL
17/17, subsets of 431 tests, before this checkpoint.

Existing Context artifacts lack the three new Promise helper exports:
`/private/tmp/deno-call-order-native.dBQZ3T/deno-core.cwasm` and
`/private/tmp/deno-reentrant-script.MZdH2Q/context-fixtures/context.cwasm`.
No replacement artifact was produced. Rebuild before claiming native ABI proof.

## Next steps

1. Audit failure rollback. Bindings are published before settled payload
   conversion; unsupported payloads may leave partial mirrors. Native state
   is also settled before synchronization can fail. Define cleanup explicitly.
2. Reuse `build-namespace-test-context.mjs ... --rejection-events`, with the
   updated bridge and real rejection dispatcher. There is no compiler option
   `standalonePromiseRejectNotifyImport`; the redundant unrun builder using
   it was removed. Optimize with Binaryen 125 and precompile with Wasmtime 47.0.3.
3. Publish test Context owner identity before mirror use. Production graph
   publication already does this. Add native controls for pending/pre-settled
   transport, exact payload identity, callback order, duplicate settlement,
   one unhandled rejection and late-handler notification. Verify zero runtime
   compiler calls and interpreter instances.
4. Rebuild a clean pinned full Deno Context. Replay unchanged missing-graph
   rejection and positive main/side tests. Never modify Deno test sources.
5. Continue original AOT thrown-payload rooting, full population/module coverage,
   snapshots, host services, shared libraries and matched V8/QuickJS/Porffor
   measurements. Full Deno integration remains unfinished.

Typst is unavailable; site rendering is unverified. Preserve adapter `.tmp/`
and unrelated compiler worktree changes. Earlier detailed artifact receipts
and commands are retained in `SCRIPT-ENVIRONMENT-HANDOFF.md` and compiler issue
4376, "Spike v8x as a rusty_v8-compatible js2wasm backend for a compiler-free Deno runtime".
