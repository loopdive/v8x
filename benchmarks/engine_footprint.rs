// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

//! Process-level engine footprint benchmark.
//!
//! Build this target separately with `quickjs` and `engine_js2wasm`. The
//! companion shell runner does that in isolated Cargo target directories so
//! the linked executable sizes and process measurements cannot be mixed.

use std::sync::Once;
use std::time::{Duration, Instant};

#[cfg(not(any(feature = "engine_quickjs", feature = "engine_js2wasm")))]
compile_error!("engine_footprint requires quickjs or engine_js2wasm");

#[cfg(all(feature = "engine_quickjs", feature = "engine_js2wasm"))]
compile_error!("engine_footprint measures exactly one engine at a time");

const MODULE_NAME: &str = "file:///v8x-engine-footprint.ts";
#[cfg(feature = "engine_quickjs")]
const ENGINE_SOURCE: &str = r#"
let answer = 0;
for (let index = 0; index < 32; index++) answer += index;
if (answer !== 496) throw new Error("wrong benchmark result");
export function benchmarkResult() { return answer; }
"#;
#[cfg(feature = "engine_js2wasm")]
const ENGINE_SOURCE: &str = r#"
let answer: number = 0;
for (let index = 0; index < 32; index++) answer += index;
if (answer !== 496) throw new Error("wrong benchmark result");
export function benchmarkResult(): number { return answer; }
"#;

fn initialize() {
  static ONCE: Once = Once::new();
  ONCE.call_once(|| {
    v8::V8::initialize_platform(
      v8::new_unprotected_default_platform(0, false).make_shared(),
    );
    v8::V8::initialize();
  });
}

#[derive(Clone, Copy)]
struct ProcessMemory {
  rss_kib: i64,
  vsz_kib: i64,
}

fn process_memory() -> ProcessMemory {
  let output = std::process::Command::new("ps")
    .args([
      "-o",
      "rss=",
      "-o",
      "vsz=",
      "-p",
      &std::process::id().to_string(),
    ])
    .output()
    .expect("run ps to measure this benchmark process");
  assert!(output.status.success(), "ps failed: {output:?}");
  let fields = std::str::from_utf8(&output.stdout)
    .expect("ps output is UTF-8")
    .split_whitespace()
    .map(|field| field.parse::<i64>().expect("ps field is numeric"))
    .collect::<Vec<_>>();
  assert_eq!(fields.len(), 2, "unexpected ps output: {output:?}");
  ProcessMemory {
    rss_kib: fields[0],
    vsz_kib: fields[1],
  }
}

struct LiveInstance {
  module: v8::Global<v8::Module>,
  isolate: v8::OwnedIsolate,
}

fn module_origin<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  name: v8::Local<'s, v8::Value>,
) -> v8::ScriptOrigin<'s> {
  v8::ScriptOrigin::new(
    scope, name, 0, 0, false, -1, None, false, false, true, None,
  )
}

#[allow(clippy::unnecessary_wraps)]
fn reject_dependency<'s>(
  context: v8::Local<'s, v8::Context>,
  specifier: v8::Local<'s, v8::String>,
  _import_attributes: v8::Local<'s, v8::FixedArray>,
  _referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
  v8::callback_scope!(unsafe scope, context);
  panic!(
    "the footprint module has no dependency {:?}",
    specifier.to_rust_string_lossy(scope)
  );
}

