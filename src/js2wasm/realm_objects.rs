use super::*;
use crate::js2wasm_spike::{DenoRuntime, RealmAccess, RealmValue};
#[path = "realm_callback_access.rs"]
mod callback_access;
#[path = "realm_host_callbacks.rs"]
mod host_callbacks;
#[path = "realm_host_values.rs"]
mod host_values;
#[path = "realm_stack_trace.rs"]
mod stack_trace;
#[path = "realm_symbols.rs"]
mod symbols;
pub(super) use host_callbacks::HostCallbackBinding;
pub(crate) use host_callbacks::invoke_host;

#[derive(Clone)]
pub(super) struct RealmObjectBinding {
  host: *const Object,
  runtime: Rc<RefCell<DenoRuntime>>,
  value: RealmValue,
}

fn binding(object: *const Object) -> Option<RealmObjectBinding> {
  unsafe { isolate_state(current_isolate()) }
    .realm_objects
    .iter()
    .find(|entry| entry.host == object)
    .cloned()
}

fn into_realm(
  runtime: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  value: *const Value,
) -> Result<RealmValue, String> {
  if matches!(unsafe { heap_value(value) }, Some(HeapValue::Symbol(_))) {
    return symbols::into_realm(runtime, owner, value);
  }
  if let Some(entry) = binding(value.cast()) {
    if !Rc::ptr_eq(owner, &entry.runtime) {
      return Err("cannot transfer an object between realms".to_string());
    }
    return Ok(entry.value);
  }
  match unsafe { heap_value(value) } {
    Some(HeapValue::Undefined) => Ok(runtime.realm_undefined()),
    Some(HeapValue::Null) => runtime.realm_null(),
    Some(HeapValue::Boolean(value)) => runtime.realm_boolean(*value),
    Some(HeapValue::Number(number)) => runtime.realm_number(*number),
    Some(HeapValue::String(text)) => {
      let units: Vec<_> = text.encode_utf16().collect();
      runtime.realm_string(&units)
    }
    Some(HeapValue::Object(_)) | Some(HeapValue::Array(_)) | Some(HeapValue::Function(_)) => {
      host_values::transfer(runtime, owner, value)
    }
    Some(HeapValue::Error { name, message }) => {
      let name = name.to_string();
      let message = message.clone();
      let name = runtime.realm_string(&name.encode_utf16().collect::<Vec<_>>())?;
      let message = runtime.realm_string(&message.encode_utf16().collect::<Vec<_>>())?;
      let handle = runtime.realm_handle("__v8x_value_error",
        &[runtime.realm_check(name)?, runtime.realm_check(message)?])?;
      unsafe { isolate_state(current_isolate()) }.realm_objects.push(RealmObjectBinding {
        host: value.cast(), runtime: owner.clone(), value: handle,
      });
      Ok(handle)
    }

    Some(HeapValue::ArrayBuffer(_)) => {
      let handle = runtime.realm_adopt_buffer(RetainedHostBuffer::new(value.cast())?)?;
      unsafe { isolate_state(current_isolate()) }.realm_objects.push(RealmObjectBinding {
        host: value.cast(), runtime: owner.clone(), value: handle,
      });
      Ok(handle)
    }
    Some(HeapValue::TypedArray(state)) => {
      if !state.properties.is_empty() {
        return Err("host typed-array custom properties are not transferable yet".to_string());
      }
      let kind = match state.kind {
        TypedArrayKind::Uint8 => 0.0,
        TypedArrayKind::Uint16 => 1.0,
        TypedArrayKind::Uint32 => 2.0,
        TypedArrayKind::Int32 => 3.0,
        TypedArrayKind::BigUint64 => 4.0,
        TypedArrayKind::BigInt64 => 5.0,
      };
      let buffer = state.buffer;
      let offset = state.byte_offset as f64;
      let length = state.length as f64;
      let buffer = into_realm(runtime, owner, buffer.cast())?;
      let handle = runtime.realm_handle("__v8x_value_typed_array",
        &[runtime.realm_check(buffer)?, kind, offset, length])?;
      unsafe { isolate_state(current_isolate()) }.realm_objects.push(RealmObjectBinding {
        host: value.cast(), runtime: owner.clone(), value: handle,
      });
      Ok(handle)
    }
    _ => Err("host value conversion to the compiled realm is not implemented for this type".to_string()),

  }
}

