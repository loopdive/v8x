// Copyright 2026 Loopdive GmbH. Licensed under Apache-2.0 WITH LLVM-exception.

use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
#[cfg(any(
  feature = "js2wasm_runtime_compile",
  not(feature = "js2wasm_deno_poc_replay")
))]
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

#[cfg(any(feature = "js2wasm_runtime_compile", feature = "js2wasm_deno_poc"))]
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

thread_local! {
  static PROMISE_REJECTION_EVENTS: RefCell<Vec<(v8::PromiseRejectEvent, usize, Option<usize>)>> = const { RefCell::new(Vec::new()) };
  static REJECTION_SECOND_REALM: RefCell<Option<(v8::Global<v8::Context>, v8::Global<v8::Function>)>> = const { RefCell::new(None) };
}

unsafe extern "C" fn reject_in_second_realm(message: v8::PromiseRejectMessage) {
  unsafe { record_promise_rejection(message) };
  let first =
    PROMISE_REJECTION_EVENTS.with(|events| events.borrow().len() == 1);
  if !first {
    return;
  }
  v8::callback_scope!(unsafe scope, &message);
  let (context, reject) = REJECTION_SECOND_REALM.with(|slot| {
    let slot = slot.borrow();
    let (context, reject) = slot.as_ref().expect("second realm installed");
    (
      v8::Local::new(scope, context),
      v8::Local::new(scope, reject),
    )
  });
  let scope = &mut v8::ContextScope::new(scope, context);
  let receiver = v8::undefined(scope).into();
  reject.call(scope, receiver, &[]).unwrap();
}

unsafe extern "C" fn record_promise_rejection(
  message: v8::PromiseRejectMessage,
) {
  v8::callback_scope!(unsafe scope, &message);
  let promise = message.get_promise();
  let value = message.get_value();
  // Exercise callback-scope entry and identity-bearing ABI getters, not just
  // a synthetic event number. Keep no locals alive beyond this callback.
  assert!(promise.is_promise());
  let _ = scope.get_continuation_preserved_embedder_data();
  PROMISE_REJECTION_EVENTS.with(|events| {
    events.borrow_mut().push((
      message.get_event(),
      &*promise as *const v8::Promise as usize,
      value.map(|value| &*value as *const v8::Value as usize),
    ))
  });
}

unsafe extern "C" fn handle_rejection_during_notification(
  message: v8::PromiseRejectMessage,
) {
  unsafe { record_promise_rejection(message) };
  if message.get_event() == v8::PromiseRejectEvent::PromiseRejectWithNoHandler {
    v8::callback_scope!(unsafe scope, &message);
    let handler = v8::Function::new_raw(scope, noop_callback).unwrap();
    message.get_promise().catch(scope, handler).unwrap();
  }
}

#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
fn compiled_rejection_delivery(reenter: bool) {
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  isolate.set_promise_reject_callback(if reenter {
    handle_rejection_during_notification
  } else {
    record_promise_rejection
  });
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
  let path = std::env::var_os("V8X_JS2WASM_REJECTION_CONTEXT")
    .expect("event-producing test context");
  if Path::new(&path)
    .extension()
    .is_some_and(|extension| extension == "cwasm")
  {
    v8::js2wasm_attach_precompiled_realm_for_test(&context, Path::new(&path))
      .unwrap();
  } else {
    #[cfg(feature = "js2wasm_runtime_compile")]
    v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
    #[cfg(not(feature = "js2wasm_runtime_compile"))]
    panic!("compiler-free rejection test requires a trusted .cwasm context");
  }
  let global = context.global(scope);
  let reason_key = v8::String::new(scope, "__v8x_test_reason").unwrap();
  let reason = global.get(scope, reason_key.into()).unwrap();
  let reject_key = v8::String::new(scope, "__v8x_test_reject").unwrap();
  let reject = v8::Local::<v8::Function>::try_from(
    global.get(scope, reject_key.into()).unwrap(),
  )
  .unwrap();
  let receiver = v8::undefined(scope).into();
  let promise = v8::Local::<v8::Promise>::try_from(
    reject.call(scope, receiver, &[]).unwrap(),
  )
  .unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Rejected);
  assert!(promise.result(scope).strict_equals(reason));
  assert_eq!(promise.has_handler(), reenter);
  let promise_id = &*promise as *const v8::Promise as usize;
  let reason_id = &*reason as *const v8::Value as usize;
  PROMISE_REJECTION_EVENTS.with(|events| {
    let events = events.borrow();
    assert_eq!(events.len(), if reenter { 2 } else { 1 });
    assert_eq!(
      events[0],
      (
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
        promise_id,
        Some(reason_id)
      )
    );
  });
  let attach_key = v8::String::new(scope, "__v8x_test_attach").unwrap();
  let attach = v8::Local::<v8::Function>::try_from(
    global.get(scope, attach_key.into()).unwrap(),
  )
  .unwrap();
  for _ in 0..2 {
    attach.call(scope, receiver, &[promise.into()]).unwrap();
  }
  assert!(promise.has_handler());
  scope.perform_microtask_checkpoint();
  PROMISE_REJECTION_EVENTS.with(|events| {
    assert_eq!(
      *events.borrow(),
      [
        (
          v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
          promise_id,
          Some(reason_id)
        ),
        (
          v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject,
          promise_id,
          None
        ),
      ]
    )
  });
}

#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
#[test]
#[ignore = "requires freshly built event-producing V8X_JS2WASM_REJECTION_CONTEXT"]
fn compiled_rejection_reports_exact_identity_and_late_handler() {
  compiled_rejection_delivery(false);
}

#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
#[test]
#[ignore = "requires freshly built event-producing V8X_JS2WASM_REJECTION_CONTEXT"]
fn compiled_rejection_callback_can_reenter_without_borrowing_runtime() {
  compiled_rejection_delivery(true);
}

