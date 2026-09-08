// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
#[cfg(feature = "js2wasm_runtime_compile")]
use std::path::Path;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAIN: &str = "file:///tmp/v8x-js2wasm-main.ts";
const RUNTIME_EVAL_MAIN: &str = "file:///tmp/v8x-js2wasm-runtime-eval-main.ts";
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

#[cfg(feature = "js2wasm_runtime_compile")]
#[derive(Debug, PartialEq)]
enum DenoOpEvent {
  Print {
    message: String,
    is_error: bool,
    data: usize,
  },
  SumArray {
    values: Vec<f64>,
    data: usize,
  },
  SumNumber {
    value: f64,
    data: usize,
  },
}

unsafe extern "C" fn noop_callback(_info: *const v8::FunctionCallbackInfo) {}

unsafe extern "C" fn count_backing_store_deletion(
  _data: *mut std::ffi::c_void,
  _byte_length: usize,
  deleter_data: *mut std::ffi::c_void,
) {
  if let Some(count) = unsafe { deleter_data.cast::<AtomicUsize>().as_ref() } {
    count.fetch_add(1, Ordering::SeqCst);
  }
}

thread_local! {
  static SYNTHETIC_CALLBACK_COUNT: Cell<usize> = const { Cell::new(0) };
  static SYNTHETIC_OP_VALUE: RefCell<Option<v8::Global<v8::Value>>> =
    const { RefCell::new(None) };
  static SYNTHETIC_LABEL_VALUE: RefCell<Option<v8::Global<v8::Value>>> =
    const { RefCell::new(None) };
  static MICROTASK_EVENTS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
  static PROMISE_EVENTS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

#[cfg(feature = "js2wasm_runtime_compile")]
thread_local! {
  static DENO_OP_EVENTS: RefCell<Vec<DenoOpEvent>> = const { RefCell::new(Vec::new()) };
}

#[cfg(feature = "js2wasm_runtime_compile")]
unsafe extern "C" fn deno_op_print_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  assert_eq!(args.length(), 2);
  let message = args.get(0);
  assert!(message.is_string());
  let message = v8::Local::<v8::String>::try_from(message).unwrap();
  let is_error = args.get(1);
  assert!(is_error.is_boolean());
  let data = unsafe {
    v8::Local::<v8::External>::cast_unchecked(args.data()).value() as usize
  };
  DENO_OP_EVENTS.with(|events| {
    events.borrow_mut().push(DenoOpEvent::Print {
      message: message.to_rust_string_lossy(scope),
      is_error: is_error.is_true(),
      data,
    });
  });
}

#[cfg(feature = "js2wasm_runtime_compile")]
unsafe extern "C" fn deno_op_sum_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  assert_eq!(args.length(), 1);
  let data = unsafe {
    v8::Local::<v8::External>::cast_unchecked(args.data()).value() as usize
  };
  let argument = args.get(0);
  if let Ok(array) = v8::Local::<v8::Array>::try_from(argument) {
    let values = (0..array.length())
      .map(|index| {
        array
          .get_index(scope, index)
          .unwrap()
          .number_value(scope)
          .unwrap()
      })
      .collect::<Vec<_>>();
    let sum = values.iter().sum();
    DENO_OP_EVENTS.with(|events| {
      events
        .borrow_mut()
        .push(DenoOpEvent::SumArray { values, data });
    });
    let mut return_value = parts.return_value;
    return_value.set_double(sum);
    return;
  }

  assert!(argument.is_number());
  let value = argument.number_value(scope).unwrap();
  DENO_OP_EVENTS.with(|events| {
    events
      .borrow_mut()
      .push(DenoOpEvent::SumNumber { value, data });
  });
  let message = v8::String::new(
    scope,
    "serde_v8 error: invalid type; expected: array, got: Number",
  )
  .unwrap();
  let exception = v8::Exception::type_error(scope, message);
  scope.throw_exception(exception);
}

unsafe extern "C" fn first_microtask(_info: *const v8::FunctionCallbackInfo) {
  MICROTASK_EVENTS.with(|events| events.borrow_mut().push(1));
}

unsafe extern "C" fn second_microtask(_info: *const v8::FunctionCallbackInfo) {
  MICROTASK_EVENTS.with(|events| events.borrow_mut().push(2));
}

unsafe extern "C" fn increment_promise_value(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  let value = args.get(0).number_value(scope).unwrap();
  PROMISE_EVENTS.with(|events| events.borrow_mut().push(value));
  let mut return_value = parts.return_value;
  return_value.set_double(value + 1.0);
}

#[allow(clippy::unnecessary_wraps)]
fn synthetic_evaluation_steps<'s>(
  context: v8::Local<'s, v8::Context>,
  module: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Value>> {
  v8::callback_scope!(unsafe scope, context);
  assert_eq!(module.get_status(), v8::ModuleStatus::Evaluating);
  SYNTHETIC_CALLBACK_COUNT.with(|count| count.set(count.get() + 1));

  let op_name = v8::String::new(scope, "op_test").unwrap();
  SYNTHETIC_OP_VALUE.with(|value| {
    let value = value.borrow();
    let op_value = v8::Local::new(scope, value.as_ref().unwrap());
    assert_eq!(
      module.set_synthetic_module_export(scope, op_name, op_value),
      Some(true)
    );
  });

  let label_name = v8::String::new(scope, "label").unwrap();
  SYNTHETIC_LABEL_VALUE.with(|value| {
    let value = value.borrow();
    let label_value = v8::Local::new(scope, value.as_ref().unwrap());
    assert_eq!(
      module.set_synthetic_module_export(scope, label_name, label_value),
      Some(true)
    );
  });

  let resolver = v8::PromiseResolver::new(scope).unwrap();
  let undefined = v8::undefined(scope);
  assert_eq!(resolver.resolve(scope, undefined.into()), Some(true));
  Some(resolver.get_promise(scope).into())
}

struct DropMarker(Rc<Cell<usize>>);

impl Drop for DropMarker {
  fn drop(&mut self) {
    self.0.set(self.0.get() + 1);
  }
}

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

#[cfg(feature = "js2wasm_runtime_compile")]
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
     if ((globalThis as any).greeting !== 'Grüße') throw new Error('context global not shared with Wasm');\n\
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

fn evaluate_runtime_eval_graph_once() {
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let source = r#"
    (globalThis as any).runtimeCounter = 40;
    let increment = 2;
    let mutation = "runtimeCounter = runtimeCounter + " + increment;
    (0, eval)(mutation);
    let readback = "runtime" + "Counter";
    export function __v8x_probe_runtime_eval_state(): number {
      return (globalThis as any).runtimeCounter + (0, eval)(readback);
    }
  "#;
  let source = v8::String::new(scope, source).unwrap();
  let name = v8::String::new(scope, RUNTIME_EVAL_MAIN).unwrap().into();
  let script_origin = origin(scope, name);
  let mut source =
    v8::script_compiler::Source::new(source, Some(&script_origin));
  let module = v8::script_compiler::compile_module(scope, &mut source).unwrap();
  assert!(
    module
      .instantiate_module(scope, resolve_dependency)
      .unwrap()
  );
  assert!(module.evaluate(scope).is_some());
  assert_eq!(module.get_status(), v8::ModuleStatus::Evaluated);
}

#[test]
fn evaluates_synthetic_module_once_with_stable_namespace_and_promise() {
  initialize();
  SYNTHETIC_CALLBACK_COUNT.with(|count| count.set(0));
  SYNTHETIC_OP_VALUE.with(|value| value.borrow_mut().take());
  SYNTHETIC_LABEL_VALUE.with(|value| value.borrow_mut().take());

  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  let op = v8::FunctionTemplate::new_raw(scope, noop_callback)
    .get_function(scope)
    .unwrap();
  let op_value: v8::Local<v8::Value> = op.into();
  let label = v8::String::new(scope, "real export value").unwrap();
  let label_value: v8::Local<v8::Value> = label.into();
  SYNTHETIC_OP_VALUE.with(|value| {
    *value.borrow_mut() = Some(v8::Global::new(scope, op_value));
  });
  SYNTHETIC_LABEL_VALUE.with(|value| {
    *value.borrow_mut() = Some(v8::Global::new(scope, label_value));
  });

  let op_name = v8::String::new(scope, "op_test").unwrap();
  let label_name = v8::String::new(scope, "label").unwrap();
  let module_name = v8::String::new(scope, "ext:core/ops").unwrap();
  let module = v8::Module::create_synthetic_module(
    scope,
    module_name,
    &[op_name, label_name],
    synthetic_evaluation_steps,
  );
  assert_eq!(module.get_status(), v8::ModuleStatus::Uninstantiated);
  assert!(module.is_synthetic_module());
  assert!(!module.is_source_text_module());
  assert!(
    module
      .instantiate_module(scope, resolve_dependency)
      .unwrap()
  );
  assert_eq!(module.get_status(), v8::ModuleStatus::Instantiated);

  let namespace_value_before = module.get_module_namespace();
  let namespace =
    v8::Local::<v8::Object>::try_from(namespace_value_before).unwrap();
  assert!(namespace.get(scope, op_name.into()).unwrap().is_undefined());

  let evaluation_value = module.evaluate(scope).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(evaluation_value).unwrap();
  assert_eq!(module.get_status(), v8::ModuleStatus::Evaluated);
  assert_eq!(promise.state(), v8::PromiseState::Fulfilled);
  assert!(promise.result(scope).is_undefined());
  assert!(!promise.has_handler());
  promise.mark_as_handled();
  assert!(promise.has_handler());
  SYNTHETIC_CALLBACK_COUNT.with(|count| assert_eq!(count.get(), 1));

  let namespace_value_after = module.get_module_namespace();
  assert!(std::ptr::eq(
    &*namespace_value_before,
    &*namespace_value_after
  ));
  let exported_op = namespace.get(scope, op_name.into()).unwrap();
  let exported_label = namespace.get(scope, label_name.into()).unwrap();
  assert!(std::ptr::eq(&*exported_op, &*op_value));
  assert!(std::ptr::eq(&*exported_label, &*label_value));

  let repeated_value = module.evaluate(scope).unwrap();
  assert!(std::ptr::eq(&*evaluation_value, &*repeated_value));
  SYNTHETIC_CALLBACK_COUNT.with(|count| assert_eq!(count.get(), 1));

  let undeclared = v8::String::new(scope, "not_declared").unwrap();
  let undefined = v8::undefined(scope);
  assert!(
    module
      .set_synthetic_module_export(scope, undeclared, undefined.into())
      .is_none()
  );

  SYNTHETIC_OP_VALUE.with(|value| value.borrow_mut().take());
  SYNTHETIC_LABEL_VALUE.with(|value| value.borrow_mut().take());
}

#[test]
fn global_module_hash_map_preserves_key_and_value_identity() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  let module_name = v8::String::new(scope, "ext:core/hash-probe").unwrap();
  let module = v8::Module::create_synthetic_module(
    scope,
    module_name,
    &[],
    synthetic_evaluation_steps,
  );
  let identity_hash = module.get_identity_hash();
  assert_eq!(module.get_identity_hash(), identity_hash);

  let key = v8::Global::new(scope, module);
  let lookup_key = key.clone();
  assert_eq!(key, lookup_key);

  let value = v8::String::new(scope, "stable value").unwrap();
  let value: v8::Local<v8::Value> = value.into();
  let stored_value = v8::Global::new(scope, value);
  let mut map = HashMap::new();
  assert!(map.insert(key, stored_value).is_none());

  let found = map.get(&lookup_key).unwrap();
  let found = v8::Local::new(scope, found);
  assert!(std::ptr::eq(&*found, &*value));

  let removed = map.remove(&lookup_key).unwrap();
  let removed = v8::Local::new(scope, &removed);
  assert!(std::ptr::eq(&*removed, &*value));
  assert!(map.is_empty());
}

