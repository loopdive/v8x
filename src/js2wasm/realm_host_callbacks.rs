use super::*;

#[derive(Clone)]
pub(crate) struct HostCallbackBinding {
  owner: Rc<RefCell<DenoRuntime>>,
  realm_id: usize,
  function: *const crate::Function,
}

pub(super) fn allocate(
  access: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  function: *const crate::Function,
) -> Result<RealmValue, String> {
  let isolate = current_isolate();
  if isolate.is_null() {
    return Err("host callback has no isolate".to_string());
  }
  let callbacks = &mut unsafe { isolate_state(isolate) }.realm_callbacks;
  let id = callbacks.len();
  if id > 9_007_199_254_740_991 {
    return Err("host callback registry exhausted".to_string());
  }
  callbacks.push(HostCallbackBinding {
    owner: owner.clone(),
    realm_id: access.realm_id(),
    function,
  });
  access.realm_handle("__v8x_value_host_function", &[id as f64])
}

pub(crate) fn invoke_host(
  access: &mut dyn RealmAccess,
  id: f64,
  receiver: f64,
  args: f64,
) -> Result<f64, String> {
  if !id.is_finite()
    || id < 0.0
    || id.fract() != 0.0
    || id > 9_007_199_254_740_991.0
  {
    return Err("invalid host callback id".to_string());
  }
  let isolate = current_isolate();
  if isolate.is_null() {
    return Err("host callback has no isolate".to_string());
  }
  let callback = unsafe { isolate_state(isolate) }
    .realm_callbacks
    .get(id as usize)
    .cloned()
    .ok_or_else(|| "unknown host callback id".to_string())?;
  if callback.realm_id != access.realm_id() {
    return Err("host callback belongs to a different realm".to_string());
  }
  let receiver = access.realm_from_handle(receiver)?;
  let args = access.realm_from_handle(args)?;
  callback_access::with_active(&callback.owner, access, || {
    let (receiver, arguments) =
      callback_access::with_owner(&callback.owner, |access| {
        if access.realm_kind(args)? != 9 {
          return Err("host callback arguments must be an array".to_string());
        }
        let key =
          access.realm_string(&"length".encode_utf16().collect::<Vec<_>>())?;
        let length = access.realm_get(args, key)?;
        let length = access.realm_as_number(length)?;
        if !length.is_finite()
          || length < 0.0
          || length.fract() != 0.0
          || length > i32::MAX as f64
        {
          return Err("invalid host callback argument count".to_string());
        }
        let mut arguments = Vec::new();
        arguments
          .try_reserve(length as usize)
          .map_err(|e| format!("allocate callback arguments: {e}"))?;
        for index in 0..length as usize {
          let key = access.realm_string(
            &index.to_string().encode_utf16().collect::<Vec<_>>(),
          )?;
          let value = access.realm_get(args, key)?;
          arguments.push(from_realm(access, &callback.owner, value)?);
        }
        Ok((from_realm(access, &callback.owner, receiver)?, arguments))
      })?;
    // Do not hold the callback access lease while invoking Rust. Public
    // Object/Function APIs used by that callback can safely acquire it again.
    let mut caught = [0_usize; 6];
    v8__TryCatch__CONSTRUCT(caught.as_mut_ptr(), isolate);
    if std::env::var_os("V8X_JS2WASM_TRACE_HOST").is_some() {
      let name = match unsafe { heap_value(callback.function) } {
        Some(HeapValue::Function(state)) => {
          unsafe { string_value(state.name) }.unwrap_or("<unnamed>")
        }
        _ => "<invalid>",
      };
      eprintln!(
        "v8x/js2wasm: host callback {name} ({} arguments)",
        arguments.len()
      );
    }
    let result =
      invoke_native_callback(callback.function, receiver, &arguments, false);
    let exception = v8__TryCatch__Exception(caught.as_ptr());
    v8__TryCatch__Reset(caught.as_mut_ptr());
    v8__TryCatch__DESTRUCT(caught.as_mut_ptr());
    let thrown = !exception.is_null();
    let value = if thrown { exception } else { result };
    if value.is_null() {
      return Err("host callback returned an empty handle".to_string());
    }
    callback_access::with_owner(&callback.owner, |access| {
      let value = into_realm(access, &callback.owner, value)?;
      let handle = access.realm_check(value)?;
      Ok(if thrown { -handle - 1.0 } else { handle })
    })
  })
}