#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
#[test]
#[ignore = "requires cross-realm event-producing V8X_JS2WASM_REJECTION_CONTEXT"]
fn compiled_rejection_preserves_cross_realm_enqueue_order() {
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  isolate.set_promise_reject_callback(reject_in_second_realm);
  v8::scope!(let scope, isolate);
  let first = v8::Context::new(scope, Default::default());
  let second = v8::Context::new(scope, Default::default());
  let path =
    PathBuf::from(std::env::var_os("V8X_JS2WASM_REJECTION_CONTEXT").unwrap());
  let second_reason_id;
  {
    let scope = &mut v8::ContextScope::new(scope, second);
    scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
    v8::js2wasm_attach_precompiled_realm_for_test(&second, &path).unwrap();
    let global = second.global(scope);
    let key = v8::String::new(scope, "__v8x_test_reject").unwrap();
    let reject = v8::Local::<v8::Function>::try_from(
      global.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let key = v8::String::new(scope, "__v8x_test_reason").unwrap();
    let reason = global.get(scope, key.into()).unwrap();
    second_reason_id = &*reason as *const v8::Value as usize;
    REJECTION_SECOND_REALM.with(|slot| {
      *slot.borrow_mut() = Some((
        v8::Global::new(scope, second),
        v8::Global::new(scope, reject),
      ))
    });
  }
  {
    let scope = &mut v8::ContextScope::new(scope, first);
    v8::js2wasm_attach_precompiled_realm_for_test(&first, &path).unwrap();
    let global = first.global(scope);
    let key = v8::String::new(scope, "__v8x_test_reason").unwrap();
    let reason = global.get(scope, key.into()).unwrap();
    let reason_id = &*reason as *const v8::Value as usize;
    assert_ne!(reason_id, second_reason_id);
    let key = v8::String::new(scope, "__v8x_test_reject_and_handle").unwrap();
    let reject = v8::Local::<v8::Function>::try_from(
      global.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let receiver = v8::undefined(scope).into();
    let promise = v8::Local::<v8::Promise>::try_from(
      reject.call(scope, receiver, &[]).unwrap(),
    )
    .unwrap();
    let promise_id = &*promise as *const v8::Promise as usize;
    PROMISE_REJECTION_EVENTS.with(|events| {
      let events = events.borrow();
      assert_eq!(events.len(), 3);
      assert_eq!(
        events[0],
        (
          v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
          promise_id,
          Some(reason_id)
        )
      );
      assert_eq!(
        events[1],
        (
          v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject,
          promise_id,
          None
        )
      );
      assert_eq!(
        events[2].0,
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler
      );
      assert_ne!(events[2].1, promise_id);
      assert_eq!(events[2].2, Some(second_reason_id));
    });
    scope.perform_microtask_checkpoint();
    PROMISE_REJECTION_EVENTS
      .with(|events| assert_eq!(events.borrow().len(), 3));
  }
  REJECTION_SECOND_REALM.with(|slot| slot.borrow_mut().take());
}

#[test]
fn native_promise_rejection_callback_can_reenter_and_attach_handler() {
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  isolate.set_promise_reject_callback(handle_rejection_during_notification);
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
  let resolver = v8::PromiseResolver::new(scope).unwrap();
  let promise = resolver.get_promise(scope);
  let reason = v8::Object::new(scope);
  resolver.reject(scope, reason.into()).unwrap();
  assert!(promise.has_handler());
  PROMISE_REJECTION_EVENTS.with(|events| {
    assert_eq!(
      events
        .borrow()
        .iter()
        .map(|event| event.0)
        .collect::<Vec<_>>(),
      [
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
        v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject
      ],
    )
  });
  scope.perform_microtask_checkpoint();
  PROMISE_REJECTION_EVENTS.with(|events| assert_eq!(events.borrow().len(), 2));
}

#[test]
fn native_promise_rejection_callback_registration_is_isolate_local() {
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  for registered in [true, false] {
    let isolate = &mut v8::Isolate::new(Default::default());
    if registered {
      isolate.set_promise_reject_callback(record_promise_rejection);
    }
    v8::scope!(let scope, isolate);
    let context = v8::Context::new(scope, Default::default());
    let scope = &mut v8::ContextScope::new(scope, context);
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let reason = v8::Object::new(scope);
    resolver.reject(scope, reason.into()).unwrap();
  }
  PROMISE_REJECTION_EVENTS.with(|events| assert_eq!(events.borrow().len(), 1));
}

#[test]
fn native_promise_rejection_reports_exact_identity_and_late_handler_once() {
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  isolate.set_promise_reject_callback(record_promise_rejection);
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
  let resolver = v8::PromiseResolver::new(scope).unwrap();
  let promise = resolver.get_promise(scope);
  let reason = v8::Object::new(scope);
  let identity = &*promise as *const v8::Promise as usize;
  let reason_identity = &*reason as *const v8::Object as usize;
  assert_eq!(resolver.reject(scope, reason.into()), Some(true));
  PROMISE_REJECTION_EVENTS.with(|events| {
    assert_eq!(
      &*events.borrow(),
      &[(
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
        identity,
        Some(reason_identity)
      ),]
    )
  });
  let handler = v8::Function::new_raw(scope, noop_callback).unwrap();
  let derived = promise.catch(scope, handler).unwrap();
  promise.catch(scope, handler).unwrap();
  PROMISE_REJECTION_EVENTS.with(|events| {
    assert_eq!(
      &*events.borrow(),
      &[
        (
          v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
          identity,
          Some(reason_identity)
        ),
        (
          v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject,
          identity,
          None
        ),
      ]
    )
  });
  assert_eq!(derived.state(), v8::PromiseState::Pending);
  scope.perform_microtask_checkpoint();
  assert_eq!(derived.state(), v8::PromiseState::Fulfilled);
  PROMISE_REJECTION_EVENTS.with(|events| assert_eq!(events.borrow().len(), 2));
}

#[test]
fn native_promise_rejection_respects_early_handlers_and_explicit_handling() {
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  isolate.set_promise_reject_callback(record_promise_rejection);
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
  let handler = v8::Function::new_raw(scope, noop_callback).unwrap();
  let value = v8::Number::new(scope, 1.0);
  for explicit in [false, true] {
    let resolver = v8::PromiseResolver::new(scope).unwrap();
    let promise = resolver.get_promise(scope);
    if explicit {
      promise.mark_as_handled();
    } else {
      promise.catch(scope, handler).unwrap();
    }
    resolver.reject(scope, value.into()).unwrap();
  }
  PROMISE_REJECTION_EVENTS.with(|events| assert!(events.borrow().is_empty()));
  scope.perform_microtask_checkpoint();
  PROMISE_REJECTION_EVENTS.with(|events| assert!(events.borrow().is_empty()));
}

#[test]
fn native_promise_duplicate_settlement_reports_attempt_without_changing_result()
{
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  isolate.set_promise_reject_callback(record_promise_rejection);
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let resolver = v8::PromiseResolver::new(scope).unwrap();
  let promise = resolver.get_promise(scope);
  let original = v8::Number::new(scope, 1.0);
  let attempted = v8::Object::new(scope);
  resolver.resolve(scope, original.into()).unwrap();
  resolver.reject(scope, attempted.into()).unwrap();
  resolver.resolve(scope, attempted.into()).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Fulfilled);
  assert!(promise.result(scope).strict_equals(original.into()));
  let identity = &*promise as *const v8::Promise as usize;
  let attempted_identity = &*attempted as *const v8::Object as usize;
  PROMISE_REJECTION_EVENTS.with(|events| {
    assert_eq!(
      &*events.borrow(),
      &[
        (
          v8::PromiseRejectEvent::PromiseRejectAfterResolved,
          identity,
          Some(attempted_identity)
        ),
        (
          v8::PromiseRejectEvent::PromiseResolveAfterResolved,
          identity,
          Some(attempted_identity)
        ),
      ]
    )
  });
}

#[test]
fn native_promise_rejection_propagation_reports_the_derived_promise() {
  initialize();
  PROMISE_REJECTION_EVENTS.with(|events| events.borrow_mut().clear());
  let isolate = &mut v8::Isolate::new(Default::default());
  isolate.set_promise_reject_callback(record_promise_rejection);
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  scope.set_microtasks_policy(v8::MicrotasksPolicy::Explicit);
  let resolver = v8::PromiseResolver::new(scope).unwrap();
  let promise = resolver.get_promise(scope);
  let handler = v8::Function::new_raw(scope, noop_callback).unwrap();
  // A fulfillment-only handler handles its receiver, but must propagate an
  // eventual rejection to its otherwise unhandled child at the checkpoint.
  let child = promise.then(scope, handler).unwrap();
  let reason = v8::Object::new(scope);
  resolver.reject(scope, reason.into()).unwrap();
  PROMISE_REJECTION_EVENTS.with(|events| assert!(events.borrow().is_empty()));
  scope.perform_microtask_checkpoint();
  assert_eq!(child.state(), v8::PromiseState::Rejected);
  assert!(child.result(scope).strict_equals(reason.into()));
  let identity = &*child as *const v8::Promise as usize;
  let reason_identity = &*reason as *const v8::Object as usize;
  PROMISE_REJECTION_EVENTS.with(|events| {
    assert_eq!(
      &*events.borrow(),
      &[(
        v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
        identity,
        Some(reason_identity)
      ),]
    )
  });
  child.catch(scope, handler).unwrap();
  scope.perform_microtask_checkpoint();
  PROMISE_REJECTION_EVENTS.with(|events| {
    assert_eq!(
      &*events.borrow(),
      &[
        (
          v8::PromiseRejectEvent::PromiseRejectWithNoHandler,
          identity,
          Some(reason_identity)
        ),
        (
          v8::PromiseRejectEvent::PromiseHandlerAddedAfterReject,
          identity,
          None
        ),
      ]
    )
  });
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn return_reaction_continuation(
  info: *const v8::FunctionCallbackInfo,
) {
  let parts = unsafe { &*info }.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let value = scope.get_continuation_preserved_embedder_data();
  let mut returned = parts.return_value;
  returned.set(value);
  let mutation = v8::Number::new(scope, 99.0);
  scope.set_continuation_preserved_embedder_data(mutation.into());
}

#[cfg(feature = "js2wasm_deno_poc")]
thread_local! {
  static DENO_PENDING_OP_IDS: std::cell::RefCell<Vec<(i32, Option<f64>)>> = const { std::cell::RefCell::new(Vec::new()) };
  static DENO_ASYNC_ARGUMENT_COUNTS: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
  static DENO_STRING_COERCION_EVENTS: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
  static DENO_REACTION_ORDER: std::cell::RefCell<Vec<f64>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn record_reaction_order(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  let order = args.data().number_value(scope).unwrap();
  DENO_REACTION_ORDER.with(|events| events.borrow_mut().push(order));
  let mut returned = parts.return_value;
  returned.set(args.get(0));
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_pending_probe_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let args = v8::FunctionCallbackArguments::from_function_callback_info(info);
  v8::callback_scope!(unsafe scope, info);
  let id = args.get(0).number_value(scope);
  DENO_PENDING_OP_IDS.with(|ids| ids.borrow_mut().push((args.length(), id)));
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_async_probe_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let args = v8::FunctionCallbackArguments::from_function_callback_info(info);
  let mut rv = v8::ReturnValue::from_function_callback_info(info);
  v8::callback_scope!(unsafe scope, info);
  assert_eq!(args.length(), 1, "Deno async stub supplies a promise id");
  assert!(args.get(0).is_number());
  if args.data().is_object() {
    rv.set(args.data());
  } else {
    let result = v8::Integer::new(scope, 42);
    rv.set(result.into());
  }
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_async_arguments_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let args = v8::FunctionCallbackArguments::from_function_callback_info(info);
  let mut rv = v8::ReturnValue::from_function_callback_info(info);
  v8::callback_scope!(unsafe scope, info);
  DENO_ASYNC_ARGUMENT_COUNTS
    .with(|counts| counts.borrow_mut().push(args.length()));
  assert!(
    args.get(0).is_number(),
    "promise id precedes user arguments"
  );
  assert!(args.this().strict_equals(args.data()));
  for index in 1..args.length() - 1 {
    assert_eq!(args.get(index).number_value(scope), Some(index as f64));
  }
  assert!(args.get(args.length() - 1).strict_equals(args.data()));
  rv.set(args.data());
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_string_coercion_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let args = v8::FunctionCallbackArguments::from_function_callback_info(info);
  v8::callback_scope!(unsafe scope, info);
  assert_eq!(args.length(), 0);
  assert!(args.this().strict_equals(args.data()));
  DENO_STRING_COERCION_EVENTS.with(|events| events.borrow_mut().push("string"));
  let result = v8::String::new(scope, "native string coercion").unwrap();
  v8::ReturnValue::from_function_callback_info(info).set(result.into());
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_string_coercion_throw(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let args = v8::FunctionCallbackArguments::from_function_callback_info(info);
  v8::callback_scope!(unsafe scope, info);
  DENO_STRING_COERCION_EVENTS.with(|events| events.borrow_mut().push("throw"));
  scope.throw_exception(args.data());
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_op_extras_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  DENO_STARTUP_OP_EVENTS.with(|events| events.borrow_mut().push("extras"));
  let parts = unsafe { &*info }.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let context = scope.get_current_context();
  let extras = context.get_extras_binding_object(scope);
  let mut returned = parts.return_value;
  returned.set(extras.into());
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_op_return_data_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  DENO_STARTUP_OP_EVENTS.with(|events| events.borrow_mut().push("import-meta"));
  let info = unsafe { &*info };
  let parts = info.get_parts();
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  let mut returned = parts.return_value;
  returned.set(args.data());
}

#[cfg(feature = "js2wasm_deno_poc")]
unsafe extern "C" fn deno_op_capture_bootstrap_callback(
  info: *const v8::FunctionCallbackInfo,
) {
  DENO_STARTUP_OP_EVENTS
    .with(|events| events.borrow_mut().push("capture-bootstrap"));
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  assert_eq!(args.length(), 1);
  assert!(args.get(0).is_object());
  let global = scope.get_current_context().global(scope);
  let key = v8::String::new(scope, "__capturedBootstrap").unwrap();
  assert_eq!(global.set(scope, key.into(), args.get(0)), Some(true));
}

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
  static ORDERED_MICROTASK_EVENTS: RefCell<Vec<i32>> = const { RefCell::new(Vec::new()) };
}

#[cfg(any(feature = "js2wasm_runtime_compile", feature = "js2wasm_deno_poc"))]
thread_local! {
  static DENO_OP_EVENTS: RefCell<Vec<DenoOpEvent>> = const { RefCell::new(Vec::new()) };
}

#[cfg(feature = "js2wasm_deno_poc")]
thread_local! {
  static DENO_STARTUP_OP_EVENTS: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
}

#[cfg(any(feature = "js2wasm_runtime_compile", feature = "js2wasm_deno_poc"))]
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
  // The unchanged JS print wrapper forwards an omitted isErr as undefined.
  // Deno's boolean op argument uses JS truthiness, not a Boolean-only carrier.
  let is_error = is_error.boolean_value(scope);
  let data = unsafe {
    v8::Local::<v8::External>::cast_unchecked(args.data()).value() as usize
  };
  DENO_OP_EVENTS.with(|events| {
    events.borrow_mut().push(DenoOpEvent::Print {
      message: message.to_rust_string_lossy(scope),
      is_error,
      data,
    });
  });
}

#[cfg(any(feature = "js2wasm_runtime_compile", feature = "js2wasm_deno_poc"))]
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

#[cfg(any(feature = "js2wasm_runtime_compile", feature = "js2wasm_deno_poc"))]
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
  let core_source;
  let source = match specifier.as_str() {
    DEPENDENCY => {
      "export function add(left: number, right: number): number { return left + right; }"
    }
    DENO => DENO_SOURCE,
    "ext:core/mod.js" => {
      let fixtures = std::env::var_os("V8X_JS2WASM_DENO_CORE_FIXTURES")
        .expect("pinned core fixtures for application import");
      core_source = fs::read_to_string(Path::new(&fixtures).join("mod.js"))
        .expect("unchanged core module fixture");
      &core_source
    }
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
  let replacement = v8::Number::new(scope, 17.0);
  assert_eq!(
    namespace.set(scope, op_name.into(), replacement.into()),
    Some(false)
  );
  assert!(
    namespace
      .get(scope, op_name.into())
      .unwrap()
      .strict_equals(op_value)
  );

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

#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
#[test]
#[ignore = "requires trusted context and source-bound graph artifacts, or build-time compilation"]
fn source_namespace_exposes_live_values_and_callable_identity() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("context-value bridge artifact");
  if Path::new(&path)
    .extension()
    .is_some_and(|value| value == "cwasm")
  {
    v8::js2wasm_attach_precompiled_realm_for_test(&context, Path::new(&path))
      .unwrap();
  } else {
    #[cfg(feature = "js2wasm_runtime_compile")]
    v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
    #[cfg(not(feature = "js2wasm_runtime_compile"))]
    panic!("compiler-free namespace test requires a trusted .cwasm context");
  }
  let before = v8::js2wasm_runtime_stats().unwrap();
  let text = v8::String::new(scope,
    "export let value=41; export function bump(){value++;} export const marker=Object.freeze({token:17}); export function fail(){throw marker;} export const proto={inherited:7}; export const otherProto={inherited:8}; const box={answer:42,set blocked(value){throw marker;}}; Object.setPrototypeOf(box,proto); export default box; export const settled=Promise.resolve(42); let resolvePending; export const pending=new Promise(resolve=>{resolvePending=resolve;}); export function settle(){resolvePending(42);}").unwrap();
  let resource = v8::String::new(scope, "ext:namespace/live.js").unwrap();
  let script_origin = origin(scope, resource.into());
  let mut source = v8::script_compiler::Source::new(text, Some(&script_origin));
  let module = v8::script_compiler::compile_module(scope, &mut source).unwrap();
  assert!(
    module
      .instantiate_module(scope, resolve_dependency)
      .unwrap()
  );
  let before_namespace = module.get_module_namespace();
  let result = module.evaluate(scope).expect("module evaluation");
  let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
  assert_eq!(promise.state(), v8::PromiseState::Fulfilled);
  let namespace = module.get_module_namespace();
  assert!(std::ptr::eq(&*before_namespace, &*namespace));
  assert!(namespace.is_module_namespace_object());
  let namespace = v8::Local::<v8::Object>::try_from(namespace).unwrap();
  let value_key = v8::String::new(scope, "value").unwrap();
  assert_eq!(
    namespace
      .get(scope, value_key.into())
      .unwrap()
      .number_value(scope),
    Some(41.0)
  );
  let bump_key = v8::String::new(scope, "bump").unwrap();
  let bump = namespace.get(scope, bump_key.into()).unwrap();
  let again = namespace.get(scope, bump_key.into()).unwrap();
  assert!(std::ptr::eq(&*bump, &*again));
  let bump = v8::Local::<v8::Function>::try_from(bump).unwrap();
  let undefined = v8::undefined(scope);
  let completion = bump.call(scope, undefined.into(), &[]).unwrap();
  assert!(
    completion.is_undefined(),
    "void completion is a successful call"
  );
  assert_eq!(
    namespace
      .get(scope, value_key.into())
      .unwrap()
      .number_value(scope),
    Some(42.0)
  );
  let replacement = v8::Number::new(scope, 0.0);
  assert_eq!(
    namespace.set(scope, value_key.into(), replacement.into()),
    Some(false)
  );
  assert_eq!(
    namespace
      .get(scope, value_key.into())
      .unwrap()
      .number_value(scope),
    Some(42.0)
  );
  let default_key = v8::String::new(scope, "default").unwrap();
  let default = namespace.get(scope, default_key.into()).unwrap();
  let default = v8::Local::<v8::Object>::try_from(default).unwrap();
  let answer_key = v8::String::new(scope, "answer").unwrap();
  assert_eq!(
    default
      .get(scope, answer_key.into())
      .unwrap()
      .number_value(scope),
    Some(42.0)
  );
  let assigned = v8::Number::new(scope, 43.0);
  assert_eq!(
    default.set(scope, answer_key.into(), assigned.into()),
    Some(true)
  );
  assert_eq!(
    default
      .get(scope, answer_key.into())
      .unwrap()
      .number_value(scope),
    Some(43.0)
  );
  let proto_key = v8::String::new(scope, "proto").unwrap();
  let proto = namespace.get(scope, proto_key.into()).unwrap();
  assert!(
    default.get_prototype(scope).unwrap().strict_equals(proto),
    "graph prototype identity"
  );
  let inherited_key = v8::String::new(scope, "inherited").unwrap();
  assert_eq!(
    default
      .get(scope, inherited_key.into())
      .unwrap()
      .number_value(scope),
    Some(7.0)
  );
  let other_key = v8::String::new(scope, "otherProto").unwrap();
  let other = namespace.get(scope, other_key.into()).unwrap();
  assert_eq!(default.set_prototype(scope, other), Some(true));
  assert!(default.get_prototype(scope).unwrap().strict_equals(other));
  assert_eq!(
    default
      .get(scope, inherited_key.into())
      .unwrap()
      .number_value(scope),
    Some(8.0)
  );
  let other_object = v8::Local::<v8::Object>::try_from(other).unwrap();
  let updated = v8::Number::new(scope, 9.0);
  assert_eq!(
    other_object.set(scope, inherited_key.into(), updated.into()),
    Some(true)
  );
  assert_eq!(
    default
      .get(scope, inherited_key.into())
      .unwrap()
      .number_value(scope),
    Some(9.0),
    "live prototype fields are not copied"
  );
  assert_eq!(
    other_object.set_prototype(scope, default.into()),
    Some(false),
    "prototype cycle refusal"
  );
  let marker_key = v8::String::new(scope, "marker").unwrap();
  for name in ["settled", "pending"] {
    let key = v8::String::new(scope, name).unwrap();
    let promise = v8::Local::<v8::Promise>::try_from(
      namespace.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let handler = v8::Function::builder_raw(realm_host_leaf)
      .build(scope)
      .unwrap();
    let derived = promise
      .then(scope, handler)
      .expect("reaction on separately compiled graph Promise");
    assert_eq!(derived.state(), v8::PromiseState::Pending);
    if name == "pending" {
      let key = v8::String::new(scope, "settle").unwrap();
      let settle = v8::Local::<v8::Function>::try_from(
        namespace.get(scope, key.into()).unwrap(),
      )
      .unwrap();
      assert!(
        settle
          .call(scope, undefined.into(), &[])
          .unwrap()
          .is_undefined()
      );
      assert_eq!(derived.state(), v8::PromiseState::Pending);
    }
    scope.perform_microtask_checkpoint();
    assert_eq!(derived.state(), v8::PromiseState::Fulfilled);
    assert_eq!(derived.result(scope).number_value(scope), Some(43.0));
    let twice = derived.then(scope, handler).unwrap();
    let thrower = v8::Function::builder_raw(realm_host_throw)
      .build(scope)
      .unwrap();
    let rejected = twice.then(scope, thrower).unwrap();
    let recover = v8::Function::builder_raw(noop_callback)
      .build(scope)
      .unwrap();
    let recovered = rejected.catch(scope, recover).unwrap();
    assert_eq!(twice.state(), v8::PromiseState::Pending);
    assert_eq!(recovered.state(), v8::PromiseState::Pending);
    scope.perform_microtask_checkpoint();
    assert_eq!(twice.state(), v8::PromiseState::Fulfilled);
    assert_eq!(twice.result(scope).number_value(scope), Some(44.0));
    assert_eq!(rejected.state(), v8::PromiseState::Rejected);
    assert_eq!(recovered.state(), v8::PromiseState::Fulfilled);
    assert!(recovered.result(scope).is_undefined());
  }
  // Two graph-owned Promise queues plus native work must share enqueue order.
  let second_text =
    v8::String::new(scope, "export const settled=Promise.resolve(42);")
      .unwrap();
  let second_resource =
    v8::String::new(scope, "ext:namespace/second.js").unwrap();
  let second_origin = origin(scope, second_resource.into());
  let mut second_source =
    v8::script_compiler::Source::new(second_text, Some(&second_origin));
  let second_module =
    v8::script_compiler::compile_module(scope, &mut second_source).unwrap();
  assert!(
    second_module
      .instantiate_module(scope, resolve_dependency)
      .unwrap()
  );
  let second_completion = second_module.evaluate(scope).unwrap();
  assert_eq!(
    v8::Local::<v8::Promise>::try_from(second_completion)
      .unwrap()
      .state(),
    v8::PromiseState::Fulfilled
  );
  let second_namespace =
    v8::Local::<v8::Object>::try_from(second_module.get_module_namespace())
      .unwrap();
  ORDERED_MICROTASK_EVENTS.with(|events| events.borrow_mut().clear());
  let settled_key = v8::String::new(scope, "settled").unwrap();
  for (graph, marker) in [(namespace, 1), (second_namespace, 2)] {
    let promise = v8::Local::<v8::Promise>::try_from(
      graph.get(scope, settled_key.into()).unwrap(),
    )
    .unwrap();
    let data = v8::Integer::new(scope, marker);
    let callback = v8::Function::builder_raw(record_ordered_microtask)
      .data(data.into())
      .build(scope)
      .unwrap();
    let chained = promise.then(scope, callback).unwrap();
    let data = v8::Integer::new(scope, marker + 10);
    let callback = v8::Function::builder_raw(record_ordered_microtask)
      .data(data.into())
      .build(scope)
      .unwrap();
    assert_eq!(
      chained.then(scope, callback).unwrap().state(),
      v8::PromiseState::Pending
    );
  }
  let data = v8::Integer::new(scope, 3);
  let native = v8::Function::builder_raw(record_ordered_microtask)
    .data(data.into())
    .build(scope)
    .unwrap();
  scope.enqueue_microtask(native);
  assert!(ORDERED_MICROTASK_EVENTS.with(|events| events.borrow().is_empty()));
  scope.perform_microtask_checkpoint();
  assert_eq!(
    ORDERED_MICROTASK_EVENTS.with(|events| events.borrow().clone()),
    [1, 2, 3, 11, 12]
  );
  scope.perform_microtask_checkpoint();
  assert_eq!(
    ORDERED_MICROTASK_EVENTS.with(|events| events.borrow().clone()),
    [1, 2, 3, 11, 12]
  );

  let marker = namespace.get(scope, marker_key.into()).unwrap();
  let frozen = v8::Local::<v8::Object>::try_from(marker).unwrap();
  let token_key = v8::String::new(scope, "token").unwrap();
  assert_eq!(
    frozen.set(scope, token_key.into(), assigned.into()),
    Some(false)
  );
  assert_eq!(
    frozen
      .get(scope, token_key.into())
      .unwrap()
      .number_value(scope),
    Some(17.0)
  );
  let blocked_key = v8::String::new(scope, "blocked").unwrap();
  {
    v8::tc_scope!(let caught, scope);
    assert_eq!(
      default.set(caught, blocked_key.into(), assigned.into()),
      None
    );
    assert!(caught.has_caught());
    assert!(
      caught.exception().unwrap().strict_equals(marker),
      "setter exception identity"
    );
  }
  let fail_key = v8::String::new(scope, "fail").unwrap();
  let fail = namespace.get(scope, fail_key.into()).unwrap();
  let fail = v8::Local::<v8::Function>::try_from(fail).unwrap();
  {
    v8::tc_scope!(let caught, scope);
    assert!(fail.call(caught, undefined.into(), &[]).is_none());
    assert!(caught.has_caught());
    assert!(
      caught.exception().unwrap().strict_equals(marker),
      "original thrown object identity"
    );
  }
  let after = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(
    after.runtime_eval_instantiations, before.runtime_eval_instantiations,
    "AOT namespace graph must not instantiate an interpreter provider"
  );
}

#[cfg(feature = "js2wasm_runtime_compile")]
#[test]
fn compiler_source_cache_identity_tracks_codegen_and_symlinks() {
  v8::js2wasm_test_compiler_source_identity();
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
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
#[ignore = "requires separately precompiled Script environment fixtures"]
fn precompiled_scripts_share_context_lexicals_without_interpreter() {
  let path = std::env::var_os("V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR")
    .expect("trusted local precompiled Script environment fixtures");
  v8::js2wasm_test_precompiled_script_environment(Path::new(&path)).unwrap();
}

#[test]
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
#[ignore = "requires trusted Context and source-bound public Script packages"]
fn runs_source_bound_aot_scripts_through_public_api() {
  initialize();
  let path = std::env::var_os("V8X_JS2WASM_SCRIPT_ENVIRONMENT_DIR")
    .expect("precompiled Context fixture");
  let directory = std::env::var_os("V8X_JS2WASM_AOT_SCRIPT_DIR")
    .expect("trusted public Script packages");
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  v8::js2wasm_attach_precompiled_realm_for_test(
    &context,
    &Path::new(&path).join("context.cwasm"),
  )
  .unwrap();
  let source = v8::String::new(scope, "41;42;").unwrap();
  let script = v8::Script::compile(scope, source, None).unwrap();
  assert_eq!(script.run(scope).unwrap().number_value(scope), Some(42.0));
  assert_eq!(script.run(scope).unwrap().number_value(scope), Some(42.0));
  {
    v8::tc_scope!(let caught, scope);
    let resource = v8::String::new(caught, "file:///other.js").unwrap();
    let origin = v8::ScriptOrigin::new(
      caught,
      resource.into(),
      0,
      0,
      false,
      -1,
      None,
      false,
      false,
      false,
      None,
    );
    let source = v8::String::new(caught, "41;42;").unwrap();
    assert!(
      v8::Script::compile(caught, source, Some(&origin))
        .unwrap()
        .run(caught)
        .is_none()
    );
    assert!(caught.has_caught());
  }
  let source = v8::String::new(scope, "void 0;").unwrap();
  assert!(
    v8::Script::compile(scope, source, None)
      .unwrap()
      .run(scope)
      .unwrap()
      .is_undefined()
  );
  let source = v8::String::new(
    scope,
    "globalThis.completionSaved={marker:42};globalThis.completionSaved;",
  )
  .unwrap();
  let object = v8::Script::compile(scope, source, None)
    .unwrap()
    .run(scope)
    .unwrap();
  let source = v8::String::new(scope, "globalThis.completionSaved;").unwrap();
  let again = v8::Script::compile(scope, source, None)
    .unwrap()
    .run(scope)
    .unwrap();
  assert!(object.strict_equals(again));
  {
    let second = v8::Context::new(scope, Default::default());
    let second_scope = &mut v8::ContextScope::new(scope, second);
    v8::js2wasm_attach_precompiled_realm_for_test(
      &second,
      &Path::new(&path).join("context.cwasm"),
    )
    .unwrap();
    let source =
      v8::String::new(second_scope, "globalThis.completionSaved;").unwrap();
    assert!(
      v8::Script::compile(second_scope, source, None)
        .unwrap()
        .run(second_scope)
        .unwrap()
        .is_undefined()
    );
  }
  {
    v8::tc_scope!(let caught, scope);
    let source =
      v8::String::new(caught, "throw globalThis.completionSaved;").unwrap();
    assert!(
      v8::Script::compile(caught, source, None)
        .unwrap()
        .run(caught)
        .is_none()
    );
    assert!(caught.exception().unwrap().strict_equals(object));
  }
  {
    v8::tc_scope!(let caught, scope);
    let source = v8::String::new(caught, "throw undefined;").unwrap();
    assert!(
      v8::Script::compile(caught, source, None)
        .unwrap()
        .run(caught)
        .is_none()
    );
    assert!(caught.has_caught());
    assert!(caught.exception().unwrap().is_undefined());
  }
  {
    v8::tc_scope!(let caught, scope);
    let source = v8::String::new(caught, "throw 42;").unwrap();
    assert!(
      v8::Script::compile(caught, source, None)
        .unwrap()
        .run(caught)
        .is_none()
    );
    assert_eq!(caught.exception().unwrap().number_value(caught), Some(42.0));
  }
  {
    v8::tc_scope!(let caught, scope);
    let source =
      v8::String::new(caught, "globalThis.shouldNotRun=2;42;").unwrap();
    assert!(
      v8::Script::compile(caught, source, None)
        .unwrap()
        .run(caught)
        .is_none()
    );
    assert!(caught.has_caught());
  }
  // Byte mismatch must be rejected before the initializer writes this marker.
  // Corrupt only a fresh generated test package that has never been loaded.
  let package = fs::read_dir(&directory)
    .unwrap()
    .filter_map(Result::ok)
    .map(|entry| entry.path())
    .find(|path| {
      path
        .extension()
        .is_some_and(|extension| extension == "json")
        && fs::read_to_string(path)
          .unwrap()
          .contains("globalThis.shouldNotRun=1;42;")
    })
    .expect("marker test package manifest");
  let native = package.with_extension("");
  let mut bytes = fs::read(&native).unwrap();
  bytes.push(0);
  fs::write(&native, bytes).unwrap();
  {
    v8::tc_scope!(let caught, scope);
    let source =
      v8::String::new(caught, "globalThis.shouldNotRun=1;42;").unwrap();
    assert!(
      v8::Script::compile(caught, source, None)
        .unwrap()
        .run(caught)
        .is_none()
    );
    assert!(caught.has_caught());
  }
  let key = v8::String::new(scope, "shouldNotRun").unwrap();
  assert!(
    context
      .global(scope)
      .get(scope, key.into())
      .unwrap()
      .is_undefined()
  );
  let stats = v8::js2wasm_runtime_stats().unwrap();
  assert_eq!(stats.compilations, 0);
  assert_eq!(stats.runtime_eval_instantiations, 0);
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
#[cfg(all(feature = "js2wasm_deno_poc", feature = "js2wasm_runtime_compile"))]
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
  DENO_STARTUP_OP_EVENTS.with(|events| events.borrow_mut().clear());
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
    if name == "01_core.js" {
      let bootstrap_key = v8::String::new(scope, "__bootstrap").unwrap();
      let primordials_key = v8::String::new(scope, "primordials").unwrap();
      let queue_key = v8::String::new(scope, "queueMicrotask").unwrap();
      let bootstrap = v8::Local::<v8::Object>::try_from(
        global.get(scope, bootstrap_key.into()).unwrap(),
      )
      .unwrap();
      let primordials = v8::Local::<v8::Object>::try_from(
        bootstrap.get(scope, primordials_key.into()).unwrap(),
      )
      .unwrap();
      let queue = primordials.get(scope, queue_key.into()).unwrap();
      assert!(
        queue.is_undefined(),
        "primordial queueMicrotask must be uninitialized before core; function={}, object={}, number={}",
        queue.is_function(),
        queue.is_object(),
        queue.is_number()
      );
    }
    if name == "02_timers.js" {
      // Mirror the core host operations required during 01_core startup.
      // Register real callbacks, not artifact-local fallbacks: attachment
      // deliberately replaces the scaffold's Deno object with this host graph.
      let extras_op =
        v8::Function::new_raw(scope, deno_op_extras_callback).unwrap();
      let extras_key =
        v8::String::new(scope, "op_get_extras_binding_object").unwrap();
      assert_eq!(
        ops.set(scope, extras_key.into(), extras_op.into()),
        Some(true)
      );
      let import_meta_prototype = v8::Object::new(scope);
      let meta_op =
        v8::FunctionTemplate::builder_raw(deno_op_return_data_callback)
          .data(import_meta_prototype.into())
          .constructor_behavior(v8::ConstructorBehavior::Throw)
          .build(scope)
          .get_function(scope)
          .unwrap();
      let meta_key =
        v8::String::new(scope, "op_get_ext_import_meta_proto").unwrap();
      assert_eq!(ops.set(scope, meta_key.into(), meta_op.into()), Some(true));
      let capture_op =
        v8::Function::new_raw(scope, deno_op_capture_bootstrap_callback)
          .unwrap();
      let capture_key =
        v8::String::new(scope, "op_set_captured_bootstrap").unwrap();
      assert_eq!(
        ops.set(scope, capture_key.into(), capture_op.into()),
        Some(true)
      );

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
    assert!(
      script.run(scope).is_some(),
      "run {specifier}; startup ops: {:?}",
      DENO_STARTUP_OP_EVENTS.with(|events| events.borrow().clone())
    );
  }

  assert_eq!(
    DENO_STARTUP_OP_EVENTS.with(|events| events.borrow().clone()),
    ["extras", "import-meta", "capture-bootstrap"]
  );

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

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  for (text, expected) in
    [("3", 3.0_f64), ("", 0.0), ("0x10", 16.0), ("-0", -0.0)]
  {
    let value = v8::String::new(scope, text).unwrap();
    let actual = value
      .number_value(scope)
      .expect("ToNumber string conversion");
    assert_eq!(actual.to_bits(), expected.to_bits(), "{text:?}");
  }
  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  {
    let invalid = v8::String::new(scope, "test").unwrap();
    assert!(invalid.number_value(scope).unwrap().is_nan());

    let first = v8::Number::new(scope, 1.0);
    let second = v8::String::new(scope, "2").unwrap();
    let array =
      v8::Array::new_with_elements(scope, &[first.into(), second.into()]);
    let iterator_key = v8::Symbol::get_iterator(scope);
    let method = array.get(scope, iterator_key.into()).unwrap();
    let method = v8::Local::<v8::Function>::try_from(method).unwrap();
    let iterator = method.call(scope, array.into(), &[]).unwrap();
    let iterator = iterator.to_object(scope).unwrap();
    let next_key = v8::String::new(scope, "next").unwrap();
    let next = iterator.get(scope, next_key.into()).unwrap();
    let next = v8::Local::<v8::Function>::try_from(next).unwrap();
    // Iteration stays live after adoption, rather than reading a detached copy.
    let replacement = v8::String::new(scope, "3").unwrap();
    assert_eq!(array.set_index(scope, 1, replacement.into()), Some(true));
    assert_eq!(
      array.get_index(scope, 1).unwrap().number_value(scope),
      Some(3.0)
    );
    let value_key = v8::String::new(scope, "value").unwrap();
    let done_key = v8::String::new(scope, "done").unwrap();
    for expected in [1.0, 3.0] {
      let result = next.call(scope, iterator.into(), &[]).unwrap();
      let result = result.to_object(scope).unwrap();
      assert!(
        !result
          .get(scope, done_key.into())
          .unwrap()
          .boolean_value(scope)
      );
      assert_eq!(
        result
          .get(scope, value_key.into())
          .unwrap()
          .number_value(scope),
        Some(expected)
      );
    }
    let result = next.call(scope, iterator.into(), &[]).unwrap();
    let result = result.to_object(scope).unwrap();
    assert!(
      result
        .get(scope, done_key.into())
        .unwrap()
        .boolean_value(scope)
    );
    let overridden = v8::Array::new(scope, 0);
    let undefined = v8::undefined(scope);
    assert_eq!(
      overridden.set(scope, iterator_key.into(), undefined.into()),
      Some(true)
    );
    assert!(
      overridden
        .get(scope, iterator_key.into())
        .unwrap()
        .is_undefined()
    );
  }

  let stub_key = v8::String::new(scope, "setUpAsyncStub").unwrap();
  let stub = core.get(scope, stub_key.into()).unwrap();
  let stub = v8::Local::<v8::Function>::try_from(stub).unwrap();
  let op = v8::Function::builder_raw(deno_async_probe_callback)
    .length(1)
    .build(scope)
    .unwrap();
  let op_value: v8::Local<v8::Value> = op.into();
  let name = v8::String::new(scope, "op_async_probe").unwrap();
  let undefined = v8::undefined(scope);
  let returned = stub
    .call(scope, undefined.into(), &[name.into(), op_value])
    .unwrap();
  #[cfg(feature = "js2wasm_deno_poc_replay")]
  assert!(std::ptr::eq(&*returned, &*op_value));
  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  {
    assert!(!returned.strict_equals(op_value));
    let wrapper = v8::Local::<v8::Function>::try_from(returned).unwrap();
    let result = wrapper.call(scope, undefined.into(), &[]).unwrap();
    assert!(
      result.is_promise(),
      "async wrapper result: undefined={}, number={}, object={}, function={}",
      result.is_undefined(),
      result.is_number(),
      result.is_object(),
      result.is_function()
    );
    let promise = v8::Local::<v8::Promise>::try_from(result).unwrap();
    assert_eq!(promise.state(), v8::PromiseState::Fulfilled);
    assert_eq!(promise.result(scope).number_value(scope), Some(42.0));
    // Exercise every generated argument-bearing branch of upstream's async
    // stub, not just async_op_0. Receiver and final argument must preserve
    // their Rust object identity through compiled Function.prototype.call.
    DENO_ASYNC_ARGUMENT_COUNTS.with(|counts| counts.borrow_mut().clear());
    for arity in 1..=9 {
      let marker = v8::Object::new(scope);
      let op = v8::Function::builder_raw(deno_async_arguments_callback)
        .length(arity + 1)
        .data(marker.into())
        .build(scope)
        .unwrap();
      let wrapped = stub
        .call(scope, undefined.into(), &[name.into(), op.into()])
        .unwrap();
      let wrapped = v8::Local::<v8::Function>::try_from(wrapped).unwrap();
      let mut arguments: Vec<v8::Local<v8::Value>> = (1..arity)
        .map(|index| v8::Integer::new(scope, index).into())
        .collect();
      arguments.push(marker.into());
      let result = wrapped.call(scope, marker.into(), &arguments).unwrap();
      let result = v8::Local::<v8::Promise>::try_from(result).unwrap();
      assert_eq!(result.state(), v8::PromiseState::Fulfilled);
      assert!(result.result(scope).strict_equals(marker.into()));
    }
    assert_eq!(
      DENO_ASYNC_ARGUMENT_COUNTS.with(|counts| counts.borrow().clone()),
      (2..=10).collect::<Vec<_>>()
    );
    // Preserve upstream's deliberate arity ceiling. Unsupported signatures
    // must throw before calling the Rust op, not truncate its arguments.
    let oversized = v8::Function::builder_raw(deno_async_arguments_callback)
      .length(11)
      .build(scope)
      .unwrap();
    {
      v8::tc_scope!(let catch, scope);
      assert!(
        stub
          .call(catch, undefined.into(), &[name.into(), oversized.into()])
          .is_none()
      );
      let error = catch.exception().unwrap().to_string(catch).unwrap();
      let error = error.to_rust_string_lossy(catch);
      assert!(
        error.contains("Too many arguments"),
        "unexpected error: {error}"
      );
    }
    assert_eq!(
      DENO_ASYNC_ARGUMENT_COUNTS.with(|counts| counts.borrow().len()),
      9
    );
    assert!(!promise.has_handler());
    promise.mark_as_handled();
    assert!(promise.has_handler());
    let handler = v8::Function::builder_raw(realm_host_leaf)
      .build(scope)
      .unwrap();
    let derived = promise
      .then(scope, handler)
      .expect("native then on compiled Promise");
    assert!(!derived.strict_equals(promise.into()));
    assert_eq!(derived.state(), v8::PromiseState::Pending);
    assert!(!derived.has_handler());
    // A compiled reaction queued first must precede a native job. Its chained
    // reaction is queued while executing and must follow that native job.
    ORDERED_MICROTASK_EVENTS.with(|events| events.borrow_mut().clear());
    let first_marker = v8::Integer::new(scope, 1);
    let first = v8::Function::builder_raw(record_ordered_microtask)
      .data(first_marker.into())
      .build(scope)
      .unwrap();
    let chained = promise.then(scope, first).unwrap();
    let last_marker = v8::Integer::new(scope, 3);
    let last = v8::Function::builder_raw(record_ordered_microtask)
      .data(last_marker.into())
      .build(scope)
      .unwrap();
    let completed = chained.then(scope, last).unwrap();
    let middle_marker = v8::Integer::new(scope, 2);
    let middle = v8::Function::builder_raw(record_ordered_microtask)
      .data(middle_marker.into())
      .build(scope)
      .unwrap();
    scope.enqueue_microtask(middle);
    assert!(ORDERED_MICROTASK_EVENTS.with(|events| events.borrow().is_empty()));
    scope.perform_microtask_checkpoint();
    assert_eq!(
      ORDERED_MICROTASK_EVENTS.with(|events| events.borrow().clone()),
      [1, 2, 3]
    );
    assert_eq!(completed.state(), v8::PromiseState::Fulfilled);
    scope.perform_microtask_checkpoint();
    assert_eq!(derived.state(), v8::PromiseState::Fulfilled);
    assert_eq!(derived.result(scope).number_value(scope), Some(43.0));
    assert!(promise.has_handler());
    assert!(!derived.has_handler());
    let captured = v8::Object::new(scope);
    let current = v8::Object::new(scope);
    scope.set_continuation_preserved_embedder_data(captured.into());
    let handler = v8::Function::builder_raw(return_reaction_continuation)
      .build(scope)
      .unwrap();
    let with_context = promise.then(scope, handler).unwrap();
    scope.set_continuation_preserved_embedder_data(current.into());
    scope.perform_microtask_checkpoint();
    assert!(with_context.result(scope).strict_equals(captured.into()));
    assert!(
      scope
        .get_continuation_preserved_embedder_data()
        .strict_equals(current.into())
    );
    scope.set_continuation_preserved_embedder_data(undefined.into());

    let thrower = v8::Function::builder_raw(realm_host_throw)
      .build(scope)
      .unwrap();
    let rejected = promise.then(scope, thrower).unwrap();
    let recovered = rejected
      .catch(scope, handler)
      .expect("native catch on compiled Promise");
    assert_eq!(rejected.state(), v8::PromiseState::Pending);
    assert_eq!(recovered.state(), v8::PromiseState::Pending);
    scope.perform_microtask_checkpoint();
    assert_eq!(rejected.state(), v8::PromiseState::Rejected);
    assert_eq!(recovered.state(), v8::PromiseState::Fulfilled);
    assert!(recovered.result(scope).is_undefined());

    // Fulfilled object results must retain their original Rust identity.
    let marker = v8::Object::new(scope);
    DENO_STRING_COERCION_EVENTS.with(|events| events.borrow_mut().clear());
    let string_key = v8::String::new(scope, "toString").unwrap();
    let string_method =
      v8::Function::builder_raw(deno_string_coercion_callback)
        .data(marker.into())
        .build(scope)
        .unwrap();
    assert_eq!(
      marker.set(scope, string_key.into(), string_method.into()),
      Some(true)
    );
    let object_op = v8::Function::builder_raw(deno_async_probe_callback)
      .length(1)
      .data(marker.into())
      .build(scope)
      .unwrap();
    let object_wrapper = stub
      .call(scope, undefined.into(), &[name.into(), object_op.into()])
      .unwrap();
    let object_wrapper =
      v8::Local::<v8::Function>::try_from(object_wrapper).unwrap();
    let object_result =
      object_wrapper.call(scope, undefined.into(), &[]).unwrap();
    let object_result =
      v8::Local::<v8::Promise>::try_from(object_result).unwrap();
    assert_eq!(object_result.state(), v8::PromiseState::Fulfilled);
    assert!(object_result.result(scope).strict_equals(marker.into()));
    assert_eq!(
      marker.to_string(scope).unwrap().to_rust_string_lossy(scope),
      "native string coercion"
    );
    let reason = v8::Object::new(scope);
    let throw_method = v8::Function::builder_raw(deno_string_coercion_throw)
      .data(reason.into())
      .build(scope)
      .unwrap();
    assert_eq!(
      marker.set(scope, string_key.into(), throw_method.into()),
      Some(true)
    );
    {
      v8::tc_scope!(let catch, scope);
      assert!(marker.to_string(catch).is_none());
      assert!(catch.exception().unwrap().strict_equals(reason.into()));
    }
    assert_eq!(
      DENO_STRING_COERCION_EVENTS.with(|events| events.borrow().clone()),
      ["string", "throw"]
    );

    // A pending native op must stay pending, not be snapshotted as fulfilled.
    let has_key = v8::String::new(scope, "hasPromise").unwrap();
    let has = core.get(scope, has_key.into()).unwrap();
    let has = v8::Local::<v8::Function>::try_from(has).unwrap();
    for index in [0, 4095] {
      let index = v8::Integer::new(scope, index);
      v8::tc_scope!(let catch, scope);
      let state = has.call(catch, undefined.into(), &[index.into()]);
      let exception = catch
        .exception()
        .and_then(|value| value.to_string(catch))
        .map(|value| value.to_rust_string_lossy(catch));
      let state = state
        .unwrap_or_else(|| panic!("read initial pending ring: {exception:?}"));
      assert!(
        !state.boolean_value(catch),
        "initial pending ring must be empty"
      );
    }
    DENO_PENDING_OP_IDS.with(|ids| ids.borrow_mut().clear());
    let pending_op = v8::Function::builder_raw(deno_pending_probe_callback)
      .length(1)
      .build(scope)
      .unwrap();
    let pending_wrapper = stub
      .call(scope, undefined.into(), &[name.into(), pending_op.into()])
      .unwrap();
    let pending_wrapper =
      v8::Local::<v8::Function>::try_from(pending_wrapper).unwrap();
    {
      v8::tc_scope!(let catch, scope);
      let pending = pending_wrapper.call(catch, undefined.into(), &[]);
      assert_eq!(
        DENO_PENDING_OP_IDS.with(|ids| ids.borrow().clone()),
        [(1, Some(0.0))],
        "immediate ops do not consume a pending promise id"
      );
      let exception = catch
        .exception()
        .and_then(|value| value.to_string(catch))
        .map(|value| value.to_rust_string_lossy(catch));
      let pending = pending.unwrap_or_else(|| {
        panic!("pending async wrapper failed: {exception:?}")
      });
      let pending = v8::Local::<v8::Promise>::try_from(pending).unwrap();
      assert_eq!(pending.state(), v8::PromiseState::Pending);
      assert!(!pending.has_handler());
      let handler = v8::Function::builder_raw(realm_host_leaf)
        .build(catch)
        .unwrap();
      let chained = pending
        .then(catch, handler)
        .expect("native then on pending compiled Promise");
      assert_eq!(chained.state(), v8::PromiseState::Pending);
      assert!(pending.has_handler());
      DENO_REACTION_ORDER.with(|events| events.borrow_mut().clear());
      for order in [1, 2, 3] {
        let data = v8::Integer::new(catch, order);
        let handler = v8::Function::builder_raw(record_reaction_order)
          .data(data.into())
          .build(catch)
          .unwrap();
        let derived = pending.then(catch, handler).unwrap();
        assert_eq!(derived.state(), v8::PromiseState::Pending);
      }
      let resolve_key = v8::String::new(catch, "__eventLoopTick").unwrap();
      let resolve = v8::Local::<v8::Function>::try_from(
        core.get(catch, resolve_key.into()).unwrap(),
      )
      .unwrap();
      let id = v8::Integer::new(catch, 0);
      let value = v8::Number::new(catch, 42.0);
      let ok = v8::Boolean::new(catch, true);
      assert!(
        resolve
          .call(
            catch,
            undefined.into(),
            &[id.into(), ok.into(), value.into()]
          )
          .unwrap()
          .is_undefined()
      );
      assert_eq!(
        pending.state(),
        v8::PromiseState::Pending,
        "the catch-derived op promise must wait for its reaction job"
      );
      catch.perform_microtask_checkpoint();
      assert_eq!(
        DENO_REACTION_ORDER.with(|events| events.borrow().clone()),
        [1.0, 2.0, 3.0]
      );
      assert_eq!(pending.state(), v8::PromiseState::Fulfilled);
      assert_eq!(pending.result(catch).number_value(catch), Some(42.0));
      assert_eq!(chained.state(), v8::PromiseState::Fulfilled);
      assert_eq!(chained.result(catch).number_value(catch), Some(43.0));
      assert!(!catch.has_caught());
      // The live native Promise wrapper must retain object/rejection identity
      // after settlement, not replace either with serialized copies.
      let marker = v8::Object::new(catch);
      let message = v8::String::new(catch, "pending op rejected").unwrap();
      let reason = v8::Exception::type_error(catch, message);
      for (id, value, success, expected) in [
        (1, marker.into(), true, v8::PromiseState::Fulfilled),
        (2, reason, false, v8::PromiseState::Rejected),
      ] {
        let result =
          pending_wrapper.call(catch, undefined.into(), &[]).unwrap();
        let result = v8::Local::<v8::Promise>::try_from(result).unwrap();
        assert_eq!(result.state(), v8::PromiseState::Pending);
        let id_value = v8::Integer::new(catch, id);
        let success = v8::Boolean::new(catch, success);
        assert!(
          resolve
            .call(
              catch,
              undefined.into(),
              &[id_value.into(), success.into(), value]
            )
            .unwrap()
            .is_undefined()
        );
        assert_eq!(result.state(), v8::PromiseState::Pending);
        catch.perform_microtask_checkpoint();
        assert_eq!(result.state(), expected);
        let actual = result.result(catch);
        let display = actual
          .to_string(catch)
          .map(|value| value.to_rust_string_lossy(catch));
        let object = v8::Local::<v8::Object>::try_from(actual).ok();
        let mut details = Vec::new();
        if let Some(object) = object {
          for key in ["name", "message"] {
            let property = v8::String::new(catch, key).unwrap();
            details.push(
              object
                .get(catch, property.into())
                .and_then(|value| value.to_string(catch))
                .map(|value| value.to_rust_string_lossy(catch)),
            );
          }
        }
        assert!(
          actual.strict_equals(value),
          "pending op {id} changed result identity: {display:?} {details:?}"
        );
        if id == 2 {
          let error = v8::Local::<v8::Object>::try_from(actual).unwrap();
          let stack_key = v8::String::new(catch, "stack").unwrap();
          let stack = error.get(catch, stack_key.into()).unwrap();
          assert!(stack.is_string(), "rejection must capture an actual stack");
          let stack =
            stack.to_string(catch).unwrap().to_rust_string_lossy(catch);
          assert!(
            stack.starts_with("TypeError: pending op rejected"),
            "{stack}"
          );
          assert!(
            stack.contains("wasm-function["),
            "captured stack has no actual Wasm frames: {stack}"
          );
        }
        assert!(!catch.has_caught());
      }
      assert_eq!(
        DENO_PENDING_OP_IDS.with(|ids| ids.borrow().clone()),
        [(1, Some(0.0)), (1, Some(1.0)), (1, Some(2.0))]
      );
    }
  }

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

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  {
    let namespace = v8::Local::<v8::Object>::try_from(namespace_after).unwrap();
    // Unlike the upstream test that drops its evaluation future, explicitly
    // inspect the application's evaluation promise and exported result.
    let application = v8::String::new(scope, r#"
      import { core, primordials, internals } from "ext:core/mod.js";
      if (typeof core === "undefined") throw new Error("core missing");
      if (typeof primordials === "undefined") throw new Error("primordials missing");
      if (typeof internals === "undefined") throw new Error("internals missing");
      export const answer = primordials.ArrayPrototypeReduce([1, 2, 3], (sum, value) => sum + value, 0);
    "#).unwrap();
    let resource =
      v8::String::new(scope, "ext:application/core-import.js").unwrap();
    let application_origin = origin(scope, resource.into());
    let mut source =
      v8::script_compiler::Source::new(application, Some(&application_origin));
    let application =
      v8::script_compiler::compile_module(scope, &mut source).unwrap();
    assert!(
      application
        .instantiate_module(scope, resolve_dependency)
        .unwrap()
    );
    let evaluated = application
      .evaluate(scope)
      .expect("core-import application evaluation");
    let evaluated = v8::Local::<v8::Promise>::try_from(evaluated).unwrap();
    assert_eq!(
      evaluated.state(),
      v8::PromiseState::Fulfilled,
      "core-import application rejected"
    );
    assert_eq!(application.get_status(), v8::ModuleStatus::Evaluated);
    let application_namespace =
      v8::Local::<v8::Object>::try_from(application.get_module_namespace())
        .unwrap();
    let answer = v8::String::new(scope, "answer").unwrap();
    assert_eq!(
      application_namespace
        .get(scope, answer.into())
        .unwrap()
        .number_value(scope),
      Some(6.0)
    );
    assert!(namespace.get_prototype(scope).unwrap().is_null());
    let bootstrap_key = v8::String::new(scope, "__bootstrap").unwrap();
    let bootstrap = v8::Local::<v8::Object>::try_from(
      global.get(scope, bootstrap_key.into()).unwrap(),
    )
    .unwrap();
    for name in ["core", "internals", "primordials"] {
      let key = v8::String::new(scope, name).unwrap();
      let exported = namespace.get(scope, key.into()).unwrap();
      assert!(exported.is_object(), "Deno namespace export {name}");
      assert!(
        exported.strict_equals(bootstrap.get(scope, key.into()).unwrap()),
        "Deno namespace export identity {name}"
      );
      let replacement = v8::Object::new(scope);
      assert_eq!(
        namespace.set(scope, key.into(), replacement.into()),
        Some(false)
      );
      assert!(
        namespace
          .get(scope, key.into())
          .unwrap()
          .strict_equals(exported)
      );
    }

    // The actual upstream error builder must retain the native message after
    // its registered constructor is passed through a JavaScript parameter.
    let builder_key = v8::String::new(scope, "buildCustomError").unwrap();
    let builder = v8::Local::<v8::Function>::try_from(
      core.get(scope, builder_key.into()).unwrap(),
    )
    .unwrap();
    for name in [
      "Error",
      "TypeError",
      "RangeError",
      "ReferenceError",
      "SyntaxError",
      "URIError",
    ] {
      v8::tc_scope!(let catch, scope);
      let class = v8::String::new(catch, name).unwrap();
      let message = v8::String::new(catch, "native op failure").unwrap();
      let receiver = v8::undefined(catch);
      let error =
        builder.call(catch, receiver.into(), &[class.into(), message.into()]);
      assert!(!catch.has_caught(), "upstream {name} builder threw");
      let error = error.expect("upstream error builder must return a value");
      assert_eq!(
        error.to_string(catch).unwrap().to_rust_string_lossy(catch),
        format!("{name}: native op failure"),
      );
    }

    // Deno's exported continuation helpers must call the real host extras,
    // sharing object identity with the native API rather than private stubs.
    let get_key = v8::String::new(scope, "getAsyncContext").unwrap();
    let set_key = v8::String::new(scope, "setAsyncContext").unwrap();
    let get = v8::Local::<v8::Function>::try_from(
      core.get(scope, get_key.into()).unwrap(),
    )
    .unwrap();
    let set = v8::Local::<v8::Function>::try_from(
      core.get(scope, set_key.into()).unwrap(),
    )
    .unwrap();
    let marker = v8::Object::new(scope);
    let receiver = v8::undefined(scope);
    assert!(
      set
        .call(scope, receiver.into(), &[marker.into()])
        .unwrap()
        .is_undefined()
    );
    assert!(
      get
        .call(scope, receiver.into(), &[])
        .unwrap()
        .strict_equals(marker.into())
    );
    assert!(
      scope
        .get_continuation_preserved_embedder_data()
        .strict_equals(marker.into())
    );
    scope.set_continuation_preserved_embedder_data(receiver.into());
  }

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
  #[cfg(feature = "js2wasm_deno_poc_replay")]
  assert_eq!(after.instantiations - before.instantiations, 1);
  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  assert_eq!(after.instantiations - before.instantiations, 2);
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

unsafe extern "C" fn record_ordered_microtask(
  info: *const v8::FunctionCallbackInfo,
) {
  let info = unsafe { &*info };
  let parts = info.get_parts();
  v8::callback_scope!(unsafe scope, &parts);
  let args = v8::FunctionCallbackArguments::from_function_callback_info_parts(
    info, &parts,
  );
  let marker = args.data().number_value(scope).unwrap() as i32;
  ORDERED_MICROTASK_EVENTS.with(|events| events.borrow_mut().push(marker));
  let mut result = parts.return_value;
  result.set(args.get(0));
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

#[test]
#[ignore = "requires independently compiled linked callback fixtures"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn separate_contexts_isolate_graph_state_but_share_native_symbol_registry() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let contexts = [
    v8::Context::new(scope, Default::default()),
    v8::Context::new(scope, Default::default()),
  ];
  let directory =
    PathBuf::from(std::env::var_os("V8X_JS2WASM_LINKED_CALLBACK_DIR").unwrap());
  let mut registered = Vec::new();
  let mut fresh = Vec::new();
  let mut callbacks = Vec::new();
  for context in contexts {
    let scope = &mut v8::ContextScope::new(scope, context);
    v8::js2wasm_attach_realm_for_test(
      &context,
      &directory.join("context.wasm"),
    )
    .unwrap();
    v8::js2wasm_attach_graph_for_test(
      &context,
      &directory.join("producer.wasm"),
    )
    .unwrap();
    let global = context.global(scope);
    let key = v8::String::new(scope, "producerRegistered").unwrap();
    let value = global.get(scope, key.into()).unwrap();
    assert!(value.is_symbol());
    registered.push(v8::Global::new(scope, value));
    let key = v8::String::new(scope, "producerFresh").unwrap();
    let value = global.get(scope, key.into()).unwrap();
    assert!(value.is_symbol());
    fresh.push(v8::Global::new(scope, value));
    let key = v8::String::new(scope, "deferredCallback").unwrap();
    let function = v8::Local::<v8::Function>::try_from(
      global.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let two = v8::Number::new(scope, 2.0);
    assert_eq!(
      function
        .call(scope, global.into(), &[two.into()])
        .unwrap()
        .number_value(scope),
      Some(42.0)
    );
    callbacks.push(v8::Global::new(scope, function));
  }
  let a = v8::Local::new(scope, &registered[0]);
  let b = v8::Local::new(scope, &registered[1]);
  assert!(a.strict_equals(b), "Symbol.for is isolate-wide");
  let a = v8::Local::new(scope, &fresh[0]);
  let b = v8::Local::new(scope, &fresh[1]);
  assert!(
    !a.strict_equals(b),
    "fresh Symbols must not alias across stores"
  );
  for (index, context) in contexts.into_iter().enumerate() {
    let scope = &mut v8::ContextScope::new(scope, context);
    let global = context.global(scope);
    if index == 0 {
      v8::js2wasm_attach_graph_for_test(
        &context,
        &directory.join("replacement.wasm"),
      )
      .unwrap();
    }
    let key = v8::String::new(scope, "deferredCallback").unwrap();
    let current = v8::Local::<v8::Function>::try_from(
      global.get(scope, key.into()).unwrap(),
    )
    .unwrap();
    let retained = v8::Local::new(scope, &callbacks[index]);
    assert_eq!(current.strict_equals(retained.into()), index == 1);
    let three = v8::Number::new(scope, 3.0);
    assert_eq!(
      retained
        .call(scope, global.into(), &[three.into()])
        .unwrap()
        .number_value(scope),
      Some(45.0)
    );
  }
}

#[test]
#[ignore = "requires independently compiled linked callback fixtures"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn runtime_eval_preserves_context_symbol_identity() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let directory =
    PathBuf::from(std::env::var_os("V8X_JS2WASM_LINKED_CALLBACK_DIR").unwrap());
  for name in ["context", "producer", "dynamic-eval"] {
    let path = directory.join(format!("{name}.wasm"));
    if name == "context" {
      v8::js2wasm_attach_realm_for_test(&context, &path).unwrap();
    } else {
      v8::js2wasm_attach_graph_for_test(&context, &path).unwrap();
    }
  }
  let global = context.global(scope);
  let marker = v8::String::new(scope, "dynamicEvalInitialized").unwrap();
  assert_eq!(
    global
      .get(scope, marker.into())
      .unwrap()
      .number_value(scope),
    Some(42.0),
    "eval graph publication marker"
  );
  let type_key = v8::String::new(scope, "dynamicEvalType").unwrap();
  let type_value = global.get(scope, type_key.into()).unwrap();
  assert_eq!(
    type_value
      .to_string(scope)
      .unwrap()
      .to_rust_string_lossy(scope),
    "function",
    "producer callable brand"
  );
  let key = v8::String::new(scope, "evaluateDynamic").unwrap();
  let candidate = global.get(scope, key.into()).unwrap();
  assert!(
    candidate.is_function(),
    "consumer callable brand: undefined={}, object={}",
    candidate.is_undefined(),
    candidate.is_object()
  );
  let evaluate =
    v8::Local::<v8::Function>::try_from(global.get(scope, key.into()).unwrap())
      .unwrap();
  // These sources arrive from the native host after compilation, not constants
  // available to the compiler. First prove the evaluator is actually executing.
  for source in [
    "__call_control__",
    "40 + 2 === 42",
    "Symbol.for('linked-key') === producerRegistered",
    "Symbol.keyFor(producerRegistered) === 'linked-key'",
    "producerFresh.description === 'fresh'",
    "Symbol('fresh') !== producerFresh",
  ] {
    v8::tc_scope!(let scope, scope);
    let text = v8::String::new(scope, source).unwrap();
    let value = evaluate
      .call(scope, global.into(), &[text.into()])
      .unwrap_or_else(|| {
        let exception = scope.exception().expect("caught evaluation exception");
        let message = exception
          .to_string(scope)
          .unwrap()
          .to_rust_string_lossy(scope);
        panic!("dynamic evaluation threw: {source}: {message}");
      });
    let description =
      value.to_string(scope).unwrap().to_rust_string_lossy(scope);
    assert!(
      value.is_true(),
      "dynamic evaluation result: {source}: {description}"
    );
  }
}

#[test]
fn number_value_converts_primitives_and_rejects_non_numeric_types() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  for (value, expected) in [
    (v8::Boolean::new(scope, true).into(), 1.0),
    (v8::Boolean::new(scope, false).into(), 0.0),
    (v8::null(scope).into(), 0.0),
  ] as [(v8::Local<v8::Value>, f64); 3]
  {
    assert_eq!(value.number_value(scope), Some(expected));
  }
  assert!(v8::undefined(scope).number_value(scope).unwrap().is_nan());
  let negative_zero = v8::Number::new(scope, -0.0);
  assert_eq!(
    negative_zero.number_value(scope).unwrap().to_bits(),
    (-0.0_f64).to_bits()
  );
  let values: [v8::Local<v8::Value>; 2] = [
    v8::Symbol::new(scope, None).into(),
    v8::BigInt::new_from_i64(scope, 1).into(),
  ];
  for value in values {
    v8::tc_scope!(let caught, scope);
    assert_eq!(value.number_value(caught), None);
    assert!(caught.has_caught());
    let text = caught
      .exception()
      .unwrap()
      .to_string(caught)
      .unwrap()
      .to_rust_string_lossy(caught);
    assert!(text.starts_with("TypeError:"), "{text}");
  }
}

#[test]
fn integer_value_preserves_numeric_boundaries() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  for (number, expected) in [
    (3.0, 3),
    (3.9, 3),
    (-3.9, -3),
    (0.0, 0),
    (-0.0, 0),
    (f64::NAN, 0),
    (f64::INFINITY, i64::MAX),
    (f64::NEG_INFINITY, i64::MIN),
    (f64::MAX, i64::MAX),
    (-f64::MAX, i64::MIN),
    (9_223_372_036_854_774_784.0, 9_223_372_036_854_774_784),
    (-9_223_372_036_854_774_784.0, -9_223_372_036_854_774_784),
  ] {
    let value = v8::Number::new(scope, number);
    assert_eq!(value.integer_value(scope), Some(expected), "{number:?}");
  }
  for (value, expected) in [
    (v8::Boolean::new(scope, true).into(), 1),
    (v8::Boolean::new(scope, false).into(), 0),
    (v8::null(scope).into(), 0),
    (v8::undefined(scope).into(), 0),
  ] as [(v8::Local<v8::Value>, i64); 4]
  {
    assert_eq!(value.integer_value(scope), Some(expected));
  }
}

#[test]
fn integer_value_rejects_symbol_and_bigint_with_type_error() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let values: [v8::Local<v8::Value>; 2] = [
    v8::Symbol::new(scope, None).into(),
    v8::BigInt::new_from_i64(scope, 1).into(),
  ];
  for value in values {
    v8::tc_scope!(let caught, scope);
    assert_eq!(value.integer_value(caught), None);
    assert!(caught.has_caught());
    let exception = caught.exception().unwrap();
    let text = exception
      .to_string(caught)
      .unwrap()
      .to_rust_string_lossy(caught);
    assert!(text.starts_with("TypeError:"), "{text}");
  }
}

#[test]
#[ignore = "requires compiled numeric-coercion context fixture"]
#[cfg(feature = "js2wasm_runtime_compile")]
fn integer_value_coerces_in_realm_and_preserves_exception_identity() {
  initialize();
  let isolate = &mut v8::Isolate::new(Default::default());
  v8::scope!(let scope, isolate);
  let context = v8::Context::new(scope, Default::default());
  let scope = &mut v8::ContextScope::new(scope, context);
  let path = std::env::var_os("V8X_JS2WASM_CONTEXT_VALUES_WASM")
    .expect("context fixture");
  v8::js2wasm_attach_realm_for_test(&context, Path::new(&path)).unwrap();
  for (text, expected) in [
    ("", 0),
    (" 42.9 ", 42),
    ("-3.9", -3),
    ("0x10", 16),
    ("0b11", 3),
    ("no", 0),
  ] {
    let value = v8::String::new(scope, text).unwrap();
    assert_eq!(value.integer_value(scope), Some(expected), "{text:?}");
  }
  let global = context.global(scope);
  let key = v8::String::new(scope, "coercionObject").unwrap();
  let object = global.get(scope, key.into()).unwrap();
  assert_eq!(object.integer_value(scope), Some(42));
  let key = v8::String::new(scope, "coercionError").unwrap();
  let token = global.get(scope, key.into()).unwrap();
  let key = v8::String::new(scope, "throwingCoercion").unwrap();
  let throwing = global.get(scope, key.into()).unwrap();
  v8::tc_scope!(let caught, scope);
  assert_eq!(throwing.integer_value(caught), None);
  assert!(caught.has_caught());
  assert!(caught.exception().unwrap().strict_equals(token));
}
