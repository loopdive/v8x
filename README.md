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
ABI and compiler-free replay coverage. The full Acorn/interpreter provider now
validates and precompiles in Wasmtime, but its own eval canary still throws at
execution there despite passing in Node; that engine-parity bug is separate
from v8x's provider linking and must be resolved before shipping that provider.

Wasmtime precompiled artifacts contain native executable code. They must come
from a trusted build pipeline using the same Wasmtime version, target, and
engine configuration; never load an artifact supplied by an untrusted user.
The optional `js2wasm_diagnostic_abi` weak-stub layer is Unix-only and is not a
production runtime feature; MSVC is intentionally unsupported for that layer.


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

| engine | deno size | engine size |
| --- | --- | --- |
| Deno V8 14.9 | 78.7 MB | ~40 MB static |
| Deno JSC | 80.7 MB | ~48 MB static |
| Deno system JSC | 54.2 MB | 0 |
| Deno quickjs-ng | 56.1 MB | ~1 MB static |