fn from_realm(
  runtime: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  value: RealmValue,
) -> Result<*const Value, String> {
  match runtime.realm_kind(value)? {
    0 => Ok(v8__Undefined(current_isolate()).cast()),
    1 => Ok(v8__Null(current_isolate()).cast()),
    2 => {
      let boolean = runtime.realm_as_boolean(value)?;
      Ok(allocate(current_isolate(), HeapValue::Boolean(boolean)))
    }
    3 => {
      let number = runtime.realm_as_number(value)?;
      Ok(allocate(current_isolate(), HeapValue::Number(number)))
    }
    4 => {
      let units = runtime.realm_as_utf16(value)?;
      let text = String::from_utf16(&units).map_err(|_| {
        "Rust string backing cannot yet represent unpaired UTF-16 surrogates"
          .to_string()
      })?;
      Ok(new_string(current_isolate(), text).cast())
    }
    8 => symbols::from_realm(runtime, owner, value),
    kind @ (5 | 6 | 9) => {
      if let Some(entry) = unsafe { isolate_state(current_isolate()) }
        .realm_objects
        .iter()
        .find(|entry| entry.value == value && Rc::ptr_eq(owner, &entry.runtime))
      {
        return Ok(entry.host.cast());
      }
      let host = if kind == 5
        && runtime
          .realm_promise_snapshot(runtime.realm_check(value)?)?
          .is_some()
      {
        // Native storage is only the wrapper identity. Reads must query the
        // live Wasm-owned promise, never this placeholder's settlement.
        allocate_promise(
          current_isolate(),
          PromiseSettlement::Pending,
          ptr::null(),
        )
        .cast()
      } else if kind == 6 {
        let key =
          runtime.realm_string(&"name".encode_utf16().collect::<Vec<_>>())?;
        let name_value = runtime.realm_get(value, key)?;
        // A configurable JS name property is not proof of callability.
        // Keep non-string public names on the bound realm value; the native
        // wrapper has no string display name to cache in that case.
        let name = if runtime.realm_kind(name_value)? == 4 {
          String::from_utf16(&runtime.realm_as_utf16(name_value)?)
            .map_err(|_| "function name contains unpaired UTF-16".to_string())?
        } else {
          String::new()
        };
        let name = new_string(current_isolate(), name);
        allocate_function(
          current_isolate(),
          unbound_realm_callback,
          ptr::null(),
          Vec::new(),
          name,
        )
        .cast()
      } else if kind == 9 {
        allocate(
          current_isolate(),
          HeapValue::Array(ArrayState {
            elements: Vec::new(),
            properties: Vec::new(),
          }),
        )
      } else {
        new_object(current_isolate())
      };
      unsafe { isolate_state(current_isolate()) }
        .realm_objects
        .push(RealmObjectBinding {
          host,
          runtime: owner.clone(),
          value,
        });
      Ok(host.cast())
    }
    kind => Err(format!(
      "compiled realm value kind {kind} has no Rust wrapper yet"
    )),
  }
}

pub(super) fn is_bound(object: *const Object) -> bool {
  binding(object).is_some()
}

pub(super) fn promise_snapshot(
  promise: *const Promise,
) -> Option<Result<(PromiseState, *const Value), String>> {
  let entry = binding(promise.cast())?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let handle = runtime.realm_check(entry.value)?;
    let (state, value) = runtime
      .realm_promise_snapshot(handle)?
      .ok_or("bound Promise lost its compiled Promise ABI")?;
    let state = match state {
      0 => PromiseState::Pending,
      1 => PromiseState::Fulfilled,
      2 => PromiseState::Rejected,
      _ => return Err("invalid compiled Promise state".to_string()),
    };
    let value = if state == PromiseState::Pending {
      ptr::null()
    } else {
      let value = runtime.realm_from_handle(value)?;
      from_realm(runtime, &entry.runtime, value)?
    };
    Ok((state, value))
  }))
}