#[test]
fn source_module_metadata_is_stable_and_empty_without_imports() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  let source = v8::String::new(
    scope,
    "const bootstrap = globalThis.__bootstrap;\n\
     const { core, internals, primordials } = bootstrap;\n\
     export { core, internals, primordials };",
  )
  .unwrap();
  let resource_name = v8::String::new(scope, "ext:core/mod.js").unwrap();
  let script_origin = origin(scope, resource_name.into());
  let mut source =
    v8::script_compiler::Source::new(source, Some(&script_origin));
  let module = v8::script_compiler::compile_module(scope, &mut source).unwrap();

  let unbound = module.get_unbound_module_script(scope);
  let unbound_again = module.get_unbound_module_script(scope);
  assert!(std::ptr::eq(&*unbound, &*unbound_again));

  let source_mapping_url = unbound.get_source_mapping_url(scope);
  let source_mapping_url_again = unbound.get_source_mapping_url(scope);
  assert!(source_mapping_url.is_undefined());
  assert!(std::ptr::eq(
    &*source_mapping_url,
    &*source_mapping_url_again
  ));

  let requests = module.get_module_requests();
  let requests_again = module.get_module_requests();
  assert!(std::ptr::eq(&*requests, &*requests_again));
  assert_eq!(requests.length(), 0);
  assert!(requests.get(scope, 0).is_none());
}

#[test]
fn exact_deno_core_source_module_has_stable_branded_namespace_value() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  // Pinned deno_core 0.407.0 `libs/core/mod.js`: this exact source has no
  // imports and is prelinked by the js2wasm backend. Namespace construction
  // must not pretend to execute it or manufacture its three live exports.
  let source = v8::String::new(
    scope,
    "// Copyright 2018-2026 the Deno authors. MIT license.\n\
// Re-export fields from `globalThis.__bootstrap` so that embedders using\n\
// ES modules can import these symbols instead of capturing the bootstrap ns.\n\
const bootstrap = globalThis.__bootstrap;\n\
const { core, internals, primordials } = bootstrap;\n\
\n\
export { core, internals, primordials };\n",
  )
  .unwrap();
  let resource_name = v8::String::new(scope, "ext:core/mod.js").unwrap();
  let script_origin = origin(scope, resource_name.into());
  let mut source =
    v8::script_compiler::Source::new(source, Some(&script_origin));
  let module = v8::script_compiler::compile_module(scope, &mut source).unwrap();
  assert_eq!(module.get_module_requests().length(), 0);
  assert!(
    module
      .instantiate_module(scope, resolve_dependency)
      .unwrap()
  );
  assert_eq!(module.get_status(), v8::ModuleStatus::Instantiated);

  let namespace = module.get_module_namespace();
  assert!(namespace.is_object());
  assert!(namespace.is_module_namespace_object());
  assert!(v8::Local::<v8::Object>::try_from(namespace).is_ok());

  let namespace_again = module.get_module_namespace();
  assert!(std::ptr::eq(&*namespace, &*namespace_again));

  let ordinary_object: v8::Local<v8::Value> = v8::Object::new(scope).into();
  assert!(!ordinary_object.is_module_namespace_object());
}

#[test]
fn creates_objects_with_live_explicit_prototype_chains() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  let prototype = v8::Object::new(scope);
  let inherited_key = v8::String::new(scope, "inherited").unwrap();
  let inherited_value = v8::String::new(scope, "prototype value").unwrap();
  assert_eq!(
    prototype.set(scope, inherited_key.into(), inherited_value.into()),
    Some(true)
  );

  let own_key = v8::String::new(scope, "own").unwrap();
  let own_name: v8::Local<v8::Name> = own_key.into();
  let own_value = v8::String::new(scope, "own value").unwrap();
  let own_value_as_value: v8::Local<v8::Value> = own_value.into();
  let prototype_as_value: v8::Local<v8::Value> = prototype.into();
  let child = v8::Object::with_prototype_and_properties(
    scope,
    prototype_as_value,
    &[own_name],
    &[own_value_as_value],
  );

  let observed_prototype = child.get_prototype(scope).unwrap();
  assert!(std::ptr::eq(&*observed_prototype, &*prototype_as_value));
  let observed_own = child.get(scope, own_key.into()).unwrap();
  assert!(std::ptr::eq(&*observed_own, &*own_value_as_value));
  let observed_inherited = child.get(scope, inherited_key.into()).unwrap();
  let inherited_value_as_value: v8::Local<v8::Value> = inherited_value.into();
  assert!(std::ptr::eq(
    &*observed_inherited,
    &*inherited_value_as_value
  ));
  assert_eq!(child.has(scope, inherited_key.into()), Some(true));
  assert_eq!(
    child.has_own_property(scope, inherited_key.into()),
    Some(false)
  );

  let late_key = v8::String::new(scope, "late").unwrap();
  let late_value = v8::String::new(scope, "late prototype value").unwrap();
  assert_eq!(
    prototype.set(scope, late_key.into(), late_value.into()),
    Some(true)
  );
  let observed_late = child.get(scope, late_key.into()).unwrap();
  let late_value_as_value: v8::Local<v8::Value> = late_value.into();
  assert!(std::ptr::eq(&*observed_late, &*late_value_as_value));

  let shadow = v8::String::new(scope, "child shadow").unwrap();
  assert_eq!(
    child.set(scope, inherited_key.into(), shadow.into()),
    Some(true)
  );
  let observed_shadow = child.get(scope, inherited_key.into()).unwrap();
  let shadow_as_value: v8::Local<v8::Value> = shadow.into();
  assert!(std::ptr::eq(&*observed_shadow, &*shadow_as_value));
  let prototype_value = prototype.get(scope, inherited_key.into()).unwrap();
  assert!(std::ptr::eq(&*prototype_value, &*inherited_value_as_value));

  assert_eq!(prototype.set_prototype(scope, child.into()), Some(false));

  let null = v8::null(scope);
  let null_as_value: v8::Local<v8::Value> = null.into();
  let null_root =
    v8::Object::with_prototype_and_properties(scope, null_as_value, &[], &[]);
  let observed_null = null_root.get_prototype(scope).unwrap();
  assert!(std::ptr::eq(&*observed_null, &*null_as_value));
  assert!(
    null_root
      .get(scope, inherited_key.into())
      .unwrap()
      .is_undefined()
  );
}

#[test]
fn gives_native_errors_a_canonical_error_prototype() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  let first_message = v8::String::new(scope, "first failure").unwrap();
  let first_error = v8::Exception::error(scope, first_message);
  let first_error = v8::Local::<v8::Object>::try_from(first_error).unwrap();
  let first_prototype = first_error.get_prototype(scope).unwrap();
  assert!(first_prototype.is_object());
  assert!(!first_prototype.is_null());

  let second_message = v8::String::new(scope, "second failure").unwrap();
  let second_error = v8::Exception::type_error(scope, second_message);
  let second_error = v8::Local::<v8::Object>::try_from(second_error).unwrap();
  let second_prototype = second_error.get_prototype(scope).unwrap();
  assert!(std::ptr::eq(&*first_prototype, &*second_prototype));
}

#[test]
fn aliases_external_backing_stores_through_typed_arrays_and_deletes() {
  initialize();
  let deletion_count = AtomicUsize::new(0);
  let mut bytes = vec![0_u8; 12].into_boxed_slice();
  bytes[0] = 7;
  bytes[1] = 11;
  bytes[4..8].copy_from_slice(&13_u32.to_ne_bytes());
  let backing_store = unsafe {
    v8::ArrayBuffer::new_backing_store_from_ptr(
      bytes.as_mut_ptr().cast(),
      bytes.len(),
      count_backing_store_deletion,
      (&deletion_count as *const AtomicUsize).cast_mut().cast(),
    )
  };
  let backing_store = backing_store.make_shared();
  assert_eq!(backing_store.byte_length(), 12);
  assert_eq!(
    backing_store.data().unwrap().as_ptr(),
    bytes.as_mut_ptr().cast()
  );

  let mut isolate = v8::Isolate::new(Default::default());
  {
    v8::scope!(let scope, &mut isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);

    let array_buffer =
      v8::ArrayBuffer::with_backing_store(scope, &backing_store);
    assert_eq!(array_buffer.byte_length(), 12);
    assert_eq!(
      array_buffer.data().unwrap().as_ptr(),
      bytes.as_mut_ptr().cast()
    );
    let copied_store = array_buffer.get_backing_store();
    assert_eq!(
      copied_store.data().unwrap().as_ptr(),
      bytes.as_mut_ptr().cast()
    );
    drop(copied_store);

    let u8_view = v8::Uint8Array::new(scope, array_buffer, 0, 2).unwrap();
    let u32_view = v8::Uint32Array::new(scope, array_buffer, 0, 3).unwrap();
    let i32_view = v8::Int32Array::new(scope, array_buffer, 0, 1).unwrap();
    assert!(u8_view.is_uint8_array());
    assert!(u32_view.is_uint32_array());
    assert!(i32_view.is_int32_array());
    assert_eq!(u8_view.length(), 2);
    assert_eq!(u32_view.length(), 3);
    assert_eq!(i32_view.length(), 1);
    assert_eq!(u8_view.byte_length(), 2);
    assert_eq!(u32_view.byte_length(), 12);
    assert_eq!(i32_view.byte_length(), 4);
    let first = u8_view.get_index(scope, 0).unwrap();
    let first = v8::Local::<v8::Number>::try_from(first).unwrap();
    assert_eq!(first.value(), 7.0);
    let second = u8_view.get_index(scope, 1).unwrap();
    let second = v8::Local::<v8::Number>::try_from(second).unwrap();
    assert_eq!(second.value(), 11.0);
    let middle = u32_view.get_index(scope, 1).unwrap();
    let middle = v8::Local::<v8::Number>::try_from(middle).unwrap();
    assert_eq!(middle.value(), 13.0);

    let unsigned = v8::Number::new(scope, 2_000_000_000.0);
    assert_eq!(u32_view.set_index(scope, 1, unsigned.into()), Some(true));
    assert_eq!(
      u32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
      2_000_000_000
    );

    let signed = v8::Number::new(scope, -2_000_000_000.0);
    assert_eq!(i32_view.set_index(scope, 0, signed.into()), Some(true));
    assert_eq!(
      i32::from_ne_bytes(bytes[0..4].try_into().unwrap()),
      -2_000_000_000
    );

    let property = v8::String::new(scope, "initOnly").unwrap();
    let sentinel = v8::String::new(scope, "present").unwrap();
    assert_eq!(
      u8_view.set(scope, property.into(), sentinel.into()),
      Some(true)
    );
    assert_eq!(u8_view.delete(scope, property.into()), Some(true));
    assert!(u8_view.get(scope, property.into()).unwrap().is_undefined());
  }

  drop(isolate);
  assert_eq!(deletion_count.load(Ordering::SeqCst), 0);
  drop(backing_store);
  assert_eq!(deletion_count.load(Ordering::SeqCst), 1);
}

