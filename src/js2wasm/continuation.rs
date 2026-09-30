use super::*;

// V8's continuation-preserved data is isolate state, not context-local state.
// The isolate value arena keeps the selected value alive.
pub(super) fn get(isolate: *mut RealIsolate) -> *const Value {
  let value = unsafe { isolate_state(isolate) }.continuation_data;
  if value.is_null() {
    v8__Undefined(isolate).cast()
  } else {
    value
  }
}

pub(super) fn set(isolate: *mut RealIsolate, value: *const Value) {
  unsafe { isolate_state(isolate) }.continuation_data = value;
}

pub(super) struct Restore {
  isolate: *mut RealIsolate,
  previous: *const Value,
}

pub(super) fn enter(isolate: *mut RealIsolate, value: *const Value) -> Restore {
  let previous = get(isolate);
  set(isolate, value);
  Restore { isolate, previous }
}

impl Drop for Restore {
  fn drop(&mut self) {
    set(self.isolate, self.previous);
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__GetContinuationPreservedEmbedderData(
  isolate: *mut RealIsolate,
) -> *const Value {
  if isolate.is_null() {
    return ptr::null();
  }
  get(isolate)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__SetContinuationPreservedEmbedderData(
  isolate: *mut RealIsolate,
  value: *const Value,
) {
  if !isolate.is_null() {
    set(isolate, value);
  }
}

unsafe extern "C" fn extras_get(
  info: *const crate::function::FunctionCallbackInfo,
) {
  if let Some(info) = unsafe { callback_info(info) } {
    *info.return_slot = get(info.isolate);
  }
}

unsafe extern "C" fn extras_set(
  info: *const crate::function::FunctionCallbackInfo,
) {
  if let Some(info) = unsafe { callback_info(info) } {
    set(
      info.isolate,
      info.args.first().copied().unwrap_or(ptr::null()),
    );
    *info.return_slot = v8__Undefined(info.isolate).cast();
  }
}

pub(super) fn install(isolate: *mut RealIsolate, extras: *const Object) {
  for (name, callback) in [
    (
      "getContinuationPreservedEmbedderData",
      extras_get as crate::FunctionCallback,
    ),
    (
      "setContinuationPreservedEmbedderData",
      extras_set as crate::FunctionCallback,
    ),
  ] {
    let key = new_string(isolate, name.to_string());
    let function =
      allocate_function(isolate, callback, ptr::null(), Vec::new(), key);
    properties_mut(extras).unwrap().push(TemplateProperty {
      key: key.cast(),
      value: function.cast(),
      attributes: 0,
    });
  }
}