pub(super) fn promise_handler(
  promise: *const Promise,
  mark: bool,
) -> Option<Result<bool, String>> {
  let entry = binding(promise.cast())?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let handle = runtime.realm_check(entry.value)?;
    runtime
      .realm_promise_handler(handle, mark)?
      .ok_or("bound Promise lost its compiled handler ABI".to_string())
  }))
}

pub(super) fn promise_then(
  promise: *const Promise,
  on_fulfilled: *const crate::Function,
  on_rejected: *const crate::Function,
) -> Option<Result<*const Promise, String>> {
  let entry = binding(promise.cast())?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let mut handlers = Vec::new();
    for handler in [on_fulfilled, on_rejected] {
      let value = if handler.is_null() {
        runtime.realm_undefined()
      } else if binding(handler.cast()).is_none() {
        host_callbacks::allocate_reaction(runtime, &entry.runtime, handler)?
      } else {
        into_realm(runtime, &entry.runtime, handler.cast())?
      };
      handlers.push(runtime.realm_check(value)?);
    }
    let handles = [runtime.realm_check(entry.value)?, handlers[0], handlers[1]];
    let value = if let Some((success, handle)) =
      runtime.realm_try_graph_promise_then(handles)?
    {
      let value = runtime.realm_from_handle(handle)?;
      if !success {
        let exception = from_realm(runtime, &entry.runtime, value)?;
        record_exception(current_isolate(), exception);
        return Ok(ptr::null());
      }
      value
    } else {
      runtime.realm_handle("__v8x_value_promise_then", &handles)?
    };
    let derived = from_realm(runtime, &entry.runtime, value)?;
    if !matches!(unsafe { heap_value(derived) }, Some(HeapValue::Promise(_))) {
      return Err("compiled Promise reaction did not return a Promise".into());
    }
    Ok(derived.cast())
  }))
}

/// Retain the Module's original Rust identity while reading its live GC namespace.
pub(super) fn bind_source_namespace(
  host: *const Object,
  owner: &Rc<RefCell<DenoRuntime>>,
  specifier: &str,
) -> Result<(), String> {
  let mut runtime = owner.try_borrow_mut().map_err(|_| {
    "namespace publication re-entered an executing realm".to_string()
  })?;
  let global = runtime.realm_global()?;
  let registry_key = runtime.realm_string(
    &"__v8x_source_module_namespaces"
      .encode_utf16()
      .collect::<Vec<_>>(),
  )?;
  let registry = runtime.realm_get(global, registry_key)?;
  if runtime.realm_kind(registry)? != 5 {
    return Err(
      "compiled source graph did not publish its namespace registry"
        .to_string(),
    );
  }
  let key =
    runtime.realm_string(&specifier.encode_utf16().collect::<Vec<_>>())?;
  let namespace = runtime.realm_get(registry, key)?;
  if runtime.realm_kind(namespace)? != 5 {
    return Err(format!(
      "compiled graph did not publish namespace for {specifier:?}"
    ));
  }
  if let Some(previous) = binding(host) {
    if Rc::ptr_eq(owner, &previous.runtime) && previous.value == namespace {
      return Ok(());
    }
    return Err(
      "source module namespace was already bound to another value".to_string(),
    );
  }
  unsafe { isolate_state(current_isolate()) }
    .realm_objects
    .push(RealmObjectBinding {
      host,
      runtime: owner.clone(),
      value: namespace,
    });
  Ok(())
}

/// Bind the stable Rust namespace to the prelinked artifact's real namespace,
/// never to a snapshot of bootstrap fields or the adapter's private helpers.
pub(super) fn bind_prelinked_core_namespace(
  host: *const Object,
  owner: &Rc<RefCell<DenoRuntime>>,
) -> Result<(), String> {
  let mut runtime = owner.try_borrow_mut().map_err(|_| {
    "Deno namespace publication re-entered an executing realm".to_string()
  })?;
  let namespace =
    runtime.realm_handle("__v8x_deno_core_namespace_handle", &[])?;
  if runtime.realm_kind(namespace)? != 5 {
    return Err(
      "prelinked Deno artifact did not return a namespace object".to_string(),
    );
  }
  if let Some(previous) = binding(host) {
    if Rc::ptr_eq(owner, &previous.runtime) && previous.value == namespace {
      return Ok(());
    }
    return Err(
      "Deno core namespace was already bound to another value".to_string(),
    );
  }
  unsafe { isolate_state(current_isolate()) }
    .realm_objects
    .push(RealmObjectBinding {
      host,
      runtime: owner.clone(),
      value: namespace,
    });
  Ok(())
}

