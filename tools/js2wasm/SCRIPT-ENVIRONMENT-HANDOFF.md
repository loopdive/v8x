# Native Script environment checkpoint, 2026-10-04

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
not installed here; the repository harness has a libtest runner, whose default
watchdog kills timed-out tests. Obtain approval or disable that watchdog before
using it, in accordance with the no-test-kill rule. No full-population baseline,
matched Context rebuild or fresh performance result is credited.
