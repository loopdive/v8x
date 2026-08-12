// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

// Process footprint, startup, and warm execution benchmark shared by the v8x
// backends and the real rusty_v8 baseline crate.

use std::sync::Once;
use std::time::{Duration, Instant};

const MODULE_NAME: &str = "file:///v8x-engine-benchmark.ts";

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
  context: v8::Global<v8::Context>,
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
    "the benchmark module has no dependency {:?}",
    specifier.to_rust_string_lossy(scope)
  );
}

fn new_exercised_instance() -> LiveInstance {
  let mut isolate = v8::Isolate::new(Default::default());
  let (context, module) = {
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
    (
      v8::Global::new(scope, context),
      v8::Global::new(scope, module),
    )
  };
  LiveInstance {
    context,
    module,
    isolate,
  }
}

fn keep_alive(instance: &LiveInstance) {
  std::hint::black_box(&instance.context);
  std::hint::black_box(&instance.module);
  std::hint::black_box(&instance.isolate);
}

fn print_memory(kind: &str, instances: usize, memory: ProcessMemory) {
  println!(
    "V8X_ENGINE_BENCH engine={ENGINE_NAME} kind={kind} instances={instances} rss_kib={} vsz_kib={}",
    memory.rss_kib, memory.vsz_kib,
  );
}

fn env_usize(name: &str, default: usize) -> usize {
  std::env::var(name)
    .unwrap_or_else(|_| default.to_string())
    .parse::<usize>()
    .unwrap_or_else(|_| panic!("{name} is numeric"))
}

#[cfg(not(feature = "engine_js2wasm"))]
fn call_export_batch(
  instance: &mut LiveInstance,
  export: &str,
  argument: f64,
  calls: usize,
) -> f64 {
  let context = instance.context.clone();
  let module = instance.module.clone();
  v8::scope!(let scope, &mut instance.isolate);
  let context = v8::Local::new(scope, &context);
  let scope = &mut v8::ContextScope::new(scope, context);
  let module = v8::Local::new(scope, &module);
  let namespace = module.get_module_namespace().to_object(scope).unwrap();
  let export_name = v8::String::new(scope, export).unwrap();
  let function: v8::Local<v8::Function> = namespace
    .get(scope, export_name.into())
    .unwrap()
    .try_into()
    .unwrap();
  let receiver = v8::undefined(scope).into();
  let argument = v8::Number::new(scope, argument).into();
  let mut result = 0.0;
  for _ in 0..calls {
    let value = function.call(scope, receiver, &[argument]).unwrap();
    result = value.number_value(scope).unwrap();
    std::hint::black_box(result);
  }
  result
}

#[cfg(feature = "engine_js2wasm")]
fn call_export_batch(
  instance: &mut LiveInstance,
  export: &str,
  argument: f64,
  calls: usize,
) -> f64 {
  let module = instance.module.clone();
  v8::scope!(let scope, &mut instance.isolate);
  let module = v8::Local::new(scope, &module);
  v8::js2wasm_run_f64_export_batch_for_benchmark(
    &module, export, argument, calls,
  )
  .unwrap()
}

fn expected_kernel(iterations: usize) -> f64 {
  let mut state = 1_u64;
  for index in 0..iterations as u64 {
    state = (state * 17 + index) % 1_000_003;
  }
  state as f64
}

#[test]
#[ignore = "run through benchmarks/run-engine-comparison.sh"]
fn measure_engine_instances() {
  let instance_count = env_usize("V8X_BENCH_INSTANCES", 100);
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
      "V8X_ENGINE_BENCH engine={ENGINE_NAME} kind=artifact_generation instances=1 creation_us={}",
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
    "V8X_ENGINE_BENCH engine={ENGINE_NAME} kind=summary instances={instance_count} initialization_rss_kib={} initialization_vsz_kib={} first_rss_kib={} first_vsz_kib={} one_instance_total_rss_kib={} one_instance_total_vsz_kib={} steady_from={steady_start} steady_rss_bytes_per_instance={steady_rss_bytes} steady_vsz_bytes_per_instance={steady_vsz_bytes} creation_us={} steady_creation_us_per_instance={steady_creation_us}",
    initialized.rss_kib - baseline.rss_kib,
    initialized.vsz_kib - baseline.vsz_kib,
    first.rss_kib - initialized.rss_kib,
    first.vsz_kib - initialized.vsz_kib,
    first.rss_kib - baseline.rss_kib,
    first.vsz_kib - baseline.vsz_kib,
    creation_time.as_micros(),
  );

  for instance in &instances {
    keep_alive(instance);
  }
  // rusty_v8 enters OwnedIsolates when they are created and requires LIFO
  // disposal. Vec's normal drop order is forward, so pop explicitly.
  while instances.pop().is_some() {}
}

#[test]
#[ignore = "run through benchmarks/run-engine-comparison.sh"]
fn measure_engine_speed() {
  let noop_calls = env_usize("V8X_BENCH_SPEED_NOOP_CALLS", 200_000);
  let kernel_calls = env_usize("V8X_BENCH_SPEED_KERNEL_CALLS", 20);
  let kernel_iterations =
    env_usize("V8X_BENCH_SPEED_KERNEL_ITERATIONS", 500_000);
  assert!(noop_calls > 0 && kernel_calls > 0 && kernel_iterations > 0);

  initialize();
  let mut instance = new_exercised_instance();

  // Warm the API boundary and give V8 enough loop executions to tier up before
  // either timed region. QuickJS and Wasmtime execute the same warmup calls.
  assert_eq!(
    call_export_batch(&mut instance, "benchmarkNoop", 41.0, 10_000),
    42.0
  );
  let warm_iterations = kernel_iterations.min(10_000);
  assert_eq!(
    call_export_batch(
      &mut instance,
      "benchmarkKernel",
      warm_iterations as f64,
      100,
    ),
    expected_kernel(warm_iterations),
  );

  let started = Instant::now();
  let noop_result =
    call_export_batch(&mut instance, "benchmarkNoop", 41.0, noop_calls);
  let noop_elapsed = started.elapsed();
  assert_eq!(noop_result, 42.0);
  println!(
    "V8X_ENGINE_SPEED engine={ENGINE_NAME} workload=noop calls={noop_calls} result={noop_result:.0} elapsed_ns={} ns_per_call={:.3}",
    noop_elapsed.as_nanos(),
    noop_elapsed.as_nanos() as f64 / noop_calls as f64,
  );

  let expected = expected_kernel(kernel_iterations);
  let started = Instant::now();
  let kernel_result = call_export_batch(
    &mut instance,
    "benchmarkKernel",
    kernel_iterations as f64,
    kernel_calls,
  );
  let kernel_elapsed = started.elapsed();
  assert_eq!(kernel_result, expected);
  let total_iterations = kernel_calls * kernel_iterations;
  println!(
    "V8X_ENGINE_SPEED engine={ENGINE_NAME} workload=kernel calls={kernel_calls} iterations_per_call={kernel_iterations} total_iterations={total_iterations} result={kernel_result:.0} elapsed_ns={} ns_per_iteration={:.3}",
    kernel_elapsed.as_nanos(),
    kernel_elapsed.as_nanos() as f64 / total_iterations as f64,
  );

  keep_alive(&instance);
}