#[test]
fn drains_enqueued_microtasks_in_fifo_order_once() {
  initialize();
  MICROTASK_EVENTS.with(|events| events.borrow_mut().clear());

  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

  let first = v8::FunctionTemplate::new_raw(scope, first_microtask)
    .get_function(scope)
    .unwrap();
  let second = v8::FunctionTemplate::new_raw(scope, second_microtask)
    .get_function(scope)
    .unwrap();
  scope.enqueue_microtask(first);
  scope.enqueue_microtask(second);
  MICROTASK_EVENTS.with(|events| assert!(events.borrow().is_empty()));

  scope.perform_microtask_checkpoint();
  MICROTASK_EVENTS.with(|events| assert_eq!(&*events.borrow(), &[1, 2]));

  scope.perform_microtask_checkpoint();
  MICROTASK_EVENTS.with(|events| assert_eq!(&*events.borrow(), &[1, 2]));
}

#[test]
fn chains_settled_promises_through_the_microtask_queue() {
  initialize();
  PROMISE_EVENTS.with(|events| events.borrow_mut().clear());

  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);

  let resolver = v8::PromiseResolver::new(scope).unwrap();
  let promise = resolver.get_promise(scope);
  let handler = v8::FunctionTemplate::new_raw(scope, increment_promise_value)
    .get_function(scope)
    .unwrap();
  let derived = promise.then2(scope, handler, handler).unwrap();
  assert!(promise.has_handler());

  let value = v8::Number::new(scope, 41.0);
  assert_eq!(resolver.resolve(scope, value.into()), Some(true));
  assert_eq!(derived.state(), v8::PromiseState::Pending);
  PROMISE_EVENTS.with(|events| assert!(events.borrow().is_empty()));

  scope.perform_microtask_checkpoint();
  PROMISE_EVENTS.with(|events| assert_eq!(&*events.borrow(), &[41.0]));
  assert_eq!(derived.state(), v8::PromiseState::Fulfilled);
  assert_eq!(derived.result(scope).number_value(scope), Some(42.0));
}

#[test]
fn keeps_symbol_identity_and_escaped_values_stable() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);

  let description = v8::String::new(scope, "deno-core-symbol").unwrap();
  let first = v8::Symbol::for_key(scope, description);
  let second = v8::Symbol::for_key(scope, description);
  assert!(std::ptr::eq(&*first, &*second));
  assert!(first.is_symbol());
  assert!(first.description(scope).strict_equals(description.into()));
  let iterator = v8::Symbol::get_iterator(scope);
  assert!(std::ptr::eq(&*iterator, &*v8::Symbol::get_iterator(scope)));

  let escaped = {
    let escapable = std::pin::pin!(v8::EscapableHandleScope::new(scope));
    let scope = &mut escapable.init();
    let value = v8::Number::new(scope, 17.0);
    scope.escape(value)
  };
  assert_eq!(escaped.value(), 17.0);
  assert!(!scope.has_pending_background_tasks());
}

#[test]
fn weak_handles_preserve_identity_and_context_slots_drop_once() {
  initialize();
  let drops = Rc::new(Cell::new(0));
  let mut isolate = v8::Isolate::new(Default::default());

  {
    v8::scope!(let scope, &mut isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let object = v8::Object::new(scope);
    let weak = v8::Weak::new(scope, object);
    assert!(!weak.is_empty());
    let reopened = weak.to_local(scope).unwrap();
    assert!(std::ptr::eq(&*object, &*reopened));
    drop(weak);

    let marker = Rc::new(DropMarker(drops.clone()));
    assert!(context.set_slot(marker.clone()).is_none());
    assert!(Rc::ptr_eq(
      &marker,
      &context.get_slot::<DropMarker>().unwrap()
    ));
    drop(marker);
    assert_eq!(drops.get(), 0);
  }

  drop(isolate);
  assert_eq!(drops.get(), 1);
}

#[test]
fn preserves_native_function_lengths() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let length_key = v8::String::new(scope, "length").unwrap();
  for length in [0, 1, 3, 8] {
    let template = v8::FunctionTemplate::builder_raw(noop_callback)
      .length(length)
      .build(scope);
    let templated = template.get_function(scope).unwrap();
    let direct = v8::Function::builder_raw(noop_callback)
      .length(length)
      .build(scope)
      .unwrap();
    for function in [templated, direct] {
      let value = function.get(scope, length_key.into()).unwrap();
      assert_eq!(value.number_value(scope), Some(length as f64));
    }
  }
}

#[test]
fn preserves_function_names_from_templates_and_explicit_updates() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);

  let template = v8::FunctionTemplate::new_raw(scope, noop_callback);
  let class_name = v8::String::new(scope, "Deno.core.Op").unwrap();
  template.set_class_name(class_name);
  let function = template.get_function(scope).unwrap();

  let template_name = function.get_name(scope);
  assert!(std::ptr::eq(&*template_name, &*class_name));
  assert_eq!(template_name.to_rust_string_lossy(scope), "Deno.core.Op");

  let name_key = v8::String::new(scope, "name").unwrap();
  let name_property = function.get(scope, name_key.into()).unwrap();
  let name_property = v8::Local::<v8::String>::try_from(name_property).unwrap();
  assert!(std::ptr::eq(&*name_property, &*class_name));

  let explicit_name = v8::String::new(scope, "op_read").unwrap();
  function.set_name(explicit_name);

  let updated_name = function.get_name(scope);
  assert!(std::ptr::eq(&*updated_name, &*explicit_name));
  assert_eq!(updated_name.to_rust_string_lossy(scope), "op_read");
  let updated_property = function.get(scope, name_key.into()).unwrap();
  let updated_property =
    v8::Local::<v8::String>::try_from(updated_property).unwrap();
  assert!(std::ptr::eq(&*updated_property, &*explicit_name));
}

#[test]
fn try_catch_tracks_nested_scopes_and_exact_exception_identity() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let mut scope = v8::ContextScope::new(scope, context);

  {
    v8::tc_scope!(let outer, &mut scope);
    assert!(!outer.has_caught());
    assert!(outer.exception().is_none());
    assert!(outer.can_continue());
    assert!(!outer.has_terminated());
    assert!(!outer.is_verbose());
    outer.set_verbose(true);
    assert!(outer.is_verbose());

    // An exception caught by an inner scope is swallowed when that scope is
    // destroyed without rethrowing, and the outer scope becomes current again.
    {
      v8::tc_scope!(let inner, outer);
      assert!(!inner.has_caught());
      assert!(inner.exception().is_none());

      let message = v8::String::new(inner, "inner failure").unwrap();
      let exception = v8::Exception::type_error(inner, message);
      let exception_ptr = &*exception as *const v8::Value;
      assert!(inner.throw_exception(exception).is_undefined());
      assert!(inner.has_caught());
      let caught = inner.exception().unwrap();
      assert!(std::ptr::eq(&*caught, exception_ptr));
    }
    assert!(!outer.has_caught());
    assert!(outer.exception().is_none());

    // Rethrow keeps the inner exception live across Reset and transfers it to
    // the restored outer scope when the inner TryCatch is destroyed.
    let rethrown_ptr = {
      v8::tc_scope!(let inner, outer);
      let message = v8::String::new(inner, "rethrown failure").unwrap();
      let exception = v8::Exception::type_error(inner, message);
      let exception_ptr = &*exception as *const v8::Value;
      inner.throw_exception(exception);
      let rethrown = inner.rethrow().unwrap();
      assert!(std::ptr::eq(&*rethrown, exception_ptr));
      inner.reset();
      assert!(inner.has_caught());
      exception_ptr
    };
    assert!(outer.has_caught());
    let caught = outer.exception().unwrap();
    assert!(std::ptr::eq(&*caught, rethrown_ptr));
    outer.reset();
    assert!(!outer.has_caught());
    assert!(outer.exception().is_none());
  }

  // Destruction of the outer scope also leaves the isolate ready for a fresh
  // no-exception TryCatch.
  v8::tc_scope!(let fresh, &mut scope);
  assert!(!fresh.has_caught());
  assert!(fresh.exception().is_none());
}

#[test]
fn precompiled_artifact_requires_exact_graph_binding() {
  const ARTIFACT_A: &[u8] = b"precompiled artifact A";
  const ARTIFACT_B: &[u8] = b"precompiled artifact B";
  let entry = "file:///main.ts";
  let modules = [
    (entry, "import './dep.ts';"),
    ("file:///dep.ts", "export const value = 42;"),
  ];
  let artifact = std::env::temp_dir().join(format!(
    "v8x-js2wasm-graph-binding-{}.cwasm",
    std::process::id()
  ));
  let mut binding = artifact.as_os_str().to_os_string();
  binding.push(".graph-sha256");
  let binding = PathBuf::from(binding);
  let _ = fs::remove_file(&artifact);
  let _ = fs::remove_file(&binding);
  fs::write(&artifact, ARTIFACT_A).unwrap();

  let missing =
    v8::js2wasm_verify_graph_binding_for_test(&artifact, entry, &modules)
      .unwrap_err();
  assert!(missing.contains("read js2wasm graph binding"));

  fs::write(
    &binding,
    format!(
      "graph-sha256 {}\nartifact-sha256 {}\n",
      "0".repeat(64),
      "0".repeat(64),
    ),
  )
  .unwrap();
  let mismatch =
    v8::js2wasm_verify_graph_binding_for_test(&artifact, entry, &modules)
      .unwrap_err();
  assert!(mismatch.contains("graph binding mismatch"));

  // Sidecar emission binds the bytes supplied by the compiler, not whatever
  // a concurrent writer may have placed at the output path.
  fs::write(&artifact, ARTIFACT_B).unwrap();
  v8::js2wasm_write_graph_binding_for_test(
    &artifact, ARTIFACT_A, entry, &modules,
  )
  .unwrap();
  assert!(
    v8::js2wasm_verify_graph_binding_for_test(&artifact, entry, &modules)
      .unwrap_err()
      .contains("artifact binding mismatch")
  );

  fs::write(&artifact, ARTIFACT_A).unwrap();
  v8::js2wasm_verify_graph_binding_for_test(&artifact, entry, &modules)
    .unwrap();

  fs::write(&artifact, ARTIFACT_B).unwrap();
  let artifact_mismatch =
    v8::js2wasm_verify_graph_binding_for_test(&artifact, entry, &modules)
      .unwrap_err();
  assert!(artifact_mismatch.contains("artifact binding mismatch"));
  fs::write(&artifact, ARTIFACT_A).unwrap();

  let changed_modules = [
    (entry, "import './dep.ts';"),
    ("file:///dep.ts", "export const value = 43;"),
  ];
  assert!(
    v8::js2wasm_verify_graph_binding_for_test(
      &artifact,
      entry,
      &changed_modules,
    )
    .unwrap_err()
    .contains("graph binding mismatch")
  );

  fs::remove_file(artifact).unwrap();
  fs::remove_file(binding).unwrap();
}

