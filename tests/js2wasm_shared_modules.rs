// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.
use super::*;

struct SharedModule(v8::Global<v8::Module>);

struct CachedFailure {
  payload: v8::Global<v8::Value>,
  calls: Cell<u32>,
}

struct FailureGraphModules(Vec<(String, v8::Global<v8::Module>)>);

#[test]
#[ignore = "requires trusted Context and source-bound failure lifecycle graph"]
fn aot_first_dependency_failure_preserves_execution_states() {
  fn resolve<'s>(
    context: v8::Local<'s, v8::Context>,
    specifier: v8::Local<'s, v8::String>,
    _attributes: v8::Local<'s, v8::FixedArray>,
    _referrer: v8::Local<'s, v8::Module>,
  ) -> Option<v8::Local<'s, v8::Module>> {
    v8::callback_scope!(unsafe scope, context);
    let request = specifier.to_rust_string_lossy(scope);
    let modules = context.get_slot::<FailureGraphModules>()?;
    let (_, module) = modules.0.iter().find(|(name, _)| name == &request)?;
    Some(v8::Local::new(scope, module))
  }
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
  let mut modules = Vec::new();
  for (name, text) in [
    (
      "prefix",
      include_str!("fixtures/js2wasm-failed-module/prefix.js"),
    ),
    (
      "shared",
      include_str!("fixtures/js2wasm-failed-module/shared.js"),
    ),
    (
      "later",
      include_str!("fixtures/js2wasm-failed-module/later.js"),
    ),
  ] {
    let module =
      compile(scope, &format!("file:///failed-module/{name}.js"), text);
    modules.push((format!("./{name}.js"), v8::Global::new(scope, module)));
  }
  context.set_slot(Rc::new(FailureGraphModules(modules)));
  let entry = compile(
    scope,
    "file:///failed-module/entry.js",
    include_str!("fixtures/js2wasm-failed-module/entry.js"),
  );
  assert_eq!(entry.instantiate_module(scope, resolve), Some(true));
  v8::tc_scope!(let caught, scope);
  let result = entry.evaluate(caught).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Rejected);
  let modules = context.get_slot::<FailureGraphModules>().unwrap();
  for ((name, module), expected) in modules.0.iter().zip([
    v8::ModuleStatus::Evaluated,
    v8::ModuleStatus::Errored,
    v8::ModuleStatus::Instantiated,
  ]) {
    let module = v8::Local::new(caught, module);
    assert_eq!(module.get_status(), expected, "{name} execution state");
  }
  let global = context.global(caught);
  let key = v8::String::new(caught, "moduleThrownToken").unwrap();
  let token = global.get(caught, key.into()).unwrap();
  assert!(token.is_object(), "failure fixture must actually execute");
  assert!(promise.result(caught).strict_equals(token));
  assert!(entry.get_exception().strict_equals(token));
  assert!(!caught.has_caught());
  promise.mark_as_handled();
  let key = v8::String::new(caught, "prefixRuns").unwrap();
  assert_eq!(
    global.get(caught, key.into()).unwrap().number_value(caught),
    Some(1.0)
  );
  let key = v8::String::new(caught, "laterRuns").unwrap();
  assert!(global.get(caught, key.into()).unwrap().is_undefined());
  let modules = context.get_slot::<FailureGraphModules>().unwrap();
  for ((name, module), expected) in modules.0.iter().zip([
    v8::ModuleStatus::Evaluated,
    v8::ModuleStatus::Errored,
    v8::ModuleStatus::Instantiated,
  ]) {
    let module = v8::Local::new(caught, module);
    assert_eq!(module.get_status(), expected, "{name} execution state");
    if expected == v8::ModuleStatus::Errored {
      assert!(module.get_exception().strict_equals(token));
    }
  }
  assert!(entry.evaluate(caught).unwrap().strict_equals(result));
  let stats = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(stats.compilations, 0);
  assert_eq!(stats.runtime_eval_instantiations, 0);
}