pub(super) fn length(array: *const Array) -> Option<Result<u32, String>> {
  let entry = binding(array.cast())?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let key =
      runtime.realm_string(&"length".encode_utf16().collect::<Vec<_>>())?;
    let value = runtime.realm_get(entry.value, key)?;
    let length = runtime.realm_as_number(value)?;
    if !length.is_finite()
      || length < 0.0
      || length > u32::MAX as f64
      || length.fract() != 0.0
    {
      return Err("invalid realm array length".to_string());
    }
    Ok(length as u32)
  }))
}

pub(super) fn get_prototype(
  object: *const Object,
) -> Option<Result<*const Value, String>> {
  if let Err(error) = adopt_native_error(object) {
    return Some(Err(error));
  }
  let entry = binding(object)?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let handles = [runtime.realm_check(entry.value)?];
    if let Some((success, handle)) =
      runtime.realm_try_graph_get_prototype(handles)?
    {
      let value = runtime.realm_from_handle(handle)?;
      let value = from_realm(runtime, &entry.runtime, value)?;
      if !success {
        record_exception(current_isolate(), value);
        return Ok(ptr::null());
      }
      return Ok(value);
    }
    let value = runtime.realm_get_prototype(entry.value)?;
    from_realm(runtime, &entry.runtime, value)
  }))
}

pub(super) fn set_prototype(
  object: *const Object,
  prototype: *const Value,
) -> Option<Result<Option<bool>, String>> {
  if let Err(error) = adopt_native_error(object) {
    return Some(Err(error));
  }
  let entry = binding(object)?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let prototype = into_realm(runtime, &entry.runtime, prototype)?;
    let handles = [
      runtime.realm_check(entry.value)?,
      runtime.realm_check(prototype)?,
    ];
    if let Some((success, handle)) =
      runtime.realm_try_graph_set_prototype(handles)?
    {
      let value = runtime.realm_from_handle(handle)?;
      if !success {
        let exception = from_realm(runtime, &entry.runtime, value)?;
        record_exception(current_isolate(), exception);
        return Ok(None);
      }
      return Ok(Some(runtime.realm_as_boolean(value)?));
    }
    runtime
      .realm_set_prototype(entry.value, prototype)
      .map(Some)
  }))
}

pub(super) fn get(
  object: *const Object,
  key: *const Value,
) -> Option<Result<*const Value, String>> {
  if let Err(error) = adopt_native_error(object) {
    return Some(Err(error));
  }
  let entry = binding(object)?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let key = into_realm(runtime, &entry.runtime, key)?;
    let handles =
      [runtime.realm_check(entry.value)?, runtime.realm_check(key)?];
    if let Some((success, handle)) = runtime.realm_try_graph_get(handles)? {
      let value = runtime.realm_from_handle(handle)?;
      let value = from_realm(runtime, &entry.runtime, value)?;
      if !success {
        record_exception(current_isolate(), value);
        return Ok(ptr::null());
      }
      return Ok(value);
    }
    let value = runtime.realm_get(entry.value, key)?;
    from_realm(runtime, &entry.runtime, value)
  }))
}

pub(super) fn set(
  object: *const Object,
  key: *const Value,
  value: *const Value,
) -> Option<Result<Option<bool>, String>> {
  if let Err(error) = adopt_native_error(object) {
    return Some(Err(error));
  }
  let entry = binding(object)?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let key = into_realm(runtime, &entry.runtime, key)?;
    let value = into_realm(runtime, &entry.runtime, value)?;
    let handles = [
      runtime.realm_check(entry.value)?,
      runtime.realm_check(key)?,
      runtime.realm_check(value)?,
    ];
    if let Some((success, handle)) = runtime.realm_try_graph_set(handles)? {
      let result = runtime.realm_from_handle(handle)?;
      if !success {
        let exception = from_realm(runtime, &entry.runtime, result)?;
        record_exception(current_isolate(), exception);
        return Ok(None);
      }
      return Ok(Some(runtime.realm_as_boolean(result)?));
    }
    runtime.realm_set(entry.value, key, value)?;
    Ok(Some(true))
  }))
}