#[test]
#[ignore = "requires a configured js2wasm compiler or a graph-bound V8X_JS2WASM_AOT_MODULE"]
fn evaluates_raw_typescript_graph_through_wasmtime() {
  #[cfg(feature = "js2wasm_runtime_compile")]
  let runtime_cache = (std::env::var_os("V8X_JS2WASM_AOT_MODULE").is_none())
    .then(|| {
      let path = std::env::temp_dir().join(format!(
        "v8x-js2wasm-runtime-cache-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
          .duration_since(std::time::UNIX_EPOCH)
          .unwrap()
          .as_nanos(),
      ));
      // This ignored integration test is run by exact name, before the backend
      // is initialized, so it exclusively owns the process-wide cache setting.
      unsafe { std::env::set_var("V8X_JS2WASM_CACHE_DIR", &path) };
      path
    });

  initialize();
  assert_eq!(v8::V8X_ENGINE, "js2wasm");
  assert!(
    std::env::var_os("V8X_JS2WASM_COMPILER_SCRIPT").is_some()
      || std::env::var_os("V8X_JS2WASM_COMPILER").is_some()
      || std::env::var_os("V8X_JS2WASM_AOT_MODULE").is_some(),
    "set V8X_JS2WASM_COMPILER to a graph compiler, V8X_JS2WASM_COMPILER_SCRIPT to compile-graph.ts, or V8X_JS2WASM_AOT_MODULE to a trusted Wasmtime-precompiled artifact"
  );

  let before = v8::js2wasm_runtime_stats().unwrap();
  evaluate_graph_once();
  evaluate_graph_once();
  let after = v8::js2wasm_runtime_stats().unwrap();

  assert_eq!(after.module_loads - before.module_loads, 1);
  assert_eq!(after.cached_modules - before.cached_modules, 1);
  assert_eq!(after.instantiations - before.instantiations, 2);

  #[cfg(feature = "js2wasm_runtime_compile")]
  if let Some(runtime_cache) = runtime_cache {
    assert_eq!(after.compilations - before.compilations, 1);
    assert_eq!(after.cache_hits - before.cache_hits, 1);
    let entries = fs::read_dir(&runtime_cache).unwrap().count();
    assert_eq!(entries, 2, "cache must contain an artifact and its binding");
    fs::remove_dir_all(runtime_cache).unwrap();
  }
}

