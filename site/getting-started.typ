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
Arbitrary Deno classic scripts and REPL submissions still need `Script::Run`
lifecycle routing into a persistent compiled graph. Build the full interpreter
provider with a current js2wasm compiler, whose standalone target uses the
standardized `try_table` encoding accepted by Wasmtime, statically binds Acorn,
and preserves an ordinary `call; return` boundary instead of `return_call` for
`externref` results. The resulting unoptimized and optimized full providers
pass their eval canaries in Node and Wasmtime. Keep those canaries as production
release gates.

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