fn new_exercised_instance() -> LiveInstance {
  let mut isolate = v8::Isolate::new(Default::default());
  let module = {
    v8::scope!(let scope, &mut isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let source = v8::String::new(scope, ENGINE_SOURCE).unwrap();
    let name = v8::String::new(scope, MODULE_NAME).unwrap().into();
    let origin = module_origin(scope, name);
    let mut source = v8::script_compiler::Source::new(source, Some(&origin));
    let module =
      v8::script_compiler::compile_module(scope, &mut source).unwrap();
    assert!(module.instantiate_module(scope, reject_dependency).unwrap());
    assert!(module.evaluate(scope).is_some());
    assert_eq!(module.get_status(), v8::ModuleStatus::Evaluated);
    v8::Global::new(scope, module)
  };
  LiveInstance { module, isolate }
}

fn keep_alive(instance: &LiveInstance) {
  std::hint::black_box(&instance.module);
  std::hint::black_box(&instance.isolate);
}

fn print_memory(kind: &str, instances: usize, memory: ProcessMemory) {
  println!(
    "V8X_ENGINE_BENCH engine={} kind={kind} instances={instances} rss_kib={} vsz_kib={}",
    v8::V8X_ENGINE,
    memory.rss_kib,
    memory.vsz_kib,
  );
}

#[test]
#[ignore = "run through benchmarks/run-engine-comparison.sh"]
fn measure_engine_instances() {
  let instance_count = std::env::var("V8X_BENCH_INSTANCES")
    .unwrap_or_else(|_| "100".to_string())
    .parse::<usize>()
    .expect("V8X_BENCH_INSTANCES is numeric");
  let generation_only = std::env::var_os("V8X_BENCH_GENERATE_ONLY").is_some();
  assert!(
    (generation_only && instance_count == 1)
      || (!generation_only && instance_count > 10),
    "measure more than ten live instances, or generate one AOT artifact"
  );

  let baseline = process_memory();
  initialize();
  let initialized = process_memory();
  print_memory("baseline", 0, baseline);
  print_memory("initialized", 0, initialized);

  let steady_start = 10;
  let mut instances = Vec::with_capacity(instance_count);
  let mut creation_time = Duration::ZERO;
  let mut first = None;
  let mut steady = None;
  let mut steady_creation_time = Duration::ZERO;
  let mut final_memory = None;

  for index in 0..instance_count {
    let started = Instant::now();
    instances.push(new_exercised_instance());
    creation_time += started.elapsed();
    let count = index + 1;
    if count == 1
      || (!generation_only
        && (count == steady_start || count == instance_count))
    {
      let memory = process_memory();
      print_memory("live", count, memory);
      if count == 1 {
        first = Some(memory);
      }
      if count == steady_start {
        steady = Some(memory);
        steady_creation_time = creation_time;
      }
      if count == instance_count {
        final_memory = Some(memory);
      }
    }
  }

  if generation_only {
    println!(
      "V8X_ENGINE_BENCH engine={} kind=artifact_generation instances=1 creation_us={}",
      v8::V8X_ENGINE,
      creation_time.as_micros(),
    );
    keep_alive(&instances[0]);
    while instances.pop().is_some() {}
    return;
  }

  let first = first.unwrap();
  let steady = steady.unwrap();
  let final_memory = final_memory.unwrap();
  let steady_instances = (instance_count - steady_start) as i64;
  let steady_rss_bytes =
    (final_memory.rss_kib - steady.rss_kib) * 1024 / steady_instances;
  let steady_vsz_bytes =
    (final_memory.vsz_kib - steady.vsz_kib) * 1024 / steady_instances;
  let steady_creation = creation_time.saturating_sub(steady_creation_time);
  let steady_creation_us =
    steady_creation.as_micros() / steady_instances as u128;
  println!(
    "V8X_ENGINE_BENCH engine={} kind=summary instances={instance_count} first_rss_kib={} first_vsz_kib={} steady_from={steady_start} steady_rss_bytes_per_instance={steady_rss_bytes} steady_vsz_bytes_per_instance={steady_vsz_bytes} creation_us={} steady_creation_us_per_instance={steady_creation_us}",
    v8::V8X_ENGINE,
    first.rss_kib - initialized.rss_kib,
    first.vsz_kib - initialized.vsz_kib,
    creation_time.as_micros(),
  );

  for instance in &instances {
    keep_alive(instance);
  }
  // rusty_v8 enters OwnedIsolates when they are created and requires LIFO
  // disposal. Vec's normal drop order is forward, so pop explicitly.
  while instances.pop().is_some() {}
}