#[test]
#[ignore = "requires graph-bound application and runtime-eval provider artifacts, or their runtime-profile build inputs"]
fn links_runtime_eval_provider_with_shared_realm_state() {
  #[cfg(feature = "js2wasm_runtime_compile")]
  {
    assert!(
      std::env::var_os("V8X_JS2WASM_RUNTIME_EVAL_WASM").is_some()
        || std::env::var_os("V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE").is_some(),
      "set V8X_JS2WASM_RUNTIME_EVAL_WASM or V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE to the zero-import runtime-eval provider"
    );
    assert!(
      std::env::var_os("V8X_JS2WASM_COMPILER_SCRIPT").is_some()
        || std::env::var_os("V8X_JS2WASM_COMPILER").is_some()
        || std::env::var_os("V8X_JS2WASM_AOT_MODULE").is_some(),
      "configure the js2wasm graph compiler or graph-bound application artifact"
    );
  }
  #[cfg(not(feature = "js2wasm_runtime_compile"))]
  {
    assert!(
      std::env::var_os("V8X_JS2WASM_AOT_MODULE").is_some(),
      "set V8X_JS2WASM_AOT_MODULE to the graph-bound application artifact"
    );
    assert!(
      std::env::var_os("V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE").is_some(),
      "set V8X_JS2WASM_RUNTIME_EVAL_AOT_MODULE to the trusted provider artifact"
    );
  }

  #[cfg(feature = "js2wasm_runtime_compile")]
  let cache = std::env::temp_dir().join(format!(
    "v8x-js2wasm-runtime-eval-cache-{}-{}",
    std::process::id(),
    std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap()
      .as_nanos(),
  ));
  #[cfg(feature = "js2wasm_runtime_compile")]
  unsafe {
    std::env::set_var("V8X_JS2WASM_CACHE_DIR", &cache)
  };
  unsafe { std::env::set_var("V8X_JS2WASM_VERIFY_RUNTIME_EVAL_STATE", "1") };

  initialize();
  let before = v8::js2wasm_runtime_stats().unwrap();
  evaluate_runtime_eval_graph_once();
  evaluate_runtime_eval_graph_once();
  let after = v8::js2wasm_runtime_stats().unwrap();

  assert_eq!(after.module_loads - before.module_loads, 1);
  assert_eq!(after.instantiations - before.instantiations, 2);
  assert_eq!(
    after.runtime_eval_provider_loads - before.runtime_eval_provider_loads,
    1
  );
  assert_eq!(
    after.runtime_eval_instantiations - before.runtime_eval_instantiations,
    2
  );
  #[cfg(feature = "js2wasm_runtime_compile")]
  {
    assert_eq!(after.compilations - before.compilations, 1);
    assert!(after.cache_hits > before.cache_hits);
    fs::remove_dir_all(cache).unwrap();
  }
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
fn context_store_preserves_graphs_and_primary_instance() {
  v8::js2wasm_test_context_store();
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
fn context_store_preserves_primary_after_initialization_error() {
  v8::js2wasm_test_context_store_failure();
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
#[ignore = "requires V8X_JS2WASM_CONTEXT_VALUES_WASM from test-context-value-bridge.mjs"]
fn transfers_context_values_through_embedded_wasmtime() {
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("compiled context value fixture");
  v8::js2wasm_test_realm_values(Path::new(&path)).unwrap();
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
fn precompiles_exact_deno_core_artifact() {
  let artifact = std::env::var_os("V8X_JS2WASM_DENO_CORE_WASM").expect(
    "set V8X_JS2WASM_DENO_CORE_WASM to the raw pinned bootstrap module",
  );
  v8::js2wasm_precompile_deno_core_for_test(Path::new(&artifact))
    .expect("precompile configured Deno core artifact");
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
fn precompiles_exact_runtime_eval_provider_artifact() {
  let artifact = std::env::var_os("V8X_JS2WASM_RUNTIME_EVAL_WASM").expect(
    "set V8X_JS2WASM_RUNTIME_EVAL_WASM to the raw interpreter provider",
  );
  v8::js2wasm_precompile_runtime_eval_provider_for_test(Path::new(&artifact))
    .expect("precompile configured runtime-eval provider artifact");
}

#[test]
#[cfg(feature = "js2wasm_deno_poc")]
fn boots_exact_deno_core_artifact_in_two_wasmtime_stores() {
  let artifact = std::env::var_os("V8X_JS2WASM_DENO_CORE_WASM").expect(
    "set V8X_JS2WASM_DENO_CORE_WASM to the raw pinned bootstrap module",
  );
  v8::js2wasm_bootstrap_raw_module_for_test(Path::new(&artifact))
    .expect("boot exact Deno core artifact through embedded Wasmtime");
}

#[test]
#[cfg(feature = "js2wasm_deno_poc")]
fn routes_exact_deno_core_scripts_through_public_script_run() {
  initialize();
  DENO_OP_EVENTS.with(|events| events.borrow_mut().clear());
  let before = v8::js2wasm_runtime_stats().unwrap();
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

  // Reject both a valid audited script in the wrong phase and modified source
  // without advancing the transaction. The exact sequence must still run
  // successfully afterward.
  let out_of_order_name = "00_infra.js";
  let out_of_order_source =
    fs::read_to_string(fixture_dir.join(out_of_order_name)).unwrap();
  let out_of_order_source =
    v8::String::new(scope, &out_of_order_source).unwrap();
  let out_of_order_specifier = format!("ext:core/{out_of_order_name}");
  let out_of_order_resource = v8::String::new(scope, &out_of_order_specifier)
    .unwrap()
    .into();
  let out_of_order_origin = classic_origin(scope, out_of_order_resource);
  let out_of_order_script =
    v8::Script::compile(scope, out_of_order_source, Some(&out_of_order_origin))
      .unwrap();
  assert!(out_of_order_script.run(scope).is_none());

  let tampered_name = "00_primordials.js";
  let mut tampered_source =
    fs::read_to_string(fixture_dir.join(tampered_name)).unwrap();
  tampered_source.push_str("\n// tampered");
  let tampered_source = v8::String::new(scope, &tampered_source).unwrap();
  let tampered_specifier = format!("ext:core/{tampered_name}");
  let tampered_resource =
    v8::String::new(scope, &tampered_specifier).unwrap().into();
  let tampered_origin = classic_origin(scope, tampered_resource);
  let tampered_script =
    v8::Script::compile(scope, tampered_source, Some(&tampered_origin))
      .unwrap();
  assert!(tampered_script.run(scope).is_none());

  let mut print_data_marker = 0_u8;
  let mut sum_data_marker = 0_u8;
  let print_data =
    std::ptr::addr_of_mut!(print_data_marker).cast::<std::ffi::c_void>();
  let sum_data =
    std::ptr::addr_of_mut!(sum_data_marker).cast::<std::ffi::c_void>();
  assert_ne!(print_data, sum_data);
  let mut deno_op_handles: Option<(
    v8::Global<v8::Function>,
    v8::Global<v8::Function>,
  )> = None;

  for name in [
    "00_primordials.js",
    "00_infra.js",
    "02_timers.js",
    "01_core.js",
  ] {
    if name == "02_timers.js" {
      let print_data = v8::External::new(scope, print_data);
      let print_template =
        v8::FunctionTemplate::builder_raw(deno_op_print_callback)
          .data(print_data.into())
          .length(2)
          .constructor_behavior(v8::ConstructorBehavior::Throw)
          .build(scope);
      let print_op = print_template.get_function(scope).unwrap();
      let print_key = v8::String::new(scope, "op_print").unwrap();
      print_op.set_name(print_key);
      assert_eq!(
        ops.set(scope, print_key.into(), print_op.into()),
        Some(true)
      );

      let sum_data = v8::External::new(scope, sum_data);
      let sum_template =
        v8::FunctionTemplate::builder_raw(deno_op_sum_callback)
          .data(sum_data.into())
          .length(1)
          .constructor_behavior(v8::ConstructorBehavior::Throw)
          .build(scope);
      let sum_op = sum_template.get_function(scope).unwrap();
      let sum_key = v8::String::new(scope, "op_sum").unwrap();
      sum_op.set_name(sum_key);
      assert_eq!(ops.set(scope, sum_key.into(), sum_op.into()), Some(true));

      deno_op_handles = Some((
        v8::Global::new(scope, print_op),
        v8::Global::new(scope, sum_op),
      ));
    }

    let source = fs::read_to_string(fixture_dir.join(name)).unwrap();
    let source = v8::String::new(scope, &source).unwrap();
    let specifier = format!("ext:core/{name}");
    let resource = v8::String::new(scope, &specifier).unwrap().into();
    let script_origin = classic_origin(scope, resource);
    let script = v8::Script::compile(scope, source, Some(&script_origin))
      .unwrap_or_else(|| panic!("compile {specifier}"));
    assert!(script.run(scope).is_some(), "run {specifier}");
  }

  let (print_op, sum_op) = deno_op_handles.unwrap();
  let print_op = v8::Local::new(scope, &print_op);
  let sum_op = v8::Local::new(scope, &sum_op);
  let print_key = v8::String::new(scope, "op_print").unwrap();
  let installed_print = ops.get(scope, print_key.into()).unwrap();
  let installed_print =
    v8::Local::<v8::Function>::try_from(installed_print).unwrap();
  let sum_key = v8::String::new(scope, "op_sum").unwrap();
  let installed_sum = ops.get(scope, sum_key.into()).unwrap();
  let installed_sum =
    v8::Local::<v8::Function>::try_from(installed_sum).unwrap();
  assert!(std::ptr::eq(&*installed_print, &*print_op));
  assert!(std::ptr::eq(&*installed_sum, &*sum_op));
  assert!(!std::ptr::eq(&*installed_print, &*installed_sum));

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

  let module_source = fs::read_to_string(fixture_dir.join("mod.js")).unwrap();
  let module_source = v8::String::new(scope, &module_source).unwrap();
  let module_resource =
    v8::String::new(scope, "ext:core/mod.js").unwrap().into();
  let module_origin = origin(scope, module_resource);
  let mut module_source =
    v8::script_compiler::Source::new(module_source, Some(&module_origin));
  let module =
    v8::script_compiler::compile_module(scope, &mut module_source).unwrap();
  assert_eq!(module.get_status(), v8::ModuleStatus::Uninstantiated);
  assert!(
    module
      .instantiate_module(scope, resolve_dependency)
      .unwrap()
  );
  let namespace_before = module.get_module_namespace();
  let evaluation = module.evaluate(scope).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(evaluation).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Fulfilled);
  assert!(promise.result(scope).is_undefined());
  assert_eq!(module.get_status(), v8::ModuleStatus::Evaluated);
  let namespace_after = module.get_module_namespace();
  assert!(std::ptr::eq(&*namespace_before, &*namespace_after));

  let usage_source =
    fs::read_to_string(fixture_dir.join("hello_world_usage.js")).unwrap();
  let usage_source = v8::String::new(scope, &usage_source).unwrap();
  let usage_resource = v8::String::new(scope, "<usage>").unwrap().into();
  let usage_origin = classic_origin(scope, usage_resource);
  let usage_script =
    v8::Script::compile(scope, usage_source, Some(&usage_origin)).unwrap();
  {
    v8::tc_scope!(let try_catch, scope);
    let result = usage_script.run(try_catch).unwrap();
    assert!(result.is_undefined());
    assert!(!try_catch.has_caught());
    assert!(try_catch.exception().is_none());
  }

  let expected_events = vec![
    DenoOpEvent::Print {
      message: "The sum of\n".to_string(),
      is_error: false,
      data: print_data as usize,
    },
    DenoOpEvent::Print {
      message: "1,2,3\n".to_string(),
      is_error: false,
      data: print_data as usize,
    },
    DenoOpEvent::Print {
      message: "is\n".to_string(),
      is_error: false,
      data: print_data as usize,
    },
    DenoOpEvent::SumArray {
      values: vec![1.0, 2.0, 3.0],
      data: sum_data as usize,
    },
    DenoOpEvent::Print {
      message: "6\n".to_string(),
      is_error: false,
      data: print_data as usize,
    },
    DenoOpEvent::SumNumber {
      value: 0.0,
      data: sum_data as usize,
    },
    DenoOpEvent::Print {
      message: "Exception:\n".to_string(),
      is_error: false,
      data: print_data as usize,
    },
    DenoOpEvent::Print {
      message: "TypeError: serde_v8 error: invalid type; expected: array, got: Number\n"
        .to_string(),
      is_error: false,
      data: print_data as usize,
    },
  ];
  DENO_OP_EVENTS.with(|events| {
    let events = events.borrow();
    assert_eq!(
      events
        .iter()
        .filter(|event| matches!(event, DenoOpEvent::Print { .. }))
        .count(),
      6
    );
    assert_eq!(
      events
        .iter()
        .filter(|event| {
          matches!(
            event,
            DenoOpEvent::SumArray { .. } | DenoOpEvent::SumNumber { .. }
          )
        })
        .count(),
      2
    );
    assert_eq!(&*events, &expected_events);
  });

  let after = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(after.instantiations - before.instantiations, 1);
}
#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
#[ignore = "requires V8X_JS2WASM_CONTEXT_VALUES_WASM from test-context-value-bridge.mjs"]
fn public_objects_use_compiled_realm_and_preserve_identity() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("compiled context fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let global = context.global(scope);
  let sample_key = v8::String::new(scope, "sample").unwrap();
  let first = global.get(scope, sample_key.into()).unwrap();
  let second = global.get(scope, sample_key.into()).unwrap();
  assert!(first.strict_equals(second));
  let sample = v8::Local::<v8::Object>::try_from(first).unwrap();
  let answer_key = v8::String::new(scope, "answer").unwrap();
  assert_eq!(
    sample
      .get(scope, answer_key.into())
      .unwrap()
      .number_value(scope),
    Some(42.0)
  );
  let number = v8::Number::new(scope, 99.0);
  assert_eq!(
    sample.set(scope, answer_key.into(), number.into()),
    Some(true)
  );
  assert_eq!(
    sample
      .get(scope, answer_key.into())
      .unwrap()
      .number_value(scope),
    Some(99.0)
  );
  let alias = v8::String::new(scope, "alias").unwrap();
  assert_eq!(global.set(scope, alias.into(), sample.into()), Some(true));
  assert!(
    global
      .get(scope, alias.into())
      .unwrap()
      .strict_equals(first)
  );
  let greeting = v8::String::new(scope, "greeting").unwrap();
  let text = v8::String::new(scope, "Grüße 😀").unwrap();
  assert_eq!(global.set(scope, greeting.into(), text.into()), Some(true));
  assert_eq!(
    global
      .get(scope, greeting.into())
      .unwrap()
      .to_rust_string_lossy(scope),
    "Grüße 😀"
  );
  let values_key = v8::String::new(scope, "values").unwrap();
  let values_value = global.get(scope, values_key.into()).unwrap();
  assert!(values_value.is_array());
  assert!(
    global
      .get(scope, values_key.into())
      .unwrap()
      .strict_equals(values_value)
  );
  let values = v8::Local::<v8::Array>::try_from(values_value).unwrap();
  assert_eq!(values.length(), 3);
  assert_eq!(
    values.get_index(scope, 0).unwrap().number_value(scope),
    Some(1.0)
  );
  assert!(values.get_index(scope, 1).unwrap().is_true());
  assert!(values.get_index(scope, 2).unwrap().is_null());
  let false_value = v8::Boolean::new(scope, false);
  assert_eq!(values.set_index(scope, 1, false_value.into()), Some(true));
  assert!(values.get_index(scope, 1).unwrap().is_false());
  let null_value = v8::null(scope);
  assert_eq!(values.set_index(scope, 2, null_value.into()), Some(true));
  assert!(values.get_index(scope, 2).unwrap().is_null());
  let nine = v8::Number::new(scope, 9.0);
  assert_eq!(values.set_index(scope, 4, nine.into()), Some(true));
  assert_eq!(values.length(), 5);
  assert!(values.get_index(scope, 3).unwrap().is_undefined());
  assert_eq!(
    values.get_index(scope, 4).unwrap().number_value(scope),
    Some(9.0)
  );
  let identity_key = v8::String::new(scope, "identity").unwrap();
  let callable_value = global.get(scope, identity_key.into()).unwrap();
  assert!(callable_value.is_function());
  assert!(
    global
      .get(scope, identity_key.into())
      .unwrap()
      .strict_equals(callable_value)
  );
  let callable = v8::Local::<v8::Function>::try_from(callable_value).unwrap();
  assert!(
    callable
      .call(scope, global.into(), &[sample.into()])
      .unwrap()
      .strict_equals(first)
  );
  let receiver_key = v8::String::new(scope, "useReceiver").unwrap();
  let method = v8::Local::<v8::Function>::try_from(
    global.get(scope, receiver_key.into()).unwrap(),
  )
  .unwrap();
  let delta = v8::Number::new(scope, 1.0);
  assert_eq!(
    method
      .call(scope, sample.into(), &[delta.into()])
      .unwrap()
      .number_value(scope),
    Some(100.0)
  );
  let throw_key = v8::String::new(scope, "throwFromRealm").unwrap();
  let throwing = v8::Local::<v8::Function>::try_from(
    global.get(scope, throw_key.into()).unwrap(),
  )
  .unwrap();
  {
    v8::tc_scope!(let caught, scope);
    assert!(throwing.call(caught, global.into(), &[]).is_none());
    assert!(caught.has_caught());
  }
  {
    v8::tc_scope!(let caught, scope);
    assert!(callable.new_instance(caught, &[]).is_none());
    assert!(caught.has_caught());
  }
}

#[test]
fn is_false_recognizes_only_the_boolean_value() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  assert!(v8::Boolean::new(scope, false).is_false());
  assert!(!v8::Boolean::new(scope, true).is_false());
  assert!(!v8::Number::new(scope, 0.0).is_false());
  assert!(!v8::null(scope).is_false());
  assert!(!v8::undefined(scope).is_false());
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn transfers_explicit_prototypes_and_keeps_updates_live() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("compiled context fixture");
  let global = context.global(scope);
  let prototype = v8::Object::new(scope);
  let nil = v8::null(scope);
  assert_eq!(prototype.set_prototype(scope, nil.into()), Some(true));
  let answer = v8::String::new(scope, "answer").unwrap();
  let number = v8::Number::new(scope, 73.0);
  assert_eq!(
    prototype.set(scope, answer.into(), number.into()),
    Some(true)
  );
  let first = v8::Object::new(scope);
  let second = v8::Object::new(scope);
  for (name, object) in [("first", first), ("second", second)] {
    assert_eq!(object.set_prototype(scope, prototype.into()), Some(true));
    let key = v8::String::new(scope, name).unwrap();
    assert_eq!(global.set(scope, key.into(), object.into()), Some(true));
  }
  // A property cycle is legal even though a prototype cycle is not.
  let child = v8::String::new(scope, "child").unwrap();
  assert_eq!(prototype.set(scope, child.into(), first.into()), Some(true));
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  for object in [first, second] {
    assert!(
      object
        .get_prototype(scope)
        .unwrap()
        .strict_equals(prototype.into())
    );
    assert_eq!(
      object
        .get(scope, answer.into())
        .unwrap()
        .number_value(scope),
      Some(73.0)
    );
  }
  assert!(prototype.get_prototype(scope).unwrap().is_null());
  assert!(
    prototype
      .get(scope, child.into())
      .unwrap()
      .strict_equals(first.into())
  );
  assert_eq!(prototype.set_prototype(scope, first.into()), Some(false));
  assert!(prototype.get_prototype(scope).unwrap().is_null());
  assert_eq!(first.set_prototype(scope, nil.into()), Some(true));
  assert!(first.get_prototype(scope).unwrap().is_null());
  assert!(first.get(scope, answer.into()).unwrap().is_undefined());
  assert_eq!(
    second
      .get(scope, answer.into())
      .unwrap()
      .number_value(scope),
    Some(73.0)
  );
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn transfers_host_graph_without_losing_identity_or_descriptors() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("compiled context fixture");
  let global = context.global(scope);
  let template = v8::ObjectTemplate::new(scope);
  let fixed = v8::String::new(scope, "fixed").unwrap();
  let nine = v8::Number::new(scope, 9.0);
  template.set_with_attr(
    fixed.into(),
    nine.into(),
    v8::PropertyAttribute::READ_ONLY
      | v8::PropertyAttribute::DONT_ENUM
      | v8::PropertyAttribute::DONT_DELETE,
  );
  let proto_key = v8::String::new(scope, "__proto__").unwrap();
  let eleven = v8::Number::new(scope, 11.0);
  template.set(proto_key.into(), eleven.into());
  let root = template.new_instance(scope).unwrap();
  let child = v8::Object::new(scope);
  let answer = v8::String::new(scope, "answer").unwrap();
  let seven = v8::Number::new(scope, 7.0);
  assert_eq!(child.set(scope, answer.into(), seven.into()), Some(true));
  for (key, value) in [("self", root), ("left", child), ("right", child)] {
    let key = v8::String::new(scope, key).unwrap();
    assert_eq!(root.set(scope, key.into(), value.into()), Some(true));
  }
  let list = v8::Array::new_with_elements(scope, &[child.into(), root.into()]);
  let list_key = v8::String::new(scope, "list").unwrap();
  assert_eq!(root.set(scope, list_key.into(), list.into()), Some(true));
  let root_key = v8::String::new(scope, "hostRoot").unwrap();
  let global_key = v8::String::new(scope, "hostGlobal").unwrap();
  assert_eq!(global.set(scope, root_key.into(), root.into()), Some(true));
  assert_eq!(
    root.set(scope, global_key.into(), global.into()),
    Some(true)
  );
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  assert!(
    global
      .get(scope, root_key.into())
      .unwrap()
      .strict_equals(root.into())
  );
  assert!(
    root
      .get(scope, global_key.into())
      .unwrap()
      .strict_equals(global.into())
  );
  let inspect_key = v8::String::new(scope, "inspectHost").unwrap();
  let inspect = v8::Local::<v8::Function>::try_from(
    global.get(scope, inspect_key.into()).unwrap(),
  )
  .unwrap();
  assert_eq!(
    inspect
      .call(scope, global.into(), &[root.into()])
      .unwrap()
      .number_value(scope),
    Some(1.0)
  );
  assert_eq!(
    child.get(scope, answer.into()).unwrap().number_value(scope),
    Some(23.0)
  );
  assert!(
    list
      .get_index(scope, 0)
      .unwrap()
      .strict_equals(child.into())
  );
  assert!(list.get_index(scope, 1).unwrap().strict_equals(root.into()));
  let self_key = v8::String::new(scope, "self").unwrap();
  assert!(
    root
      .get(scope, self_key.into())
      .unwrap()
      .strict_equals(root.into())
  );
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn rejected_host_graph_can_be_repaired_and_retried() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("compiled context fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let global = context.global(scope);
  let root = v8::Object::new(scope);
  let child = v8::Object::new(scope);
  let nested = v8::String::new(scope, "nested").unwrap();
  let callback_key = v8::String::new(scope, "callback").unwrap();
  // Symbols now transfer; a native External still must fail preflight.
  let callback = v8::External::new(scope, std::ptr::null_mut());
  assert_eq!(root.set(scope, nested.into(), child.into()), Some(true));
  assert_eq!(
    child.set(scope, callback_key.into(), callback.into()),
    Some(true)
  );
  let key = v8::String::new(scope, "host").unwrap();
  {
    v8::tc_scope!(let caught, scope);
    assert_eq!(global.set(caught, key.into(), root.into()), None);
    assert!(caught.has_caught());
  }
  assert!(global.get(scope, key.into()).unwrap().is_undefined());
  // A preflight failure must not publish bindings for either object. Repair
  // their ordinary Rust storage and prove the retried transfer sees it.
  let replacement = v8::Number::new(scope, 31.0);
  assert_eq!(
    child.set(scope, callback_key.into(), replacement.into()),
    Some(true)
  );
  assert_eq!(global.set(scope, key.into(), root.into()), Some(true));
  assert!(
    global
      .get(scope, key.into())
      .unwrap()
      .strict_equals(root.into())
  );
  let rebound_child =
    v8::Local::<v8::Object>::try_from(root.get(scope, nested.into()).unwrap())
      .unwrap();
  assert!(rebound_child.strict_equals(child.into()));
  assert_eq!(
    rebound_child
      .get(scope, callback_key.into())
      .unwrap()
      .number_value(scope),
    Some(31.0)
  );
}

unsafe extern "C" fn realm_host_leaf(info: *const v8::FunctionCallbackInfo) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  let value = args.get(0).number_value(scope).unwrap();
  let mut result = parts.return_value;
  result.set_double(value + 1.0);
}

unsafe extern "C" fn realm_host_mutate(info: *const v8::FunctionCallbackInfo) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  let object = v8::Local::<v8::Object>::try_from(args.get(0)).unwrap();
  assert!(args.this().strict_equals(object.into()));
  let array = v8::Local::<v8::Array>::try_from(args.get(1)).unwrap();
  let sum: f64 = (0..array.length())
    .map(|i| {
      array
        .get_index(scope, i)
        .unwrap()
        .number_value(scope)
        .unwrap()
    })
    .sum();
  let key = v8::String::new(scope, "value").unwrap();
  let old = object
    .get(scope, key.into())
    .unwrap()
    .number_value(scope)
    .unwrap();
  let intermediate = v8::Number::new(scope, old + sum);
  let nested = v8::Local::<v8::Function>::try_from(args.get(2)).unwrap();
  let updated = nested
    .call(scope, object.into(), &[intermediate.into()])
    .unwrap();
  assert_eq!(object.set(scope, key.into(), updated), Some(true));
  let mut result = parts.return_value;
  result.set(object.into());
}

unsafe extern "C" fn realm_host_throw(info: *const v8::FunctionCallbackInfo) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let message = v8::String::new(scope, "host failure").unwrap();
  let error = v8::Exception::type_error(scope, message);
  scope.throw_exception(error);
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn calls_rust_from_wasm_with_nested_reentry_and_caught_exceptions() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("compiled fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let global = context.global(scope);
  let key = v8::String::new(scope, "exerciseHost").unwrap();
  let exercise =
    v8::Local::<v8::Function>::try_from(global.get(scope, key.into()).unwrap())
      .unwrap();
  let host = v8::Function::new_raw(scope, realm_host_mutate).unwrap();
  let leaf = v8::Function::new_raw(scope, realm_host_leaf).unwrap();
  assert_eq!(
    exercise
      .call(scope, global.into(), &[host.into(), leaf.into()])
      .unwrap()
      .number_value(scope),
    Some(1.0)
  );
  let key = v8::String::new(scope, "exerciseThrow").unwrap();
  let exercise =
    v8::Local::<v8::Function>::try_from(global.get(scope, key.into()).unwrap())
      .unwrap();
  let host = v8::Function::new_raw(scope, realm_host_throw).unwrap();
  assert_eq!(
    exercise
      .call(scope, global.into(), &[host.into()])
      .unwrap()
      .number_value(scope),
    Some(1.0)
  );
}

#[test]
#[ignore = "requires compiled bootstrap context bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn attaches_host_context_during_bootstrap_and_retains_failed_owner() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let path = std::env::var_os("V8X_JS2WASM_BOOTSTRAP_CONTEXT_WASM")
    .expect("compiled bootstrap fixture");
  for fail in [false, true] {
    let template = v8::ObjectTemplate::new(scope);
    template.set_internal_field_count(2);
    let context = v8::Context::new(
      scope,
      v8::ContextOptions {
        global_template: Some(template),
        ..Default::default()
      },
    );
    let scope = &mut v8::ContextScope::new(scope, context);
    let global = context.global(scope);
    let marker = Box::new(123_u64);
    let marker_ptr = (&*marker as *const u64).cast::<std::ffi::c_void>();
    global.set_aligned_pointer_in_internal_field(0, marker_ptr, 0);
    let number_key = v8::String::new(scope, "hostNumber").unwrap();
    let number = v8::Number::new(scope, 42.0);
    assert_eq!(
      global.set(scope, number_key.into(), number.into()),
      Some(true)
    );
    let callback_key = v8::String::new(scope, "hostCallback").unwrap();
    let callback = v8::Function::builder_raw(realm_host_leaf)
      .length(3)
      .build(scope)
      .unwrap();
    assert_eq!(
      global.set(scope, callback_key.into(), callback.into()),
      Some(true)
    );
    let fail_key = v8::String::new(scope, "bootFail").unwrap();
    let should_fail = v8::Boolean::new(scope, fail);
    assert_eq!(
      global.set(scope, fail_key.into(), should_fail.into()),
      Some(true)
    );
    let result =
      v8::js2wasm_bootstrap_context_for_test(&context, Path::new(&path));
    if fail {
      let error = result.unwrap_err();
      assert!(error.contains("__module_init"), "{error}");
      assert!(
        error.contains("Error: requested bootstrap failure"),
        "{error}"
      );
    } else {
      result.unwrap();
    }
    let length_key = v8::String::new(scope, "length").unwrap();
    assert_eq!(
      callback
        .get(scope, length_key.into())
        .unwrap()
        .number_value(scope),
      Some(3.0)
    );
    if !fail {
      let identity_key = v8::String::new(scope, "identity").unwrap();
      let identity = v8::Local::<v8::Function>::try_from(
        global.get(scope, identity_key.into()).unwrap(),
      )
      .unwrap();
      let returned = identity
        .call(scope, global.into(), &[global.into()])
        .unwrap();
      assert!(returned.strict_equals(global.into()));
      let returned = v8::Local::<v8::Object>::try_from(returned).unwrap();
      assert_eq!(returned.internal_field_count(), 2);
      assert_eq!(
        unsafe { returned.get_aligned_pointer_from_internal_field(0, 0) },
        marker_ptr
      );
    }
    assert_eq!(global.internal_field_count(), 2);
    assert_eq!(
      unsafe { global.get_aligned_pointer_from_internal_field(0, 0) },
      marker_ptr
    );
    global.set_aligned_pointer_in_internal_field(1, marker_ptr, 0);
    assert_eq!(
      unsafe { global.get_aligned_pointer_from_internal_field(1, 0) },
      marker_ptr
    );
    let key = v8::String::new(scope, "initializedFromHost").unwrap();
    assert_eq!(
      global.get(scope, key.into()).unwrap().number_value(scope),
      Some(43.0)
    );
    let error =
      v8::js2wasm_bootstrap_context_for_test(&context, Path::new(&path))
        .unwrap_err();
    assert!(error.contains("already owns a runtime"), "{error}");
    assert_eq!(
      global.get(scope, key.into()).unwrap().number_value(scope),
      Some(43.0)
    );
  }
}

thread_local! {
  static CONTINUATION_EVENTS: RefCell<Vec<f64>> = const { RefCell::new(Vec::new()) };
}

unsafe extern "C" fn observe_continuation(
  info: *const v8::FunctionCallbackInfo,
) {
  let parts = unsafe { &*info }.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let value = scope
    .get_continuation_preserved_embedder_data()
    .number_value(scope)
    .unwrap();
  CONTINUATION_EVENTS.with(|events| events.borrow_mut().push(value));
  let inner = v8::Number::new(scope, 99.0);
  scope.set_continuation_preserved_embedder_data(inner.into());
  if value == 7.0 {
    let message = v8::String::new(scope, "continuation task failed").unwrap();
    let error = v8::Exception::error(scope, message);
    scope.throw_exception(error);
  }
}

#[test]
fn extras_and_native_apis_share_continuation_data() {
  initialize();
  for _ in 0..2 {
    let isolate = &mut v8::Isolate::new(Default::default());
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    assert!(
      scope
        .get_continuation_preserved_embedder_data()
        .is_undefined()
    );
    let extras = context.get_extras_binding_object(scope);
    let get_key =
      v8::String::new(scope, "getContinuationPreservedEmbedderData").unwrap();
    let set_key =
      v8::String::new(scope, "setContinuationPreservedEmbedderData").unwrap();
    let get = v8::Local::<v8::Function>::try_from(
      extras.get(scope, get_key.into()).unwrap(),
    )
    .unwrap();
    let set = v8::Local::<v8::Function>::try_from(
      extras.get(scope, set_key.into()).unwrap(),
    )
    .unwrap();
    let first = v8::Object::new(scope);
    scope.set_continuation_preserved_embedder_data(first.into());
    assert!(
      get
        .call(scope, extras.into(), &[])
        .unwrap()
        .strict_equals(first.into())
    );
    let second = v8::Object::new(scope);
    set.call(scope, extras.into(), &[second.into()]).unwrap();
    assert!(
      scope
        .get_continuation_preserved_embedder_data()
        .strict_equals(second.into())
    );
    set.call(scope, extras.into(), &[]).unwrap();
    assert!(
      scope
        .get_continuation_preserved_embedder_data()
        .is_undefined()
    );
  }
}

#[test]
fn microtasks_restore_captured_continuation_data() {
  initialize();
  CONTINUATION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
  let observe = v8::Function::new_raw(scope, observe_continuation).unwrap();
  let one = v8::Number::new(scope, 1.0);
  scope.set_continuation_preserved_embedder_data(one.into());
  scope.enqueue_microtask(observe);
  let two = v8::Number::new(scope, 2.0);
  scope.set_continuation_preserved_embedder_data(two.into());
  let resolver = v8::PromiseResolver::new(scope).unwrap();
  let promise = resolver.get_promise(scope);
  promise.then2(scope, observe, observe).unwrap();
  let three = v8::Number::new(scope, 3.0);
  scope.set_continuation_preserved_embedder_data(three.into());
  resolver.resolve(scope, one.into()).unwrap();
  let four = v8::Number::new(scope, 4.0);
  scope.set_continuation_preserved_embedder_data(four.into());
  promise.then2(scope, observe, observe).unwrap();
  let seven = v8::Number::new(scope, 7.0);
  scope.set_continuation_preserved_embedder_data(seven.into());
  scope.enqueue_microtask(observe);
  let five = v8::Number::new(scope, 5.0);
  scope.set_continuation_preserved_embedder_data(five.into());
  scope.perform_microtask_checkpoint();
  CONTINUATION_EVENTS
    .with(|events| assert_eq!(&*events.borrow(), &[1.0, 2.0, 4.0, 7.0]));
  assert_eq!(
    scope
      .get_continuation_preserved_embedder_data()
      .number_value(scope),
    Some(5.0)
  );
}

#[test]
#[ignore = "requires compiled staged-core fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn defers_core_until_native_ops_are_registered() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let path =
    std::env::var_os("V8X_JS2WASM_STAGED_CORE_WASM").expect("staged fixture");
  for install_op in [false, true] {
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    v8::js2wasm_bootstrap_context_for_test(&context, Path::new(&path)).unwrap();
    let global = context.global(scope);
    let first = v8::String::new(scope, "first").unwrap();
    assert!(global.get(scope, first.into()).unwrap().is_undefined());
    assert!(v8::js2wasm_run_core_script_for_test(&context, 1).is_err());
    v8::js2wasm_run_core_script_for_test(&context, 0).unwrap();
    v8::js2wasm_run_core_script_for_test(&context, 1).unwrap();
    if !install_op {
      assert!(v8::js2wasm_run_core_script_for_test(&context, 2).is_err());
      let error =
        v8::js2wasm_run_core_script_for_test(&context, 2).unwrap_err();
      assert!(error.contains("expected 2"), "{error}");
      continue;
    }
    let key = v8::String::new(scope, "nativeOp").unwrap();
    let callback = v8::Function::new_raw(scope, realm_host_leaf).unwrap();
    assert_eq!(global.set(scope, key.into(), callback.into()), Some(true));
    v8::js2wasm_run_core_script_for_test(&context, 2).unwrap();
    v8::js2wasm_run_core_script_for_test(&context, 3).unwrap();
    let key = v8::String::new(scope, "moduleAnswer").unwrap();
    let function = v8::Local::<v8::Function>::try_from(
      global.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let result = function.call(scope, global.into(), &[]).unwrap();
    assert_eq!(result.number_value(scope), Some(42.0));
  }
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn shares_host_buffers_with_the_compiled_realm() {
  initialize();
  let deletion_count = AtomicUsize::new(0);
  let mut bytes = vec![0_u8; 16].into_boxed_slice();
  let backing = unsafe {
    v8::ArrayBuffer::new_backing_store_from_ptr(
      bytes.as_mut_ptr().cast(),
      bytes.len(),
      count_backing_store_deletion,
      (&deletion_count as *const AtomicUsize).cast_mut().cast(),
    )
  }
  .make_shared();
  let mut isolate = v8::Isolate::new(Default::default());
  {
    v8::scope!(let scope, &mut isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM").unwrap();
    v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
    let global = context.global(scope);
    let key = v8::String::new(scope, "identity").unwrap();
    let identity = v8::Local::<v8::Function>::try_from(
      global.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &backing);
    let u8_view = v8::Uint8Array::new(scope, buffer, 0, 16).unwrap();
    let u32_view = v8::Uint32Array::new(scope, buffer, 4, 2).unwrap();
    for value in [buffer.into(), u8_view.into(), u32_view.into()] {
      let returned = identity.call(scope, global.into(), &[value]).unwrap();
      assert!(returned.strict_equals(value));
    }
    drop(backing);
    assert_eq!(deletion_count.load(Ordering::SeqCst), 0);
    bytes[4..8].copy_from_slice(&0x12345678_u32.to_le_bytes());
    assert_eq!(
      u32_view.get_index(scope, 0).unwrap().number_value(scope),
      Some(0x12345678_u32 as f64)
    );
    let number = v8::Number::new(scope, 255.0);
    assert_eq!(u8_view.set_index(scope, 7, number.into()), Some(true));
    assert_eq!(bytes[7], 255);
    assert_eq!(
      u32_view.get_index(scope, 0).unwrap().number_value(scope),
      Some(0xff345678_u32 as f64)
    );
    let key = v8::String::new(scope, "throwSharedBuffer").unwrap();
    let thrower = v8::Local::<v8::Function>::try_from(
      global.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    v8::tc_scope!(let scope, scope);
    assert!(
      thrower
        .call(scope, global.into(), &[u8_view.into()])
        .is_none()
    );
    assert!(scope.has_caught());
    assert_eq!(bytes[0], 11);
  }
  drop(isolate);
  assert_eq!(deletion_count.load(Ordering::SeqCst), 1);
}

#[test]
fn private_keys_are_interned_and_hidden_from_public_properties() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let anonymous = v8::Private::new(scope, None);
  assert!(anonymous.name(scope).is_undefined());
  let name = v8::String::new(scope, "Deno#error").unwrap();
  let private = v8::Private::new(scope, Some(name));
  let api = v8::Private::for_api(scope, Some(name));
  let other_name = v8::String::new(scope, "Deno#error").unwrap();
  let same_api = v8::Private::for_api(scope, Some(other_name));
  assert!(api == same_api);
  assert!(api != private);
  assert!(private.name(scope).strict_equals(name.into()));
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let object = v8::Object::new(scope);
  let child = v8::Object::new(scope);
  let sentinel = v8::Object::new(scope);
  assert_eq!(object.has_private(scope, api), Some(false));
  assert!(object.get_private(scope, api).unwrap().is_undefined());
  assert_eq!(object.delete_private(scope, api), Some(true));
  assert_eq!(object.set_private(scope, api, sentinel.into()), Some(true));
  assert_eq!(object.has_private(scope, same_api), Some(true));
  assert!(
    object
      .get_private(scope, same_api)
      .unwrap()
      .strict_equals(sentinel.into())
  );
  assert!(object.get(scope, name.into()).unwrap().is_undefined());
  assert_eq!(
    object
      .get_own_property_names(scope, Default::default())
      .unwrap()
      .length(),
    0
  );
  let public_value = v8::Number::new(scope, 42.0);
  assert_eq!(
    object.set(scope, name.into(), public_value.into()),
    Some(true)
  );
  assert!(
    object
      .get_private(scope, api)
      .unwrap()
      .strict_equals(sentinel.into())
  );
  assert_eq!(child.set_prototype(scope, object.into()), Some(true));
  assert_eq!(child.has_private(scope, api), Some(false));
  assert!(child.get_private(scope, api).unwrap().is_undefined());
  let undefined = v8::undefined(scope);
  assert_eq!(object.set_private(scope, api, undefined.into()), Some(true));
  assert_eq!(object.has_private(scope, api), Some(true));
  assert_eq!(object.delete_private(scope, api), Some(true));
  assert_eq!(object.has_private(scope, api), Some(false));
  assert_eq!(
    object.get(scope, name.into()).unwrap().number_value(scope),
    Some(42.0)
  );
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn private_keys_survive_compiled_realm_roundtrips() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM").unwrap();
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let global = context.global(scope);
  let name = v8::String::new(scope, "sample").unwrap();
  let object =
    v8::Local::<v8::Object>::try_from(global.get(scope, name.into()).unwrap())
      .unwrap();
  let key_name = v8::String::new(scope, "Deno#originalError").unwrap();
  let key = v8::Private::for_api(scope, Some(key_name));
  let sentinel = v8::Object::new(scope);
  assert_eq!(object.set_private(scope, key, sentinel.into()), Some(true));
  assert!(object.get(scope, key_name.into()).unwrap().is_undefined());
  let name = v8::String::new(scope, "identity").unwrap();
  let identity = v8::Local::<v8::Function>::try_from(
    global.get(scope, name.into()).unwrap(),
  )
  .unwrap();
  let returned = identity
    .call(scope, global.into(), &[object.into()])
    .unwrap();
  let returned = v8::Local::<v8::Object>::try_from(returned).unwrap();
  assert!(returned.strict_equals(object.into()));
  assert!(
    returned
      .get_private(scope, key)
      .unwrap()
      .strict_equals(sentinel.into())
  );
  assert_eq!(returned.delete_private(scope, key), Some(true));
  assert_eq!(object.has_private(scope, key), Some(false));
}

#[test]
#[cfg(feature = "js2wasm_runtime_compile")]
fn graph_packages_bind_entry_source_and_bytes() {
  v8::js2wasm_test_graph_packages();
}

#[test]
#[ignore = "requires compiled function-name context fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn transfers_callable_values_with_non_string_name_properties() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_FUNCTION_NAMES_WASM")
    .expect("function name fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let global = context.global(scope);
  let name_key = v8::String::new(scope, "name").unwrap();
  for (key, expected) in [
    ("unnamedForHost", 42.0),
    ("numericNameForHost", 43.0),
    ("namedForHost", 44.0),
  ] {
    let key = v8::String::new(scope, key).unwrap();
    let value = global
      .get(scope, key.into())
      .expect("function transfer must not require a string name");
    let function = v8::Local::<v8::Function>::try_from(value).unwrap();
    assert!(global.get(scope, key.into()).unwrap().strict_equals(value));
    assert_eq!(
      function
        .call(scope, global.into(), &[])
        .unwrap()
        .number_value(scope),
      Some(expected)
    );
    let name = function.get(scope, name_key.into()).unwrap();
    if expected == 42.0 {
      assert!(name.is_undefined());
    } else if expected == 43.0 {
      assert_eq!(name.number_value(scope), Some(17.0));
    } else {
      assert_eq!(
        function.get_name(scope).to_rust_string_lossy(scope),
        "namedCallback"
      );
    }
  }
}

#[test]
fn identifies_external_values_by_brand_not_pointer_contents() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let mut payload = 42_u8;
  for pointer in [std::ptr::null_mut(), (&mut payload as *mut u8).cast()] {
    let external = v8::External::new(scope, pointer);
    let value: v8::Local<v8::Value> = external.into();
    assert!(value.is_external());
    assert_eq!(
      v8::Local::<v8::External>::try_from(value).unwrap().value(),
      pointer
    );
  }
  let values: [v8::Local<v8::Value>; 6] = [
    v8::undefined(scope).into(),
    v8::null(scope).into(),
    v8::Boolean::new(scope, false).into(),
    v8::Number::new(scope, 42.0).into(),
    v8::String::new(scope, "pointer").unwrap().into(),
    v8::Object::new(scope).into(),
  ];
  for value in values {
    assert!(!value.is_external());
  }
}

#[test]
#[ignore = "requires independently compiled linked callback fixtures"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn retains_and_invokes_foreign_callback_after_replacing_its_global() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let directory = PathBuf::from(
    std::env::var_os("V8X_JS2WASM_LINKED_CALLBACK_DIR")
      .expect("linked callback fixtures"),
  );
  v8::js2wasm_attach_realm_for_test(&context, &directory.join("context.wasm"))
    .unwrap();
  v8::js2wasm_attach_graph_for_test(&context, &directory.join("producer.wasm"))
    .unwrap();
  let global = context.global(scope);
  let key = v8::String::new(scope, "deferredCallback").unwrap();
  let function =
    v8::Local::<v8::Function>::try_from(global.get(scope, key.into()).unwrap())
      .unwrap();
  let saved = v8::Global::new(scope, function);
  let two = v8::Number::new(scope, 2.0);
  assert_eq!(
    function
      .call(scope, global.into(), &[two.into()])
      .unwrap()
      .number_value(scope),
    Some(42.0)
  );
  v8::js2wasm_attach_graph_for_test(
    &context,
    &directory.join("replacement.wasm"),
  )
  .unwrap();
  let replacement =
    v8::Local::<v8::Function>::try_from(global.get(scope, key.into()).unwrap())
      .unwrap();
  for name in [
    "linkedSymbolEqual",
    "linkedFreshDistinct",
    "linkedSymbolKey",
    "linkedSymbolDescription",
  ] {
    let key = v8::String::new(scope, name).unwrap();
    let value = global.get(scope, key.into()).unwrap();
    assert!(value.is_boolean(), "{name} must be a real Boolean result");
    assert!(value.is_true(), "{name}");
  }
  let registered_key = v8::String::new(scope, "linked-key").unwrap();
  let registered = v8::Symbol::for_key(scope, registered_key);
  let field = v8::String::new(scope, "producerRegistered").unwrap();
  assert!(
    global
      .get(scope, field.into())
      .unwrap()
      .strict_equals(registered.into())
  );
  let retained = v8::Local::new(scope, &saved);
  assert!(!retained.strict_equals(replacement.into()));
  let three = v8::Number::new(scope, 3.0);
  assert_eq!(
    retained
      .call(scope, global.into(), &[three.into()])
      .unwrap()
      .number_value(scope),
    Some(45.0)
  );
  assert_eq!(
    replacement
      .call(scope, global.into(), &[])
      .unwrap()
      .number_value(scope),
    Some(99.0)
  );
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn native_errors_expose_realm_constructor_and_preserve_identity() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("context fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let message = v8::String::new(scope, "native failure").unwrap();
  let error = v8::Exception::error(scope, message);
  let object = v8::Local::<v8::Object>::try_from(error).unwrap();
  let constructor_key = v8::String::new(scope, "constructor").unwrap();
  let constructor = object.get(scope, constructor_key.into()).unwrap();
  assert!(
    constructor.is_function(),
    "native error must expose the realm constructor"
  );
  let message_key = v8::String::new(scope, "message").unwrap();
  assert_eq!(
    object
      .get(scope, message_key.into())
      .unwrap()
      .to_string(scope)
      .unwrap()
      .to_rust_string_lossy(scope),
    "native failure"
  );
  let global = context.global(scope);
  let identity_key = v8::String::new(scope, "identity").unwrap();
  let identity = v8::Local::<v8::Function>::try_from(
    global.get(scope, identity_key.into()).unwrap(),
  )
  .unwrap();
  assert!(
    identity
      .call(scope, global.into(), &[error])
      .unwrap()
      .strict_equals(error)
  );
  let prototype = object.get_prototype(scope).unwrap();
  assert!(prototype.is_object());
  assert!(
    object
      .get_prototype(scope)
      .unwrap()
      .strict_equals(prototype)
  );
  // Deno reads constructor.name at every level while formatting Errors.
  let mut current = prototype;
  let mut depth = 0;
  while current.is_object() {
    let current_object = v8::Local::<v8::Object>::try_from(current).unwrap();
    let ctor = current_object.get(scope, constructor_key.into()).unwrap();
    assert!(
      ctor.is_function(),
      "prototype at depth {depth} has no constructor"
    );
    current = current_object.get_prototype(scope).unwrap();
    depth += 1;
    assert!(
      depth <= 2,
      "cyclic or unexpected native Error prototype chain"
    );
  }
  assert_eq!(depth, 2);
  assert!(current.is_null());
}

#[test]
#[ignore = "requires compiled context value bridge fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn native_errors_accept_registered_symbol_property_keys() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("context fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let message = v8::String::new(scope, "symbol key").unwrap();
  let error = v8::Exception::error(scope, message);
  let object = v8::Local::<v8::Object>::try_from(error).unwrap();
  let name = v8::String::new(scope, "errorAdditionalPropertyKeys").unwrap();
  let symbol = v8::Symbol::for_key(scope, name);
  let absent = object
    .get(scope, symbol.into())
    .expect("missing symbol is undefined, not a conversion error");
  assert!(absent.is_undefined());
  let number = v8::Number::new(scope, 42.0);
  assert_eq!(object.set(scope, symbol.into(), number.into()), Some(true));
  let same_symbol = v8::Symbol::for_key(scope, name);
  assert_eq!(
    object
      .get(scope, same_symbol.into())
      .unwrap()
      .number_value(scope),
    Some(42.0)
  );
}