#[test]
fn cached_dependency_failure_rejects_with_original_payload_without_reexecution()
{
  fn fail<'s>(
    context: v8::Local<'s, v8::Context>,
    _module: v8::Local<'s, v8::Module>,
  ) -> Option<v8::Local<'s, v8::Value>> {
    v8::callback_scope!(unsafe scope, context);
    let failure = context.get_slot::<CachedFailure>().unwrap().clone();
    failure.calls.set(failure.calls.get() + 1);
    let payload = v8::Local::new(scope, &failure.payload);
    scope.throw_exception(payload);
    None
  }
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let payload: v8::Local<v8::Value> = v8::Object::new(scope).into();
  let failure = Rc::new(CachedFailure {
    payload: v8::Global::new(scope, payload),
    calls: Cell::new(0),
  });
  context.set_slot(failure.clone());
  let name =
    v8::String::new(scope, "file:///cached-failure/shared.js").unwrap();
  let shared = v8::Module::create_synthetic_module(scope, name, &[], fail);
  context.set_slot(Rc::new(SharedModule(v8::Global::new(scope, shared))));
  assert_eq!(shared.instantiate_module(scope, resolve_shared), Some(true));
  let rejected = shared.evaluate(scope).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(rejected).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Rejected);
  assert!(promise.result(scope).strict_equals(payload));
  promise.mark_as_handled();
  assert!(shared.evaluate(scope).unwrap().strict_equals(rejected));
  for index in 0..2 {
    let entry = compile(
      scope,
      &format!("file:///cached-failure/entry-{index}.js"),
      "import './shared.js'; throw new Error('entry must not execute');",
    );
    assert_eq!(entry.instantiate_module(scope, resolve_shared), Some(true));
    v8::tc_scope!(let caught, scope);
    let prior = if index == 1 {
      let value: v8::Local<v8::Value> = v8::Object::new(caught).into();
      caught.throw_exception(value);
      Some(value)
    } else {
      None
    };
    let result = entry.evaluate(caught).unwrap();
    let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
    assert_eq!(promise.state(), v8::PromiseState::Rejected);
    assert!(
      promise.result(caught).strict_equals(payload),
      "entry {index} must retain the cached dependency payload"
    );
    assert!(entry.get_exception().strict_equals(payload));
    if let Some(prior) = prior {
      assert!(
        caught.exception().unwrap().strict_equals(prior),
        "cached delivery must preserve an unrelated caught exception"
      );
      caught.reset();
    } else {
      assert!(
        !caught.has_caught(),
        "dependency failure is an asynchronous rejection"
      );
    }
    assert!(entry.evaluate(caught).unwrap().strict_equals(result));
    promise.mark_as_handled();
  }
  let middle = compile(
    scope,
    "file:///cached-failure/middle.js",
    "import './shared.js'; export const value=1;",
  );
  assert_eq!(middle.instantiate_module(scope, resolve_shared), Some(true));
  context.set_slot(Rc::new(SharedModule(v8::Global::new(scope, middle))));
  let leaf = compile(
    scope,
    "file:///cached-failure/leaf.js",
    "import './shared.js'; throw new Error('leaf must not execute');",
  );
  assert_eq!(leaf.instantiate_module(scope, resolve_shared), Some(true));
  v8::tc_scope!(let caught, scope);
  let result = leaf.evaluate(caught).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Rejected);
  assert!(promise.result(caught).strict_equals(payload));
  assert_eq!(middle.get_status(), v8::ModuleStatus::Errored);
  assert!(middle.get_exception().strict_equals(payload));
  let middle_result = middle.evaluate(caught).unwrap();
  let middle_promise =
    v8::Local::<v8::Promise>::try_from(middle_result).unwrap();
  assert_eq!(middle_promise.state(), v8::PromiseState::Rejected);
  assert!(middle_promise.result(caught).strict_equals(payload));
  assert!(
    middle
      .evaluate(caught)
      .unwrap()
      .strict_equals(middle_result)
  );
  assert!(!caught.has_caught());
  promise.mark_as_handled();
  middle_promise.mark_as_handled();
  assert_eq!(failure.calls.get(), 1);
  // A fresh synthetic dependency must execute before graph packaging. Its
  // first callback failure has the same semantics as a previously cached one.
  let name =
    v8::String::new(caught, "file:///cached-failure/fresh.js").unwrap();
  let fresh = v8::Module::create_synthetic_module(caught, name, &[], fail);
  context.set_slot(Rc::new(SharedModule(v8::Global::new(caught, fresh))));
  let consumer = compile(
    caught,
    "file:///cached-failure/fresh-consumer.js",
    "import './shared.js'; throw new Error('consumer must not execute');",
  );
  assert_eq!(
    consumer.instantiate_module(caught, resolve_shared),
    Some(true)
  );
  let result = consumer.evaluate(caught).unwrap();
  let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Rejected);
  assert!(promise.result(caught).strict_equals(payload));
  assert_eq!(fresh.get_status(), v8::ModuleStatus::Errored);
  assert!(fresh.get_exception().strict_equals(payload));
  assert!(consumer.get_exception().strict_equals(payload));
  assert!(consumer.evaluate(caught).unwrap().strict_equals(result));
  assert!(!caught.has_caught());
  assert_eq!(failure.calls.get(), 2);
  promise.mark_as_handled();
  let dependency_result = fresh.evaluate(caught).unwrap();
  let dependency_promise =
    v8::Local::<v8::Promise>::try_from(dependency_result).unwrap();
  assert!(dependency_promise.result(caught).strict_equals(payload));
  dependency_promise.mark_as_handled();
  assert_eq!(failure.calls.get(), 2);
  let stats = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(stats.compilations, 0);
  assert_eq!(stats.runtime_eval_instantiations, 0);
}

