// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

#[cfg(feature = "js2wasm_runtime_compile")]
use std::path::Path;
use std::sync::Once;
#[cfg(feature = "js2wasm_runtime_compile")]
use std::{fs, path::PathBuf};

const MAIN: &str = "file:///tmp/v8x-js2wasm-main.ts";
const DEPENDENCY: &str = "file:///tmp/v8x-js2wasm-math.ts";
const DENO: &str = "file:///tmp/v8x-js2wasm-deno.ts";
const DENO_SOURCE: &str = r#"
declare function __v8x_op_cwd_utf16_length(): number;
declare function __v8x_op_cwd_utf16_code_unit(index: number): number;

function cwd(): string {
  const length = __v8x_op_cwd_utf16_length();
  let value = "";
  for (let index = 0; index < length; index++) {
    value += String.fromCharCode(__v8x_op_cwd_utf16_code_unit(index));
  }
  return value;
}

export const Deno = { cwd };
"#;

unsafe extern "C" fn noop_callback(_info: *const v8::FunctionCallbackInfo) {}

fn initialize() {
  static ONCE: Once = Once::new();
  ONCE.call_once(|| {
    v8::V8::initialize_platform(
      v8::new_unprotected_default_platform(0, false).make_shared(),
    );
    v8::V8::initialize();
  });
}

fn origin<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  name: v8::Local<'s, v8::Value>,
) -> v8::ScriptOrigin<'s> {
  v8::ScriptOrigin::new(
    scope, name, 0, 0, false, -1, None, false, false, true, None,
  )
}

fn classic_origin<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  name: v8::Local<'s, v8::Value>,
) -> v8::ScriptOrigin<'s> {
  v8::ScriptOrigin::new(
    scope, name, 0, 0, false, -1, None, false, false, false, None,
  )
}

#[allow(clippy::unnecessary_wraps)]
fn resolve_dependency<'s>(
  context: v8::Local<'s, v8::Context>,
  specifier: v8::Local<'s, v8::String>,
  _import_attributes: v8::Local<'s, v8::FixedArray>,
  _referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
  v8::callback_scope!(unsafe scope, context);
  let specifier = specifier.to_rust_string_lossy(scope);
  let source = match specifier.as_str() {
    DEPENDENCY => {
      "export function add(left: number, right: number): number { return left + right; }"
    }
    DENO => DENO_SOURCE,
    _ => panic!("unexpected module dependency {specifier}"),
  };
  let source = v8::String::new(scope, source).unwrap();
  let name = v8::String::new(scope, &specifier).unwrap().into();
  let script_origin = origin(scope, name);
  let mut source =
    v8::script_compiler::Source::new(source, Some(&script_origin));
  v8::script_compiler::compile_module(scope, &mut source)
}

fn evaluate_graph_once() {
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  let global = context.global(scope);
  let property_key = v8::String::new(scope, "greeting").unwrap();
  let property_value = v8::String::new(scope, "Grüße").unwrap();
  assert_eq!(
    global.set(scope, property_key.into(), property_value.into()),
    Some(true)
  );
  let stored = global.get(scope, property_key.into()).unwrap();
  let stored = v8::Local::<v8::String>::try_from(stored).unwrap();
  assert_eq!(stored.to_rust_string_lossy(scope), "Grüße");

  let persistent = v8::Global::new(scope, global);
  let reopened = v8::Local::new(scope, &persistent);
  assert_eq!(
    reopened
      .get(scope, property_key.into())
      .unwrap()
      .is_string(),
    true
  );

  let object_template = v8::ObjectTemplate::new(scope);
  assert!(object_template.set_internal_field_count(1));
  let templated_object = object_template.new_instance(scope).unwrap();
  assert_eq!(templated_object.internal_field_count(), 1);
  let marker = 42_u64;
  let marker_ptr = (&marker as *const u64).cast();
  templated_object.set_aligned_pointer_in_internal_field(0, marker_ptr, 7);
  assert_eq!(
    unsafe { templated_object.get_aligned_pointer_from_internal_field(0, 7) },
    marker_ptr
  );

  let message = v8::String::new(scope, "operation failed").unwrap();
  assert!(v8::Exception::error(scope, message).is_native_error());

  let source = format!(
    "import {{ add }} from {DEPENDENCY:?};\n\
     import {{ Deno }} from {DENO:?};\n\
     const answer: number = add(20, 22);\n\
     if (answer !== 42) throw new Error('wrong result');\n\
     export function __v8x_probe_cwd_utf16_length(): number {{\n\
       return Deno.cwd().length;\n\
     }}\n\
     export function __v8x_probe_cwd_utf16_checksum(): number {{\n\
       const value = Deno.cwd();\n\
       let checksum = 0;\n\
       for (let index = 0; index < value.length; index++) {{\n\
         checksum += (index + 1) * value.charCodeAt(index);\n\
       }}\n\
       return checksum;\n\
     }}"
  );
  let source = v8::String::new(scope, &source).unwrap();
  let name = v8::String::new(scope, MAIN).unwrap().into();
  let script_origin = origin(scope, name);
  let mut source =
    v8::script_compiler::Source::new(source, Some(&script_origin));

  let module = v8::script_compiler::compile_module(scope, &mut source).unwrap();
  assert_eq!(module.get_status(), v8::ModuleStatus::Uninstantiated);
  assert!(
    module
      .instantiate_module(scope, resolve_dependency)
      .unwrap()
  );
  assert_eq!(module.get_status(), v8::ModuleStatus::Instantiated);

  assert!(module.evaluate(scope).is_some());
  assert_eq!(module.get_status(), v8::ModuleStatus::Evaluated);
}

