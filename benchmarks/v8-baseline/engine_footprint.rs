// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

//! Real rusty_v8 entry point for the common engine benchmark.

const ENGINE_NAME: &str = "v8";

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

include!("../engine_benchmark_common.rs");
