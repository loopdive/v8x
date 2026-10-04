//! Deno's host extension, installed before its primordial snapshot.
//! Frames describe the real Wasmtime stack. JS source maps and identity-based
//! constructorOpt trimming are not yet available; no source locations are invented.
use super::*;

pub(super) fn install(
  access: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  global: RealmValue,
) -> Result<(), String> {
  let key = access.realm_string(&"Error".encode_utf16().collect::<Vec<_>>())?;
  let error = access.realm_get(global, key)?;
  if access.realm_kind(error)? != 6 {
    return Err("Deno realm Error constructor is not callable".to_string());
  }
  let capture_key = access
    .realm_string(&"captureStackTrace".encode_utf16().collect::<Vec<_>>())?;
  let existing = access.realm_get(error, capture_key)?;
  // Preserve any extension the embedder installed before bootstrap.
  if access.realm_kind(existing)? != 0 {
    return Ok(());
  }
  let isolate = current_isolate();
  let name = new_string(isolate, "captureStackTrace".to_string());
  let function =
    allocate_function(isolate, capture, ptr::null(), Vec::new(), name);
  let value = host_callbacks::allocate(access, owner, function)?;
  let name_key =
    access.realm_string(&"name".encode_utf16().collect::<Vec<_>>())?;
  let name = access
    .realm_string(&"captureStackTrace".encode_utf16().collect::<Vec<_>>())?;
  access.realm_define_data(value, name_key, name, 3)?;
  let length_key =
    access.realm_string(&"length".encode_utf16().collect::<Vec<_>>())?;
  let length = access.realm_number(2.0)?;
  access.realm_define_data(value, length_key, length, 3)?;
  access.realm_define_data(error, capture_key, value, 2)
}

unsafe extern "C" fn capture(
  info: *const crate::function::FunctionCallbackInfo,
) {
  let Some(info) = (unsafe { callback_info(info) }) else {
    return;
  };
  let target = info.args.first().copied().unwrap_or(ptr::null());
  let result = capture_on(target);
  if let Err(error) = result {
    let message = new_string(current_isolate(), error);
    record_exception(current_isolate(), allocate_error(message, "TypeError"));
  }
}

fn capture_on(target: *const Value) -> Result<(), String> {
  let entry = binding(target.cast())
    .ok_or("captureStackTrace target must be a realm object")?;
  callback_access::with_owner(&entry.runtime, |access| {
    if !matches!(access.realm_kind(entry.value)?, 5 | 6 | 9) {
      return Err("captureStackTrace target must be an object".to_string());
    }
    // Capture before bridge property reads create temporary Wasm frames.
    let frames = access.realm_capture_stack()?;
    let mut heading = Vec::new();
    for (property, default) in [("name", "Error"), ("message", "")] {
      let key =
        access.realm_string(&property.encode_utf16().collect::<Vec<_>>())?;
      let value = access.realm_get(entry.value, key)?;
      heading.push(if access.realm_kind(value)? == 4 {
        String::from_utf16_lossy(&access.realm_as_utf16(value)?)
      } else {
        default.to_string()
      });
    }
    let mut stack = if heading[1].is_empty() {
      heading[0].clone()
    } else {
      format!("{}: {}", heading[0], heading[1])
    };
    for (index, name) in frames {
      stack.push_str(&format!(
        "\n    at {} (wasm-function[{index}])",
        name.as_deref().unwrap_or("<anonymous>")
      ));
    }
    let key =
      access.realm_string(&"stack".encode_utf16().collect::<Vec<_>>())?;
    let value =
      access.realm_string(&stack.encode_utf16().collect::<Vec<_>>())?;
    access.realm_define_data(entry.value, key, value, 2)
  })
}
