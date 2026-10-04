#import "./shim/html.typ": *

#set document(
  title: "getting started · v8x",
  description: "Swap the JavaScript engine under rusty_v8 or Deno with a one-line Cargo change.",
)

#show: html-shim

= Getting started

== In your own crate

Replace the `v8` dependency with `v8x` and pick an engine feature:

```toml
[dependencies]
v8 = { package = "v8x", version = "149.4.0", features = ["quickjs"] }
```

Engine features are mutually exclusive; enable exactly one:

#table(
  columns: 3,
  [*feature*], [*engine*], [*notes*],
  [`quickjs`], [QuickJS-ng, vendored + static], [works everywhere, \~1 MB, fastest build],
  [`jsc`], [WebKit JSCOnly, built from source], [macOS; shippable, JIT-enabled],
  [`system_jsc`], [Apple's `JavaScriptCore.framework`], [macOS; zero engine bytes in your binary],
  [`engine_js2wasm,js2wasm_diagnostic_abi`], [js2wasm AOT modules on Wasmtime], [experimental; unsupported V8 ABI calls abort with their symbol name],
  [`engine_js2wasm_runtime`], [js2wasm compiler + native artifact cache], [experimental; compiles module-graph cache misses at run time],
)

Your code keeps using the `v8` crate API: `v8::Isolate`, `v8::Local`,
handle scopes, all of it. Nothing else changes.

== Under Deno

Patch the workspace instead, so `deno_core` and everything above it picks up
the swap:

```toml
# deno's workspace Cargo.toml
[patch.crates-io]
v8 = { package = "v8x", version = "149.4.0", features = ["jsc"] }
```

```sh
cargo build -p deno
```

`deno_core` compiles unchanged; the resulting binary runs your JS on the
engine you selected. The experimental js2wasm backend currently passes 132 of
429 `deno_core` tests and is not yet a complete Deno runtime.

Use compiler-free `engine_js2wasm` for closed-world `deno compile`-style
artifacts. Use `engine_js2wasm_runtime` when source arrives after startup: it
ships or locates the js2wasm compiler and caches target-native artifacts by
module graph and compiler identity. Graphs using dynamic code can link the
existing zero-import `js2wasm:runtime-eval` provider in the same Wasmtime store,
preserving global objects and mutable binding cells across the module boundary.
Known classic Script sources can use `V8X_JS2WASM_AOT_SCRIPT_DIR` with trusted
build-side packages. Public `Script::Run` selects an exact source/resource-name
binding, validates native completion signatures before instantiation, and adopts
results and thrown values through the owning Context. A configured missing or
mismatched package fails without interpreter fallback. Focused compiler-free
controls cover repeat execution, Context isolation and object/exception identity;
this is not full unchanged Deno conformance.
The pinned core bootstrap runs before package lookup to create the Context owner.
With a fresh optimized Context and five original Script packages, unchanged
WebIDL checks pass 17 of 17. This is a focused replay, not the full population.
Known `CompileFunction` bodies use trusted function-factory packages binding the
body, parameter names and resource name. Compilation creates a native callable
without executing its body. Active host callbacks instantiate and call these
functions in the same store. The unchanged lazy-script test now passes, including
cached export identity and a lazy dependency. Context extensions, V8 code-cache
consumption and alternate receivers for foreign Script getters remain unsupported.
Arbitrary new classic scripts and REPL submissions still need general AOT
compilation/cache routing. Build the full interpreter
provider with a current js2wasm compiler, whose standalone target uses the
standardized `try_table` encoding accepted by Wasmtime, statically binds Acorn,
and preserves an ordinary `call; return` boundary instead of `return_call` for
`externref` results. The resulting unoptimized and optimized full providers
pass their eval canaries in Node and Wasmtime. Keep those canaries as production
release gates.

The runtime-profile Deno artifact publishes its context owner before module
initialization and attaches host globals before core wrappers capture them.
Focused multi-module tests cover host callbacks during initialization and
retention of the original realm after a failed initializer. This is not yet
verification of a complete Deno runtime. A pinned unchanged core/hello-world
fixture also exercises native pending-op settlement and rejection identity.
The `js2wasm_deno_poc` feature selects that acceptance fixture without enabling
a runtime compiler; build-time precompilation additionally requires
`js2wasm_runtime_compile`. Arbitrary new programs and full Deno compatibility
remain separate requirements.

The runtime artifact builder now requests the compiled enqueue notification
and rejects output without the single-job drain and pending-count functions.
AOT mode links only native host capabilities; an interpreter provider remains
an explicit dynamic-code fallback. The historical POC compile commitment is
unchanged. The runtime compiler pin now includes this scheduler ABI and the
clean detached raw-artifact build is verified for the pinned program.
Distribution packaging and broader release validation remain required.

Separately evaluated extension and application graphs can be packaged with
`V8X_JS2WASM_ARTIFACT_OUTPUT_DIR` during build-time execution. Compiler-free
replay selects each exact graph from `V8X_JS2WASM_AOT_GRAPH_DIR`, verifying its
source and native-code binding before loading. These graphs share the Deno
context's store and realm. Only evaluated graphs are packaged; missing graphs
fail without invoking a compiler.

== macOS note: JIT entitlements

JavaScriptCore's JIT needs permission to allocate executable memory. Binaries
using the `jsc` backend must be codesigned with the JIT entitlement:

```sh
codesign -s - -f --entitlements tools/jit-entitlements.plist ./your-binary
```

The `system_jsc` and `quickjs` backends don't need this. The test harness in
the v8x repo does it automatically.

== Building from the repo

```sh
git clone --recursive https://github.com/littledivy/v8x
cd v8x
cargo build --no-default-features --features quickjs   # no engine build step
cargo build --features jsc                             # builds WebKit from source
```

The first vendored JSC build compiles WebKit's JSCOnly target, which takes a
while. QuickJS builds in seconds.
