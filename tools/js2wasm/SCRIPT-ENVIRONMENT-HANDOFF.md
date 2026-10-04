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
