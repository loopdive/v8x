// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
use super::*;

struct SharedModule(v8::Global<v8::Module>);

fn resolve_shared<'s>(
  context: v8::Local<'s, v8::Context>,
  specifier: v8::Local<'s, v8::String>,
  _attributes: v8::Local<'s, v8::FixedArray>,
  _referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
  v8::callback_scope!(unsafe scope, context);
  assert_eq!(specifier.to_rust_string_lossy(scope), "./shared.js");
  Some(v8::Local::new(
    scope,
    &context.get_slot::<SharedModule>()?.0,
  ))
}

fn compile<'s>(
  scope: &mut v8::PinScope<'s, '_>,
  name: &str,
  text: &str,
) -> v8::Local<'s, v8::Module> {
  let text = v8::String::new(scope, text).unwrap();
  let resource = v8::String::new(scope, name).unwrap();
  let origin = origin(scope, resource.into());
  let mut source = v8::script_compiler::Source::new(text, Some(&origin));
  v8::script_compiler::compile_module(scope, &mut source).unwrap()
}

#[test]
#[ignore = "requires trusted Context and source-bound shared module graphs"]
fn aot_shared_dependency_keeps_namespace_live_exports_and_single_execution() {
  initialize();
  let path = std::env::var_os("V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR").unwrap();
  assert!(std::env::var_os("V8X_JS2WASM_AOT_GRAPH_DIR").is_some());
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  v8::js2wasm_attach_precompiled_realm_for_test(
    &context,
    &Path::new(&path).join("context.cwasm"),
  )
  .unwrap();
  let shared = compile(
    scope,
    "file:///shared-module/shared.js",
    include_str!("fixtures/js2wasm-shared-module/shared.js"),
  );
  context.set_slot(Rc::new(SharedModule(v8::Global::new(scope, shared))));
  let first = compile(
    scope,
    "file:///shared-module/first.js",
    include_str!("fixtures/js2wasm-shared-module/first.js"),
  );
  assert_eq!(first.instantiate_module(scope, resolve_shared), Some(true));
  let result = first.evaluate(scope).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Fulfilled);
  let namespace =
    v8::Local::<v8::Object>::try_from(shared.get_module_namespace()).unwrap();
  let first_namespace =
    v8::Local::<v8::Object>::try_from(first.get_module_namespace()).unwrap();
  let shared_key = v8::String::new(scope, "shared").unwrap();
  let first_identity = first_namespace
    .get(scope, shared_key.into())
    .unwrap()
    .strict_equals(namespace.into());
  eprintln!("first entry namespace matches native dependency={first_identity}");
  assert!(
    first_identity,
    "the first graph must already have one canonical namespace"
  );
  let count = v8::String::new(scope, "count").unwrap();
  assert_eq!(
    namespace
      .get(scope, count.into())
      .unwrap()
      .number_value(scope),
    Some(2.0)
  );

  let second = compile(
    scope,
    "file:///shared-module/second.js",
    include_str!("fixtures/js2wasm-shared-module/second.js"),
  );
  assert_eq!(second.instantiate_module(scope, resolve_shared), Some(true));
  let result = second.evaluate(scope).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
  // Report side effects even when namespace publication rejects evaluation.
  let global = context.global(scope);
  let runs = v8::String::new(scope, "sharedModuleRuns").unwrap();
  let executions = global.get(scope, runs.into()).unwrap().number_value(scope);
  let second_namespace =
    v8::Local::<v8::Object>::try_from(second.get_module_namespace()).unwrap();
  let observed_key = v8::String::new(scope, "observed").unwrap();
  let observed = second_namespace
    .get(scope, observed_key.into())
    .unwrap()
    .number_value(scope);
  let same = v8::String::new(scope, "same").unwrap();
  let same_identity =
    second_namespace.get(scope, same.into()).unwrap().is_true();
  eprintln!(
    "shared dependency executions={executions:?}, observed={observed:?}, same namespace={same_identity}, second state={:?}",
    promise.state()
  );
  let stats = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(stats.compilations, 0);
  assert_eq!(stats.runtime_eval_instantiations, 0);
  assert_eq!(
    executions,
    Some(1.0),
    "an evaluated dependency must not run again"
  );
  assert_eq!(promise.state(), v8::PromiseState::Fulfilled);
  for (key, expected) in [("observed", 2.0), ("runs", 1.0)] {
    let key = v8::String::new(scope, key).unwrap();
    assert_eq!(
      second_namespace
        .get(scope, key.into())
        .unwrap()
        .number_value(scope),
      Some(expected)
    );
  }
  assert!(same_identity);
  assert!(
    second_namespace
      .get(scope, shared_key.into())
      .unwrap()
      .strict_equals(namespace.into())
  );
  let bump_key = v8::String::new(scope, "bump").unwrap();
  let bump = v8::Local::<v8::Function>::try_from(
    namespace.get(scope, bump_key.into()).unwrap(),
  )
  .unwrap();
  assert_eq!(
    bump
      .call(scope, namespace.into(), &[])
      .unwrap()
      .number_value(scope),
    Some(3.0)
  );
  for name in ["read", "readNamed"] {
    let key = v8::String::new(scope, name).unwrap();
    let read = v8::Local::<v8::Function>::try_from(
      second_namespace.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    assert_eq!(
      read
        .call(scope, second_namespace.into(), &[])
        .unwrap()
        .number_value(scope),
      Some(3.0),
      "live reader {name} must use the original dependency binding"
    );
  }
  for (name, expected) in [("mutateNamespace", 4.0), ("mutateNamed", 5.0)] {
    let key = v8::String::new(scope, name).unwrap();
    let function = v8::Local::<v8::Function>::try_from(
      second_namespace.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    assert_eq!(
      function
        .call(scope, second_namespace.into(), &[])
        .unwrap()
        .number_value(scope),
      Some(expected),
      "{name}"
    );
  }
  for name in ["receiverNamespace", "receiverNamed"] {
    let key = v8::String::new(scope, name).unwrap();
    let function = v8::Local::<v8::Function>::try_from(
      second_namespace.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let result = function.call(scope, second_namespace.into(), &[]).unwrap();
    if name == "receiverNamespace" {
      assert!(result.strict_equals(namespace.into()));
    } else {
      assert!(result.is_undefined());
    }
  }

  for (name, expected) in [
    ("spreadNamespace", 10.0),
    ("spreadNamed", 15.0),
    ("spreadMixed", 20.0),
    ("spreadNested", 65.0),
    ("spreadInvalid", 1.0),
  ] {
    let key = v8::String::new(scope, name).unwrap();
    let function = v8::Local::<v8::Function>::try_from(
      second_namespace.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    assert_eq!(
      function
        .call(scope, second_namespace.into(), &[])
        .unwrap()
        .number_value(scope),
      Some(expected),
      "{name}"
    );
  }

  // A different Module with the same URL must not reuse the previous binding.
  let replacement = compile(
    scope,
    "file:///shared-module/shared.js",
    include_str!("fixtures/js2wasm-shared-module/shared.js"),
  );
  context.set_slot(Rc::new(SharedModule(v8::Global::new(scope, replacement))));
  let another = compile(
    scope,
    "file:///shared-module/first.js",
    include_str!("fixtures/js2wasm-shared-module/first.js"),
  );
  assert_eq!(
    another.instantiate_module(scope, resolve_shared),
    Some(true)
  );
  let result =
    v8::Local::<v8::Promise>::try_from(another.evaluate(scope).unwrap())
      .unwrap();
  assert_eq!(result.state(), v8::PromiseState::Fulfilled);
  let replacement_namespace =
    v8::Local::<v8::Object>::try_from(replacement.get_module_namespace())
      .unwrap();
  assert!(!replacement_namespace.strict_equals(namespace.into()));
  assert_eq!(
    replacement_namespace
      .get(scope, count.into())
      .unwrap()
      .number_value(scope),
    Some(2.0)
  );
  assert_eq!(
    namespace
      .get(scope, count.into())
      .unwrap()
      .number_value(scope),
    Some(65.0)
  );
  assert_eq!(
    global.get(scope, runs.into()).unwrap().number_value(scope),
    Some(2.0)
  );
  let stats = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(stats.compilations, 0);
  assert_eq!(stats.runtime_eval_instantiations, 0);
}
