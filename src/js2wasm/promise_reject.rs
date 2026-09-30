//! Native Promise rejection notifications through rusty_v8's three-word ABI.
use super::*;
use crate::{PromiseRejectEvent, PromiseRejectMessage};

pub(super) fn notify(
  isolate: *mut RealIsolate,
  promise: *const Promise,
  value: *const Value,
  event: PromiseRejectEvent,
) {
  if isolate.is_null() {
    return;
  }
  // Copy the callback before reentering user code. Never retain an isolate or
  // Promise state borrow across notification: the callback may attach handlers.
  let callback = unsafe { isolate_state(isolate) }.promise_reject_callback;
  if let Some(callback) = callback {
    let words = [promise as usize, value as usize, event as usize];
    let message =
      unsafe { std::mem::transmute::<[usize; 3], PromiseRejectMessage>(words) };
    unsafe { callback(message) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__PromiseRejectMessage__GetPromise(
  message: *const PromiseRejectMessage,
) -> *const Promise {
  if message.is_null() {
    return ptr::null();
  }
  unsafe { *message.cast::<usize>() as *const Promise }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__PromiseRejectMessage__GetValue(
  message: *const PromiseRejectMessage,
) -> *const Value {
  if message.is_null() {
    return ptr::null();
  }
  unsafe { *message.cast::<usize>().add(1) as *const Value }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__PromiseRejectMessage__GetEvent(
  message: *const PromiseRejectMessage,
) -> PromiseRejectEvent {
  if message.is_null() {
    return PromiseRejectEvent::PromiseRejectWithNoHandler;
  }
  match unsafe { *message.cast::<usize>().add(2) } {
    1 => PromiseRejectEvent::PromiseHandlerAddedAfterReject,
    2 => PromiseRejectEvent::PromiseRejectAfterResolved,
    3 => PromiseRejectEvent::PromiseResolveAfterResolved,
    _ => PromiseRejectEvent::PromiseRejectWithNoHandler,
  }
}
