`v8x` makes rusty_v8 engine agnostic.

Docs: https://littledivy.github.io/v8x · Test dashboard: https://littledivy.github.io/v8x/status/

```diff
-v8 = "0.155.0"
+v8 = { package = "v8x", version = "0.155.0", features = ["jsc"] }
```

Supported engines:

- V8 14.9.207.2-rusty
- JavaScriptCore / WebKit 625.1+ and System-framework path uses the OS's JSC.
- QuickJS-ng 0.15.1

## Experimental js2wasm backend

The backend has two deployment profiles:

- `engine_js2wasm` is compiler-free. It only executes trusted,
  Wasmtime-precompiled js2wasm artifacts and is the intended profile for
  closed-world applications such as `deno compile` output.
- `engine_js2wasm_runtime` invokes a shipped js2wasm compiler on a cache miss
  and stores a target-native, content-addressed artifact. It is intended for
  repeated `deno run`-style invocations and other hosts whose module source is
  not known when the v8x binary is built.

The compiler-free profile does not link Cranelift, invoke js2wasm, or require a
JavaScript engine at run time. Deno-shaped functions are ordinary typed imports
implemented directly in Rust; the current vertical slice implements
`Deno.cwd()` without a WASI or Component Model boundary.

Within one process the backend shares one Wasmtime `Engine`, one host-function
`Linker`, and one cached `Module`/`InstancePre` for each artifact. Every v8x
module evaluation receives a separate `Store` and `Instance`, so its WasmGC
heap, globals, permissions, and host state remain isolated.

For a compiler-free application, generate the artifact during packaging, then
load it through `V8X_JS2WASM_AOT_MODULE`:

```sh
V8X_JS2WASM_AOT_MODULE=/absolute/path/app.cwasm \
cargo test --release --no-default-features \
  --features engine_js2wasm,simdutf --test js2wasm_spike
```

Generic AOT replay also requires the generated
`<artifact>.graph-sha256` sidecar. v8x checks both its graph digest (the exact
entry point, module specifiers, and source bytes) and its artifact digest before
loading the artifact, so neither side can silently be replaced.

The runtime profile accepts either a standalone graph compiler through
`V8X_JS2WASM_COMPILER`, or `V8X_JS2WASM_COMPILER_SCRIPT` with
`V8X_JS2WASM_COMPILER` (default: `node`). Both implement the same
`--manifest FILE --entry URL --output FILE` protocol.
`V8X_JS2WASM_WORKDIR` controls the compiler's working directory. The cache key
covers the exact module graph, compiler identity, v8x/Wasmtime versions, OS,
and architecture. Set `V8X_JS2WASM_CACHE_DIR` to control the cache location or
`V8X_JS2WASM_COMPILER_ID` to provide a release identifier for a packaged
compiler. Without that explicit identifier, v8x derives one from the compiler
script, executable version, and package lockfiles. The legacy
`js2wasm_runtime_compile` feature name remains as a compatibility alias.

```sh
V8X_JS2WASM_COMPILER_SCRIPT=/absolute/path/compile-graph.ts \
V8X_JS2WASM_WORKDIR=/absolute/path/js2 \
cargo test --release --no-default-features \
  --features engine_js2wasm_runtime,simdutf --test js2wasm_spike
```

Compiled graphs that import `js2wasm:runtime-eval` can link js2wasm's existing
zero-import interpreter provider. In the runtime profile, point
`V8X_JS2WASM_RUNTIME_EVAL_WASM` at its raw Wasm; v8x precompiles and caches it,
then instantiates a fresh provider in the application's own Wasmtime store.
Provider exports are registered directly as the graph's four runtime-eval
imports, so WasmGC global objects, closures, and mutable binding cells cross
the boundary without a Rust value-copy layer.

Set `V8X_JS2WASM_RUNTIME_EVAL_AOT_OUTPUT` while packaging to save the provider's
native artifact. A compiler-free application loads that artifact through
`V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE` alongside `V8X_JS2WASM_AOT_MODULE`.
Both native artifacts must be produced by the same trusted pipeline and
Wasmtime configuration.

This completes the v8x side of runtime `eval`/`Function` linking inside a
compiled module graph. Arbitrary Deno classic-script and REPL submissions need
to be routed into a persistent compiled graph; that is a v8x `Script::Run`
lifecycle task, not a missing runtime-eval realm ABI.

Build the provider with a js2wasm revision that emits standardized `try_table`
exception handling for standalone targets. Current js2wasm does so; older
compiler bundles emitted the withdrawn legacy `try`/`catch` encoding rejected
by Wasmtime. The v8x integration test keeps a narrow provider fixture for fast
ABI and compiler-free replay coverage. Current js2wasm statically binds Acorn
without carrying its parser through a generic function value, and keeps an
ordinary `call; return` boundary instead of `return_call` for `externref`
results. The resulting unoptimized and optimized providers pass their eval
canaries in Node and Wasmtime. Keep those canaries as a release gate.

