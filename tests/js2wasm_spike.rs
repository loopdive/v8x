// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

use std::sync::Once;

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

#[test]
fn evaluates_raw_typescript_graph_through_wasmtime() {
  initialize();
  if cfg!(feature = "engine_js2wasm") {
    assert_eq!(v8::V8X_ENGINE, "js2wasm");
  }
  assert!(
    std::env::var_os("V8X_JS2WASM_COMPILER_SCRIPT").is_some()
      || std::env::var_os("V8X_JS2WASM_AOT_MODULE").is_some(),
    "set V8X_JS2WASM_COMPILER_SCRIPT to compile-graph.ts or V8X_JS2WASM_AOT_MODULE to a precompiled artifact"
  );

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
