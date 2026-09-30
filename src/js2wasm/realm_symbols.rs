use super::*;

fn remember(
  owner: &Rc<RefCell<DenoRuntime>>,
  host: *const Value,
  value: RealmValue,
) {
  unsafe { isolate_state(current_isolate()) }
    .realm_objects
    .push(RealmObjectBinding {
      host: host.cast(),
      runtime: owner.clone(),
      value,
    });
}

pub(super) fn into_realm(
  runtime: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  host: *const Value,
) -> Result<RealmValue, String> {
  let isolate = current_isolate();
  let state = unsafe { isolate_state(isolate) };
  if let Some(entry) = state.realm_objects.iter().find(|entry| {
    entry.host == host.cast() && Rc::ptr_eq(owner, &entry.runtime)
  }) {
    return Ok(entry.value);
  }
  let (kind, text) = if state.iterator_symbol.cast::<Value>() == host {
    (2.0, None)
  } else if let Some((key, _)) = state
    .symbol_registry
    .iter()
    .find(|(_, symbol)| symbol.cast::<Value>() == host)
  {
    (1.0, Some(key.clone()))
  } else {
    match unsafe { heap_value(host) } {
      Some(HeapValue::Symbol(symbol)) => (
        0.0,
        symbol
          .description
          .and_then(|value| unsafe { string_value(value) }.map(str::to_owned)),
      ),
      _ => return Err("expected native Symbol".into()),
    }
  };
  // Snapshot native state before entering Wasm, which may re-enter Rust.
  let text = match text {
    Some(text) => {
      runtime.realm_string(&text.encode_utf16().collect::<Vec<_>>())?
    }
    None => runtime.realm_undefined(),
  };
  let value = runtime.realm_handle(
    "__v8x_value_symbol_create",
    &[kind, runtime.realm_check(text)?],
  )?;
  remember(owner, host, value);
  Ok(value)
}

pub(super) fn from_realm(
  runtime: &mut dyn RealmAccess,
  owner: &Rc<RefCell<DenoRuntime>>,
  value: RealmValue,
) -> Result<*const Value, String> {
  let isolate = current_isolate();
  if let Some(entry) = unsafe { isolate_state(isolate) }
    .realm_objects
    .iter()
    .find(|entry| entry.value == value && Rc::ptr_eq(owner, &entry.runtime))
  {
    return Ok(entry.host.cast());
  }
  let handle = runtime.realm_check(value)?;
  let kind = runtime.realm_raw("__v8x_value_symbol_kind", &[handle], true)?;
  let symbol = if kind == 2.0 {
    v8__Symbol__GetIterator(isolate)
  } else {
    let text = runtime.realm_handle("__v8x_value_symbol_text", &[handle])?;
    let description = match runtime.realm_kind(text)? {
      0 if kind == 0.0 => ptr::null(),
      4 => {
        let text = String::from_utf16(&runtime.realm_as_utf16(text)?)
          .map_err(|_| "Symbol text contains unpaired UTF-16")?;
        new_string(isolate, text)
      }
      _ => return Err("invalid realm Symbol text".into()),
    };
    if kind == 1.0 {
      v8__Symbol__For(isolate, description)
    } else if kind == 0.0 {
      v8__Symbol__New(isolate, description)
    } else {
      return Err("invalid realm Symbol kind".into());
    }
  };
  remember(owner, symbol.cast(), value);
  Ok(symbol.cast())
}