#[test]
#[ignore = "requires compiled Symbol context fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn symbols_preserve_registry_freshness_and_descriptions_across_realms() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("context fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  let global = context.global(scope);
  macro_rules! get {
    ($name:expr) => {{
      let key = v8::String::new(scope, $name).unwrap();
      global.get(scope, key.into()).unwrap()
    }};
  }
  let registered = get!("registeredSymbol");
  let key = v8::String::new(scope, "errorAdditionalPropertyKeys").unwrap();
  let native = v8::Symbol::for_key(scope, key);
  assert!(registered.strict_equals(native.into()));
  assert!(registered.strict_equals(get!("registeredSymbol")));
  let first = get!("freshSymbolA");
  let second = get!("freshSymbolB");
  assert!(first.is_symbol() && second.is_symbol());
  assert!(!first.strict_equals(second));
  assert!(first.strict_equals(get!("freshSymbolA")));
  let absent = v8::Local::<v8::Symbol>::try_from(get!("absentSymbol")).unwrap();
  assert!(absent.description(scope).is_undefined());
  let empty = v8::Local::<v8::Symbol>::try_from(get!("emptySymbol")).unwrap();
  assert_eq!(
    empty
      .description(scope)
      .to_string(scope)
      .unwrap()
      .to_rust_string_lossy(scope),
    ""
  );
  assert!(!absent.strict_equals(empty.into()));
  let iterator = get!("iteratorSymbol");
  let native_iterator = v8::Symbol::get_iterator(scope);
  assert!(iterator.strict_equals(native_iterator.into()));
  let identity = v8::Local::<v8::Function>::try_from(get!("identity")).unwrap();
  let description = v8::String::new(scope, "same").unwrap();
  let native_fresh = v8::Symbol::new(scope, Some(description));
  let returned = identity
    .call(scope, global.into(), &[native_fresh.into()])
    .unwrap();
  assert!(returned.strict_equals(native_fresh.into()));
  assert!(!returned.strict_equals(first));
  let error_message = v8::String::new(scope, "properties").unwrap();
  let error = v8::Exception::error(scope, error_message);
  let object = v8::Local::<v8::Object>::try_from(error).unwrap();
  let number = v8::Number::new(scope, 73.0);
  assert_eq!(object.set(scope, native.into(), number.into()), Some(true));
  let read =
    v8::Local::<v8::Function>::try_from(get!("readErrorSymbol")).unwrap();
  assert_eq!(
    read
      .call(scope, global.into(), &[error])
      .unwrap()
      .number_value(scope),
    Some(73.0)
  );
}

