// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

//! v8x backend entry point for the common engine benchmark.

#[cfg(not(any(feature = "engine_quickjs", feature = "engine_js2wasm")))]
compile_error!("engine_footprint requires quickjs or engine_js2wasm");

#[cfg(all(feature = "engine_quickjs", feature = "engine_js2wasm"))]
compile_error!("engine_footprint measures exactly one engine at a time");

const ENGINE_NAME: &str = v8::V8X_ENGINE;

#[cfg(feature = "engine_quickjs")]
const ENGINE_SOURCE: &str = r#"
import { addJS, complexKernel } from "./v8x-engine-workload.ts";
export function benchmarkAddDynamic(value) { return addJS(value, 1); }
export function benchmarkAddConstant(_value) { return addJS(1, 2); }
export function benchmarkComplex(seed) {
  return complexKernel(seed);
}
"#;

#[cfg(feature = "engine_quickjs")]
const ENGINE_WORKLOAD_SOURCE: &str = r#"
export function addJS(left, right) { return left + right; }
export function complexKernel(seed) {
  let state = seed % 1000003;
  let checksum = 0;
  for (let round = 0; round < 512; round++) {
    state = (state * 48271 + round + 1) % 1000003;
    if (state % 2 === 0) {
      state = (state / 2 + 7919) % 1000003;
    } else {
      state = (state * 3 + 1) % 1000003;
    }
    checksum = (checksum + state * ((round % 17) + 1)) % 1000003;
  }
  return checksum;
}
"#;

#[cfg(feature = "engine_js2wasm")]
const ENGINE_SOURCE: &str = r#"
import { addJS, complexKernel } from "./v8x-engine-workload.ts";
export function benchmarkAddDynamic(value: number): number {
  return addJS(value, 1);
}
export function benchmarkAddConstant(_value: number): number {
  return addJS(1, 2);
}
export function benchmarkComplex(seed: number): number {
  return complexKernel(seed);
}
"#;

#[cfg(feature = "engine_js2wasm")]
const ENGINE_WORKLOAD_SOURCE: &str = r#"
export function addJS(left: number, right: number): number {
  return left + right;
}
export function complexKernel(seed: number): number {
  let state: number = seed % 1000003;
  let checksum: number = 0;
  for (let round: number = 0; round < 512; round++) {
    state = (state * 48271 + round + 1) % 1000003;
    if (state % 2 === 0) {
      state = (state / 2 + 7919) % 1000003;
    } else {
      state = (state * 3 + 1) % 1000003;
    }
    checksum = (checksum + state * ((round % 17) + 1)) % 1000003;
  }
  return checksum;
}
"#;

include!("engine_benchmark_common.rs");