unsafe extern "C" fn unbound_realm_callback(
  _info: *const crate::function::FunctionCallbackInfo,
) {
  report("realm function lost its owner binding".to_string());
}

pub(super) fn call(
  function: *const crate::Function,
  receiver: *const Value,
  args: &[*const Value],
  construct: bool,
) -> Option<Result<*const Value, String>> {
  let entry = binding(function.cast())?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    if construct {
      return Err(
        "constructing a realm-backed function is not implemented".to_string(),
      );
    }
    let receiver = into_realm(runtime, &entry.runtime, receiver)?;
    let array = runtime.realm_array()?;
    for (index, argument) in args.iter().enumerate() {
      let key = runtime
        .realm_string(&index.to_string().encode_utf16().collect::<Vec<_>>())?;
      let value = into_realm(runtime, &entry.runtime, *argument)?;
      runtime.realm_set(array, key, value)?;
    }
    let handles = [
      runtime.realm_check(entry.value)?,
      runtime.realm_check(receiver)?,
      runtime.realm_check(array)?,
    ];
    if let Some((success, handle)) = runtime.realm_try_graph_call(handles)? {
      let value = runtime.realm_from_handle(handle)?;
      let value = from_realm(runtime, &entry.runtime, value)?;
      if !success {
        record_exception(current_isolate(), value);
        return Ok(ptr::null());
      }
      return Ok(value);
    }
    let value = runtime.realm_call(entry.value, receiver, array)?;
    from_realm(runtime, &entry.runtime, value)
  }))
}

pub(super) fn to_number(
  value: *const Value,
  context: *const Context,
) -> Result<Option<f64>, String> {
  let owner = binding(value.cast())
    .or_else(|| binding(v8__Context__Global(context)))
    .ok_or_else(|| {
      "numeric coercion requires an attached compiled realm".to_string()
    })?
    .runtime;
  callback_access::with_owner(&owner, |runtime| {
    let value = into_realm(runtime, &owner, value)?;
    let envelope = runtime
      .realm_handle("__v8x_value_to_number", &[runtime.realm_check(value)?])?;
    let zero = runtime.realm_string(&[48])?;
    let one = runtime.realm_string(&[49])?;
    let success = runtime.realm_get(envelope, zero)?;
    let result = runtime.realm_get(envelope, one)?;
    if runtime.realm_as_boolean(success)? {
      Ok(Some(runtime.realm_as_number(result)?))
    } else {
      let exception = from_realm(runtime, &owner, result)?;
      record_exception(current_isolate(), exception);
      Ok(None)
    }
  })
}

pub(super) fn report(error: String) {
  if std::env::var_os("V8X_JS2WASM_TRACE_HOST").is_some() {
    eprintln!("v8x/js2wasm: {error}");
  }
  let message = new_string(current_isolate(), error);
  let exception = allocate_error(message, "Error");
  record_exception(current_isolate(), exception);
}

pub(crate) fn bootstrap_owner_for_attachment(
  identity: usize,
) -> Result<Rc<RefCell<DenoRuntime>>, String> {
  let owner = deno_core_runtime(current_context())?;
  // Equality token only, never dereferenced. ContextState retains the Rc.
  if identity == 0 || identity != Rc::as_ptr(&owner) as usize {
    return Err(
      "context attachment requires its published runtime".to_string(),
    );
  }
  Ok(owner)
}

pub(crate) fn attach_bootstrap_context(
  access: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
) -> Result<(), String> {
  let context = current_context();
  let published = deno_core_runtime(context)?;
  if !Rc::ptr_eq(owner, &published) {
    return Err(
      "context attachment runtime does not own this context".to_string(),
    );
  }
  let global = v8__Context__Global(context);
  if global.is_null() || binding(global).is_some() {
    return Err("context global is missing or already attached".to_string());
  }
  let target = access.realm_global()?;
  host_values::transfer_into(access, owner, global.cast(), Some(target))?;
  stack_trace::install(access, owner, target)?;
  Ok(())
}

