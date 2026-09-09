use super::*;
use crate::Private;

pub(super) struct PrivateEntry {
  object: *const Object,
  key: *const Private,
  value: *const Value,
}

fn valid(
  object: *const Object,
  context: *const Context,
  key: *const Private,
) -> bool {
  !current_isolate().is_null()
    && matches!(unsafe { heap_value(context) }, Some(HeapValue::Context(_)))
    && v8__Value__IsObject(object.cast())
    && matches!(unsafe { heap_value(key) }, Some(HeapValue::Private(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Private__New(
  isolate: *mut RealIsolate,
  name: *const V8String,
) -> *const Private {
  allocate(isolate, HeapValue::Private(name))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Private__ForApi(
  isolate: *mut RealIsolate,
  name: *const V8String,
) -> *const Private {
  if isolate.is_null() {
    return ptr::null();
  }
  let description = unsafe { string_value(name) }.map(str::to_owned);
  if let Some((_, key)) = unsafe { isolate_state(isolate) }
    .private_registry
    .iter()
    .find(|(text, _)| *text == description)
  {
    return *key;
  }
  let key = v8__Private__New(isolate, name);
  unsafe { isolate_state(isolate) }
    .private_registry
    .push((description, key));
  key
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Private__Name(key: *const Private) -> *const Value {
  match unsafe { heap_value(key) } {
    Some(HeapValue::Private(name)) if !name.is_null() => name.cast(),
    Some(HeapValue::Private(_)) => v8__Undefined(current_isolate()).cast(),
    _ => ptr::null(),
  }
}

// Private values live outside JavaScript property tables, so enumeration,
// prototypes and same-spelling public properties cannot observe them.
#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__GetPrivate(
  object: *const Object,
  context: *const Context,
  key: *const Private,
) -> *const Value {
  if !valid(object, context, key) {
    return ptr::null();
  }
  unsafe { isolate_state(current_isolate()) }
    .private_properties
    .iter()
    .find(|entry| entry.object == object && entry.key == key)
    .map(|entry| entry.value)
    .unwrap_or_else(|| v8__Undefined(current_isolate()).cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__SetPrivate(
  object: *const Object,
  context: *const Context,
  key: *const Private,
  value: *const Value,
) -> MaybeBool {
  if !valid(object, context, key) || value.is_null() {
    return MaybeBool::Nothing;
  }
  let entries =
    &mut unsafe { isolate_state(current_isolate()) }.private_properties;
  if let Some(entry) = entries
    .iter_mut()
    .find(|entry| entry.object == object && entry.key == key)
  {
    entry.value = value;
  } else {
    entries.push(PrivateEntry { object, key, value });
  }
  MaybeBool::JustTrue
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__HasPrivate(
  object: *const Object,
  context: *const Context,
  key: *const Private,
) -> MaybeBool {
  if !valid(object, context, key) {
    return MaybeBool::Nothing;
  }
  if unsafe { isolate_state(current_isolate()) }
    .private_properties
    .iter()
    .any(|entry| entry.object == object && entry.key == key)
  {
    MaybeBool::JustTrue
  } else {
    MaybeBool::JustFalse
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__DeletePrivate(
  object: *const Object,
  context: *const Context,
  key: *const Private,
) -> MaybeBool {
  if !valid(object, context, key) {
    return MaybeBool::Nothing;
  }
  unsafe { isolate_state(current_isolate()) }
    .private_properties
    .retain(|entry| entry.object != object || entry.key != key);
  MaybeBool::JustTrue
}