#[test]
fn evaluates_raw_typescript_graph_through_wasmtime() {
  initialize();
  assert_eq!(v8::V8X_ENGINE, "js2wasm");
  assert!(
    std::env::var_os("V8X_JS2WASM_COMPILER_SCRIPT").is_some()
      || std::env::var_os("V8X_JS2WASM_AOT_MODULE").is_some(),
    "set V8X_JS2WASM_COMPILER_SCRIPT to compile-graph.ts or V8X_JS2WASM_AOT_MODULE to a trusted Wasmtime-precompiled artifact"
  );

  let before = v8::js2wasm_runtime_stats().unwrap();
  evaluate_graph_once();

  if std::env::var_os("V8X_JS2WASM_AOT_MODULE").is_some() {
    evaluate_graph_once();
    let after = v8::js2wasm_runtime_stats().unwrap();
    assert_eq!(after.module_loads - before.module_loads, 1);
    assert_eq!(after.cached_modules - before.cached_modules, 1);
    assert_eq!(after.instantiations - before.instantiations, 2);
  }
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
#[ignore = "requires V8X_JS2WASM_DENO_CORE_WASM from js2wasm's pinned bootstrap probe"]
fn boots_exact_deno_core_artifact_in_two_wasmtime_stores() {
  let artifact = std::env::var_os("V8X_JS2WASM_DENO_CORE_WASM").expect(
    "set V8X_JS2WASM_DENO_CORE_WASM to the raw pinned bootstrap module",
  );
  v8::js2wasm_bootstrap_raw_module_for_test(Path::new(&artifact))
    .expect("boot exact Deno core artifact through embedded Wasmtime");
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
#[ignore = "requires V8X_JS2WASM_DENO_CORE_WASM and V8X_JS2WASM_DENO_CORE_FIXTURES"]
fn routes_exact_deno_core_scripts_through_public_script_run() {
  initialize();
  let fixture_dir =
    PathBuf::from(std::env::var_os("V8X_JS2WASM_DENO_CORE_FIXTURES").expect(
      "set V8X_JS2WASM_DENO_CORE_FIXTURES to the pinned deno_core sources",
    ));
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  // Reproduce the Rust-owned graph deno_core installs before its first
  // classic script. Script::Run must leave an observable bootstrap function
  // on this same object graph, not only succeed inside an isolated store.
  let global = context.global(scope);
  let deno = v8::Object::new(scope);
  let core = v8::Object::new(scope);
  let ops = v8::Object::new(scope);
  let deno_key = v8::String::new(scope, "Deno").unwrap();
  let core_key = v8::String::new(scope, "core").unwrap();
  let ops_key = v8::String::new(scope, "ops").unwrap();
  assert_eq!(core.set(scope, ops_key.into(), ops.into()), Some(true));
  assert_eq!(deno.set(scope, core_key.into(), core.into()), Some(true));
  assert_eq!(global.set(scope, deno_key.into(), deno.into()), Some(true));

  for name in ["00_primordials.js", "00_infra.js", "01_core.js"] {
    let source = fs::read_to_string(fixture_dir.join(name)).unwrap();
    let source = v8::String::new(scope, &source).unwrap();
    let specifier = format!("ext:core/{name}");
    let resource = v8::String::new(scope, &specifier).unwrap().into();
    let script_origin = classic_origin(scope, resource);
    let script = v8::Script::compile(scope, source, Some(&script_origin))
      .unwrap_or_else(|| panic!("compile {specifier}"));
    assert!(script.run(scope).is_some(), "run {specifier}");
  }

  let stub_key = v8::String::new(scope, "setUpAsyncStub").unwrap();
  let stub = core.get(scope, stub_key.into()).unwrap();
  let stub = v8::Local::<v8::Function>::try_from(stub).unwrap();
  let op = v8::Function::new_raw(scope, noop_callback).unwrap();
  let op_value: v8::Local<v8::Value> = op.into();
  let name = v8::String::new(scope, "op_async_probe").unwrap();
  let undefined = v8::undefined(scope);
  let returned = stub
    .call(scope, undefined.into(), &[name.into(), op_value])
    .unwrap();
  assert!(std::ptr::eq(&*returned, &*op_value));
}
