use super::*;

// ToBoolean never invokes JavaScript conversion hooks, even for objects.
fn truthiness(value: &HeapValue) -> bool {
  match value {
    HeapValue::Undefined | HeapValue::Null => false,
    HeapValue::Boolean(value) => *value,
    HeapValue::Number(value) => *value != 0.0 && !value.is_nan(),
    HeapValue::BigInt(value) => *value != 0,
    HeapValue::String(value) => !value.is_empty(),
    HeapValue::Object(_)
    | HeapValue::Array(_)
    | HeapValue::ArrayBuffer(_)
    | HeapValue::TypedArray(_)
    | HeapValue::Function(_)
    | HeapValue::Promise(_)
    | HeapValue::PromiseResolver(_)
    | HeapValue::Symbol(_)
    | HeapValue::Error { .. }
    | HeapValue::External(_) => true,
    // Internal Data handles cannot be passed through the public Value API.
    HeapValue::Context(_)
    | HeapValue::Module(_)
    | HeapValue::Script(_)
    | HeapValue::ObjectTemplate(_)
    | HeapValue::FunctionTemplate(_)
    | HeapValue::FixedArray(_)
    | HeapValue::PrimitiveArray(_)
    | HeapValue::ModuleRequest(_)
    | HeapValue::UnboundModuleScript(_)
    | HeapValue::Private(_) => false,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__BooleanValue(
  value: *const Value,
  _isolate: *mut RealIsolate,
) -> bool {
  unsafe { heap_value(value) }.is_some_and(truthiness)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__ToBoolean(
  value: *const Value,
  isolate: *mut RealIsolate,
) -> *const Boolean {
  let Some(value) = (unsafe { heap_value(value) }) else {
    return ptr::null();
  };
  if isolate.is_null() {
    return ptr::null();
  }
  v8__Boolean__New(isolate, truthiness(value))
}