#[test]
#[ignore = "requires independently compiled linked callback fixtures"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn rejects_linked_symbol_state_when_context_exports_are_missing() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let directory =
    PathBuf::from(std::env::var_os("V8X_JS2WASM_LINKED_CALLBACK_DIR").unwrap());
  v8::js2wasm_attach_realm_for_test(
    &context,
    &directory.join("unshared-context.wasm"),
  )
  .unwrap();
  let error = v8::js2wasm_attach_graph_for_test(
    &context,
    &directory.join("producer.wasm"),
  )
  .unwrap_err();
  assert!(
    error.contains("missing context export __symbol_counter"),
    "{error}"
  );
  let key = v8::String::new(scope, "producerRegistered").unwrap();
  assert!(
    context
      .global(scope)
      .get(scope, key.into())
      .unwrap()
      .is_undefined()
  );
}

#[test]
fn boolean_value_and_to_boolean_follow_native_truthiness() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let cases: Vec<(v8::Local<v8::Value>, bool)> = vec![
    (v8::undefined(scope).into(), false),
    (v8::null(scope).into(), false),
    (v8::Boolean::new(scope, false).into(), false),
    (v8::Boolean::new(scope, true).into(), true),
    (v8::Number::new(scope, 0.0).into(), false),
    (v8::Number::new(scope, -0.0).into(), false),
    (v8::Number::new(scope, f64::NAN).into(), false),
    (v8::Number::new(scope, -1.0).into(), true),
    (v8::Number::new(scope, f64::MIN_POSITIVE).into(), true),
    (v8::Number::new(scope, f64::INFINITY).into(), true),
    (v8::Number::new(scope, f64::NEG_INFINITY).into(), true),
    (v8::BigInt::new_from_i64(scope, 0).into(), false),
    (v8::BigInt::new_from_i64(scope, 1).into(), true),
    (v8::BigInt::new_from_i64(scope, -1).into(), true),
    (v8::String::new(scope, "").unwrap().into(), false),
    (v8::String::new(scope, "0").unwrap().into(), true),
    (v8::String::new(scope, "false").unwrap().into(), true),
    (v8::String::new(scope, "\0").unwrap().into(), true),
    (v8::Object::new(scope).into(), true),
    (v8::Array::new(scope, 0).into(), true),
    (v8::Symbol::new(scope, None).into(), true),
    (v8::External::new(scope, std::ptr::null_mut()).into(), true),
  ];
  for (index, (value, expected)) in cases.into_iter().enumerate() {
    assert_eq!(
      value.boolean_value(scope),
      expected,
      "BooleanValue case {index}"
    );
    let boolean = value.to_boolean(scope);
    assert!(boolean.is_boolean());
    assert_eq!(boolean.is_true(), expected, "ToBoolean case {index}");
  }
}