#[cfg(feature = "js2wasm_runtime_compile")]
#[doc(hidden)]
pub fn js2wasm_bootstrap_context_for_test(
  context: &Context,
  path: &std::path::Path,
) -> Result<(), String> {
  if let Some(HeapValue::Context(state)) = unsafe { heap_value(context) } {
    if state.deno_core_bootstrap.is_some() || state.module_runtime.is_some() {
      return Err("Deno bootstrap context already owns a runtime".to_string());
    }
  }
  crate::js2wasm_spike::bootstrap_context_for_test(path, |runtime| {
    publish_deno_core_runtime(context, runtime)
  })?;
  Ok(())
}

#[cfg(feature = "js2wasm_runtime_compile")]
#[doc(hidden)]
pub fn js2wasm_attach_realm_for_test(
  context: &Context,
  path: &std::path::Path,
) -> Result<(), String> {
  let runtime = crate::js2wasm_spike::load_realm_for_test(path)?;
  attach_realm_for_test(context, runtime)
}

/// Attach a trusted build-pipeline artifact without linking a Wasm compiler.
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
#[doc(hidden)]
pub fn js2wasm_attach_precompiled_realm_for_test(
  context: &Context,
  path: &std::path::Path,
) -> Result<(), String> {
  let runtime = crate::js2wasm_spike::load_precompiled_realm_for_test(path)?;
  attach_realm_for_test(context, runtime)
}

#[cfg(any(
  feature = "js2wasm_runtime_compile",
  not(feature = "js2wasm_deno_poc_replay")
))]
fn attach_realm_for_test(
  context: &Context,
  runtime: Rc<RefCell<crate::js2wasm_spike::DenoRuntime>>,
) -> Result<(), String> {
  runtime.borrow_mut().configure_heap_limit(current_isolate());
  let value = runtime.borrow_mut().realm_global()?;
  let global = v8__Context__Global(context);
  if global.is_null() {
    return Err("test context has no global".to_string());
  }
  if binding(global).is_some() {
    return Err("context global is already attached to a realm".to_string());
  }
  host_values::transfer_into(
    &mut *runtime.borrow_mut(),
    &runtime,
    global.cast(),
    Some(value),
  )?;
  if let Some(HeapValue::Context(state)) = unsafe { heap_value_mut(context) } {
    state.module_runtime = Some(runtime);
  }
  Ok(())
}

#[cfg(feature = "js2wasm_runtime_compile")]
#[doc(hidden)]
pub fn js2wasm_run_core_script_for_test(
  context: &Context,
  phase: usize,
) -> Result<(), String> {
  with_deno_core_runtime(context, "test script stage", |runtime| {
    if !runtime.run_deno_core_script(phase)? {
      return Err("fixture has no deferred script runner".into());
    }
    Ok(())
  })
}

#[cfg(feature = "js2wasm_runtime_compile")]
#[doc(hidden)]
pub fn js2wasm_attach_graph_for_test(
  context: &Context,
  path: &std::path::Path,
) -> Result<(), String> {
  let entry =
    binding(v8__Context__Global(context)).ok_or("test context has no realm")?;
  let mut runtime = entry
    .runtime
    .try_borrow_mut()
    .map_err(|_| "test realm is executing")?;
  crate::js2wasm_spike::load_graph_for_test(&mut runtime, path)
}

fn adopt_native_error(object: *const Object) -> Result<(), String> {
  if !matches!(unsafe { heap_value(object) }, Some(HeapValue::Error { .. }))
    || binding(object).is_some()
  {
    return Ok(());
  }
  let context = current_context();
  if context.is_null() {
    return Ok(());
  }
  let Some(global) = binding(v8__Context__Global(context)) else {
    return Ok(());
  };
  callback_access::with_owner(&global.runtime, |runtime| {
    into_realm(runtime, &global.runtime, object.cast()).map(|_| ())
  })
}
