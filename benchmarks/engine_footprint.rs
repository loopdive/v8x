// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

//! v8x backend entry point for the common engine benchmark.

#[cfg(not(any(feature = "engine_quickjs", feature = "engine_js2wasm")))]
compile_error!("engine_footprint requires quickjs or engine_js2wasm");

#[cfg(all(feature = "engine_quickjs", feature = "engine_js2wasm"))]
compile_error!("engine_footprint measures exactly one engine at a time");

const ENGINE_NAME: &str = v8::V8X_ENGINE;

#[cfg(feature = "engine_quickjs")]
const ENGINE_SOURCE: &str = r#"
let answer = 0;
for (let index = 0; index < 32; index++) answer += index;
if (answer !== 496) throw new Error("wrong benchmark result");
export function benchmarkNoop(value) { return value + 1; }
export function benchmarkKernel(iterations) {
  let state = 1;
  for (let index = 0; index < iterations; index++) {
    state = (state * 17 + index) % 1000003;
  }
  return state;
}
"#;

#[cfg(feature = "engine_js2wasm")]
const ENGINE_SOURCE: &str = r#"
let answer: number = 0;
for (let index = 0; index < 32; index++) answer += index;
if (answer !== 496) throw new Error("wrong benchmark result");
export function benchmarkNoop(value: number): number { return value + 1; }
export function benchmarkKernel(iterations: number): number {
  let state: number = 1;
  for (let index: number = 0; index < iterations; index++) {
    state = (state * 17 + index) % 1000003;
  }
  return state;
}
"#;

include!("engine_benchmark_common.rs");