Wasmtime precompiled artifacts contain native executable code. They must come
from a trusted build pipeline using the same Wasmtime version, target, and
engine configuration; never load an artifact supplied by an untrusted user.
The optional `js2wasm_diagnostic_abi` weak-stub layer is Unix-only and is not a
production runtime feature; MSVC is intentionally unsupported for that layer.

For unchanged `deno_core`, build v8x with `engine_js2wasm` and the temporary
Unix-only `js2wasm_diagnostic_abi` link-completion layer. The Rust-owned ABI
surface handles contexts, values, callbacks, and conversions directly. Audited
bootstrap scripts and later classic scripts run in one persistent
JS2/Wasm/Wasmtime instance per Deno context; no QuickJS engine is linked into
this backend. Packaged runs set `V8X_JS2WASM_DENO_CORE_AOT_MODULE` and, when the
application needs runtime-created source strings, set
`V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE` to trusted caches created by the same
Wasmtime build.

### Bounded Deno `hello_world` POC

`tools/deno/run-js2wasm-poc.sh` is a non-ignored Linux x86_64 gate for one
closed-world claim: the unmodified `deno_core` `hello_world` example at the
pinned Deno commit executes its six enumerated inputs through a JS2-produced,
Wasmtime-AOT application and interpreter provider. It is not evidence for the
Deno CLI, arbitrary `Script::Run` input, snapshots, extensions, or general
Deno compatibility.

The runner requires clean detached worktrees: the current v8x commit is
recorded exactly, JS2 is fixed at
`9bda388e593cbf9631dc7c4f2c4016685d357587`, and Deno is fixed at
`1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44`. It reads all Deno source through
`git show <pinned-ref>:path`, including the raw Rust string literal in
`libs/core/examples/hello_world.rs`; it does not use a checked-out fixture or
handwritten copy. The graph includes pristine `mod.js`, and the exact usage
literal is embedded and evaluated by the direct JS2 interpreter provider rather
than recreating its print/sum sequence in the adapter.

The CI gate uses Rust 1.95.0, matching the pinned Deno checkout's toolchain and
satisfying the pinned Wasmtime 47.0.3 dependency graph.

```sh
tools/deno/run-js2wasm-poc.sh \
  --v8x=/absolute/path/to/v8x \
  --js2=/absolute/path/to/clean-detached-js2 \
  --deno=/absolute/path/to/clean-detached-deno \
  --out-dir=/absolute/path/to/an-empty-output-directory
```

The trusted packaging phase may invoke Node, JS2, and the runtime-compilation
feature. It writes `raw-inputs.json`, raw `deno-core.wasm` and
`runtime-eval-provider.wasm`, then same-host native artifacts, a strict
per-artifact `.attestation.json` sidecar, and a strict `poc-lock.json`. Each
sidecar binds its role, Wasmtime 47.0.3 target and engine configuration, raw
Wasm SHA-256, and AOT SHA-256; the lock commits the canonical sidecar digest.
That raw provenance binds clean revisions, source byte lengths
and SHA-256s, compiler/interpreter inputs, canonical compile options, raw Wasm
hashes, and the Wasmtime 47.0.3 Linux x86_64 engine configuration. A canonical
raw contract digest commits the complete source graph, generated adapter,
direct interpreter provider graph, raw artifacts, compiler options, revisions,
target, and engine flags before native precompilation. The final replay contract
adds both native AOT digests and is embedded into the Deno build, so a modified
lock plus replacement artifacts cannot pass as the packaged proof.

The replay phase moves only `poc-lock.json`, `deno-core.cwasm`, and
`runtime-eval-provider.cwasm` into a fresh directory. It modifies only Deno's
workspace `v8` dependency plus `Cargo.lock` to select
`js2wasm_deno_poc_replay`; the lock change is a committed patch with an exact
post-apply SHA-256, so CI never re-resolves the graph against a changing
registry. It then builds the genuine Deno example and starts it with `env -i`.
The process receives only the manifest and two AOT paths: no raw Wasm, Node,
JS2 compiler, runtime-compilation feature, or QuickJS feature is present.
It requires exit status zero, byte-for-byte stdout matching
[`tools/deno/js2wasm-poc-expected.stdout`](tools/deno/js2wasm-poc-expected.stdout),
and empty stderr. The manifest also binds both native artifacts and the complete
build contract; tamper and configuration controls run in the same CI job.

Swap the engine under Deno without touching `deno_core`:

```toml
# deno's workspace Cargo.toml
v8 = { package = "v8x", version = "0.155.0", features = ["jsc"] }
```

```diff
- cargo build -p deno
+ cargo build -p deno --features hmr
```

`v8x` vendors the real `v8` crate's Rust source and implements the `v8__*` C ABI
on the chosen engine, so the swap is a drop-in — `deno_core` compiles unchanged.

| engine          | deno size | engine size   |
| --------------- | --------- | ------------- |
| Deno V8 14.9    | 78.7 MB   | ~40 MB static |
| Deno JSC        | 80.7 MB   | ~48 MB static |
| Deno system JSC | 54.2 MB   | 0             |
| Deno quickjs-ng | 56.1 MB   | ~1 MB static  |