fn resolve_shared<'s>(
  context: v8::Local<'s, v8::Context>,
  specifier: v8::Local<'s, v8::String>,
  _attributes: v8::Local<'s, v8::FixedArray>,
  _referrer: v8::Local<'s, v8::Module>,
) -> Option<v8::Local<'s, v8::Module>> {
  v8::callback_scope!(unsafe scope, context);
  assert!(matches!(
    specifier.to_rust_string_lossy(scope).as_str(),
    "./shared.js" | "./shared.ts"
  ));
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
#[ignore = "requires trusted Context and source-bound typed module graphs"]
fn aot_typed_dependency_reads_original_numeric_export() {
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
    "file:///typed-module/shared.ts",
    include_str!("fixtures/js2wasm-typed-module/shared.ts"),
  );
  context.set_slot(Rc::new(SharedModule(v8::Global::new(scope, shared))));
  for name in ["first", "second"] {
    let entry = compile(
      scope,
      &format!("file:///typed-module/{name}.ts"),
      if name == "first" {
        include_str!("fixtures/js2wasm-typed-module/first.ts")
      } else {
        include_str!("fixtures/js2wasm-typed-module/second.ts")
      },
    );
    assert_eq!(entry.instantiate_module(scope, resolve_shared), Some(true));
    let result = entry.evaluate(scope).unwrap();
    let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
    assert_eq!(
      promise.state(),
      v8::PromiseState::Fulfilled,
      "{name} evaluation"
    );
    let shared_namespace =
      v8::Local::<v8::Object>::try_from(shared.get_module_namespace()).unwrap();
    let bump_key = v8::String::new(scope, "bump").unwrap();
    let bump = v8::Local::<v8::Function>::try_from(
      shared_namespace.get(scope, bump_key.into()).unwrap(),
    )
    .unwrap();
    if name == "first" {
      assert_eq!(
        bump
          .call(scope, shared_namespace.into(), &[])
          .unwrap()
          .number_value(scope),
        Some(78.0)
      );
    }
    if name == "second" {
      let namespace =
        v8::Local::<v8::Object>::try_from(entry.get_module_namespace())
          .unwrap();
      let key = v8::String::new(scope, "observed").unwrap();
      assert_eq!(
        namespace
          .get(scope, key.into())
          .unwrap()
          .number_value(scope),
        Some(81.0)
      );
      let key = v8::String::new(scope, "read").unwrap();
      let function = v8::Local::<v8::Function>::try_from(
        namespace.get(scope, key.into()).unwrap(),
      )
      .unwrap();
      assert_eq!(
        function
          .call(scope, namespace.into(), &[])
          .unwrap()
          .number_value(scope),
        Some(81.0)
      );
      assert_eq!(
        bump
          .call(scope, shared_namespace.into(), &[])
          .unwrap()
          .number_value(scope),
        Some(79.0)
      );
      assert_eq!(
        function
          .call(scope, namespace.into(), &[])
          .unwrap()
          .number_value(scope),
        Some(82.0)
      );
      let key = v8::String::new(scope, "observed").unwrap();
      assert_eq!(
        namespace
          .get(scope, key.into())
          .unwrap()
          .number_value(scope),
        Some(81.0)
      );
    }
  }
  let stats = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(stats.compilations, 0);
  assert_eq!(stats.runtime_eval_instantiations, 0);
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

  for (name, expected) in [
    ("optionalNamed", 66.0),
    ("optionalMethod", 67.0),
    ("optionalComputed", 68.0),
    ("optionalNamespace", 69.0),
    ("parenBreak", 1.0),
    ("nonCallable", 1.0),
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
  for name in [
    "absentCall",
    "absentReceiver",
    "chainSkip",
    "computedSkip",
    "nullSkip",
    "optionalNamedReceiver",
  ] {
    let key = v8::String::new(scope, name).unwrap();
    let function = v8::Local::<v8::Function>::try_from(
      second_namespace.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    assert!(
      function
        .call(scope, second_namespace.into(), &[])
        .unwrap()
        .is_undefined(),
      "{name}"
    );
  }
  for name in [
    "optionalReceiver",
    "parenthesizedReceiver",
    "nestedReceiver",
  ] {
    let key = v8::String::new(scope, name).unwrap();
    let function = v8::Local::<v8::Function>::try_from(
      second_namespace.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let expected = if name == "nestedReceiver" {
      let nested = v8::String::new(scope, "nested").unwrap();
      namespace.get(scope, nested.into()).unwrap()
    } else {
      namespace.into()
    };
    assert!(
      function
        .call(scope, second_namespace.into(), &[])
        .unwrap()
        .strict_equals(expected),
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
    Some(70.0)
  );
  assert_eq!(
    global.get(scope, runs.into()).unwrap().number_value(scope),
    Some(2.0)
  );
  let stats = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(stats.compilations, 0);
  assert_eq!(stats.runtime_eval_instantiations, 0);
}
