use super::*;
use crate::js2wasm_spike::{DenoRuntime, RealmAccess, RealmValue};
#[path = "realm_callback_access.rs"]
mod callback_access;
#[path = "realm_host_callbacks.rs"]
mod host_callbacks;
#[path = "realm_host_values.rs"]
mod host_values;
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
      runtime.realm_handle("__v8x_value_error",
        &[runtime.realm_check(name)?, runtime.realm_check(message)?])
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
    kind @ (5 | 6 | 9) => {
      if let Some(entry) = unsafe { isolate_state(current_isolate()) }
        .realm_objects
        .iter()
        .find(|entry| entry.value == value && Rc::ptr_eq(owner, &entry.runtime))
      {
        return Ok(entry.host.cast());
      }
      let host = if kind == 6 {
        let key =
          runtime.realm_string(&"name".encode_utf16().collect::<Vec<_>>())?;
        let name_value = runtime.realm_get(value, key)?;
        let name = String::from_utf16(&runtime.realm_as_utf16(name_value)?)
          .map_err(|_| "function name contains unpaired UTF-16".to_string())?;
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

pub(super) fn get(
  object: *const Object,
  key: *const Value,
) -> Option<Result<*const Value, String>> {
  let entry = binding(object)?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let key = into_realm(runtime, &entry.runtime, key)?;
    let value = runtime.realm_get(entry.value, key)?;
    from_realm(runtime, &entry.runtime, value)
  }))
}

pub(super) fn set(
  object: *const Object,
  key: *const Value,
  value: *const Value,
) -> Option<Result<(), String>> {
  let entry = binding(object)?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let key = into_realm(runtime, &entry.runtime, key)?;
    let value = into_realm(runtime, &entry.runtime, value)?;
    runtime.realm_set(entry.value, key, value)
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
    let value = runtime.realm_call(entry.value, receiver, array)?;
    from_realm(runtime, &entry.runtime, value)
  }))
}

pub(super) fn report(error: String) {
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
