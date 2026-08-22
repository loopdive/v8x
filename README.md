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

## Experimental compiler-free js2wasm backend

`engine_js2wasm` executes a trusted Wasmtime-precompiled js2wasm artifact. It
does not link Cranelift, invoke js2wasm, or require a JavaScript engine at run
time. Deno-shaped functions are ordinary typed imports implemented directly in
Rust; the current vertical slice implements `Deno.cwd()` without a WASI or
Component Model boundary.

Within one process the backend shares one Wasmtime `Engine`, one host-function
`Linker`, and one cached `Module`/`InstancePre` for each artifact. Every v8x
module evaluation receives a separate `Store` and `Instance`, so its WasmGC
heap, globals, permissions, and host state remain isolated.

The development-only `js2wasm_runtime_compile` feature invokes the external
js2wasm compiler and enables Wasmtime's Cranelift precompiler. Set
`V8X_JS2WASM_ARTIFACT_OUTPUT` to save its target-specific `.cwasm` result. A
production build uses only `engine_js2wasm` and loads that immutable artifact
through `V8X_JS2WASM_AOT_MODULE`:

```sh
V8X_JS2WASM_AOT_MODULE=/absolute/path/app.cwasm \
cargo test --release --no-default-features \
  --features engine_js2wasm,simdutf --test js2wasm_spike
```

Generic AOT replay also requires the generated
`<artifact>.graph-sha256` sidecar. v8x checks both its graph digest (the exact
entry point, module specifiers, and source bytes) and its artifact digest before
loading the artifact, so neither side can silently be replaced.

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
