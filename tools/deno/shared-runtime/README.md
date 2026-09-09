# Experimental shared Deno runtime

This opt-in packaging crate moves the compiler-free v8x/Wasmtime runtime into
a Rust dynamic library. The normal v8x package and its other backends are
unchanged. It does not implement additional Deno APIs.

Use this package as the Deno workspace dependency named `v8`, with
`package = "v8x-shared-runtime"`, a path to this directory, and
`features = ["simdutf"]`. Build the unchanged example with:

```sh
CARGO_PROFILE_RELEASE_PANIC=unwind cargo build -p deno_core --example hello_world --release
```

The unwind setting is required by the prebuilt Rust shared standard library.
Consumers and the library must use the exact same Rust toolchain and compatible
dependencies. This is not a stable C ABI or independently upgradable plugin.
Inspect the executable with `otool -L` on macOS and deploy all referenced
non-system dynamic libraries, not just the smaller executable.

The library retains more public API code than the static example. The
102 entries in diagnostic-symbols.txt were unresolved in the initial macOS
ARM64 dylib link. Packaging-only weak stubs print the exact entry and abort;
they do not report success or silently ignore missing behavior. This remains
a diagnostic prototype, not a production Deno replacement.

The core and provider `.cwasm` files remain separate shared code artifacts.
Moving native runtime code does not shrink those files. Mutable Wasmtime
stores, globals and heaps remain isolated per instance. A smaller executable
alone is not evidence of reduced total deployment size or private memory.

## Verified probe (2026-09-09)

On macOS ARM64 with Rust 1.95.0 and Deno commit
1d4e6c1cb855b62a7fb572c6c138e4e8b4e7fa44, the unchanged
deno_core hello_world example links to libv8.dylib and the matching libstd.
It exits successfully and exactly matches js2wasm-poc-expected.stdout;
stderr is empty (1/1 replay). No Deno source files were changed.

Run from the Deno checkout so rustc resolves its pinned toolchain:

```sh
export DYLD_LIBRARY_PATH="$(rustc --print target-libdir)"
export V8X_JS2WASM_DENO_CORE_AOT_MODULE=/absolute/path/core.cwasm
export V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE=/absolute/path/provider.cwasm
/path/to/target/release/examples/hello_world
```

The development library install name is an absolute build path. A distributable
bundle still needs relocated install names/rpaths and signing. The probe does
not establish a relocatable distribution or Linux/Windows support.

Stripped sizes (strip -x on separate copies):

| Component | Bytes | MiB |
| --- | ---: | ---: |
| Deno example executable | 1,683,096 | 1.6 |
| Shared v8x/Wasmtime library | 6,354,296 | 6.1 |
| Required Rust standard library | 1,205,440 | 1.1 |
| Total native payload | 9,242,832 | 8.8 |

The previous static executable was 2,396,592 bytes (2.3 MiB).
This is a packaging experiment, not a controlled static/dynamic size benchmark:
dynamic linking retains more public methods and uses unwind instead of Deno's
abort panic setting. It increases native payload for one consumer. Multiple
consumers can map the same library files, but physical sharing, marginal
instance memory and throughput have not yet been measured.
