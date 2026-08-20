//! Engine-free v8x vertical slice backed by js2wasm and Wasmtime.
//!
//! This module intentionally implements only the rusty_v8 ABI exercised by
//! source-text module compilation. Values are stable Rust allocations owned by
//! an isolate; no JavaScript interpreter is linked into this backend.

#![allow(non_snake_case, unused)]

// These helpers are engine-independent despite living under the QuickJS
// backend today. Reuse their real Rust implementation so deno_core's string
// conversion path never needs an interpreter or a native simdutf library.
#[path = "../quickjs/simdutf.rs"]
mod simdutf;

use crate::isolate::ModuleImportPhase;
use crate::module::{
  ModuleStatus, ResolveModuleCallback, ResolveModuleCallbackRet,
  ResolveSourceCallback,
};
use crate::script::ScriptOrigin;
use crate::script_compiler::{
  CachedData, CompileOptions, NoCacheReason, Source,
};
use crate::string::{NewStringType, ValueView};
use crate::support::{MaybeBool, SharedPtrBase, UniquePtr, int, long};
use crate::{
  Allocator, Array, Boolean, Context, Data, External, FixedArray, Int32,
  Integer, Module, Number, Object, Platform, Primitive, RealIsolate, Script,
  String as V8String, Uint32, Value,
};
use std::collections::HashSet;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::{MaybeUninit, size_of};
use std::ptr;
use std::sync::atomic::{AtomicUsize, Ordering};

unsafe extern "C" {
  fn v8__Platform__CustomPlatform__BASE__DROP(context: *mut c_void);
}

const STATUS_UNINSTANTIATED: u8 = 0;
const STATUS_INSTANTIATING: u8 = 1;
const STATUS_INSTANTIATED: u8 = 2;
const STATUS_EVALUATING: u8 = 3;
const STATUS_EVALUATED: u8 = 4;
const STATUS_ERRORED: u8 = 5;

struct ModuleState {
  status: u8,
  source: String,
  specifier: String,
  imports: Vec<String>,
  dependencies: Vec<*const Module>,
  runtime: Option<crate::js2wasm_spike::DenoRuntime>,
}

struct ScriptState {
  source: String,
  specifier: String,
}

#[derive(Clone, Copy)]
struct TemplateProperty {
  key: *const crate::Name,
  value: *const Data,
  attributes: u32,
}

struct ObjectTemplateState {
  properties: Vec<TemplateProperty>,
  internal_field_count: int,
}

struct FunctionTemplateState {
  callback: crate::FunctionCallback,
  data: *const Value,
  properties: Vec<TemplateProperty>,
  prototype_template: *const crate::ObjectTemplate,
  instance_template: *const crate::ObjectTemplate,
}

struct ObjectState {
  properties: Vec<TemplateProperty>,
  internal_fields: Vec<*mut c_void>,
}

struct ArrayState {
  elements: Vec<*const Value>,
  properties: Vec<TemplateProperty>,
}

struct FunctionState {
  callback: crate::FunctionCallback,
  data: *const Value,
  properties: Vec<TemplateProperty>,
}

struct CallbackInfoState {
  isolate: *mut RealIsolate,
  this: *const Value,
  data: *const Value,
  new_target: *const Value,
  is_construct: bool,
  args: Vec<*const Value>,
  return_slot: Box<*const Value>,
}

struct ContextState {
  global: *const Object,
  extras: *const Object,
  embedder_data: Vec<*mut c_void>,
  deno_core_bootstrap: Option<crate::js2wasm_spike::DenoRuntime>,
  deno_core_bootstrap_phase: usize,
}

enum HeapValue {
  String(String),
  Context(ContextState),
  Object(ObjectState),
  Array(ArrayState),
  Function(FunctionState),
  Module(ModuleState),
  Script(ScriptState),
  ObjectTemplate(ObjectTemplateState),
  FunctionTemplate(FunctionTemplateState),
  FixedArray,
  Promise,
  Error { name: &'static str, message: String },
  External(*mut c_void),
  Boolean(bool),
  Number(f64),
  Null,
  Undefined,
}

#[repr(C)]
pub(crate) struct RawReturnValue(usize);

#[repr(C)]
pub(crate) struct RawFunctionCallbackInfoParts {
  isolate: *mut RealIsolate,
  return_value: usize,
  data: *const Value,
  length: int,
}

struct IsolateState {
  values: Vec<*mut HeapValue>,
  contexts: Vec<*const Context>,
  data_slots: [*mut c_void; 4],
  microtasks_policy: crate::MicrotasksPolicy,
}

struct PlatformState {
  custom_context: *mut c_void,
}

thread_local! {
  static ENTERED_ISOLATES: std::cell::RefCell<Vec<*mut RealIsolate>> =
    const { std::cell::RefCell::new(Vec::new()) };
}

fn current_isolate() -> *mut RealIsolate {
  ENTERED_ISOLATES
    .with(|entered| entered.borrow().last().copied().unwrap_or(ptr::null_mut()))
}

unsafe fn isolate_state<'a>(isolate: *mut RealIsolate) -> &'a mut IsolateState {
  unsafe { &mut *(isolate as *mut IsolateState) }
}

fn allocate<T>(isolate: *mut RealIsolate, value: HeapValue) -> *const T {
  if isolate.is_null() {
    return ptr::null();
  }
  let value = Box::into_raw(Box::new(value));
  unsafe { isolate_state(isolate).values.push(value) };
  value.cast()
}

unsafe fn heap_value<'a, T>(value: *const T) -> Option<&'a HeapValue> {
  unsafe { (value as *const HeapValue).as_ref() }
}

unsafe fn heap_value_mut<'a, T>(value: *const T) -> Option<&'a mut HeapValue> {
  unsafe { (value as *mut HeapValue).as_mut() }
}

unsafe fn module_state<'a>(
  module: *const Module,
) -> Option<&'a mut ModuleState> {
  match unsafe { (module as *mut HeapValue).as_mut() } {
    Some(HeapValue::Module(state)) => Some(state),
    _ => None,
  }
}

unsafe fn string_value<'a, T>(value: *const T) -> Option<&'a str> {
  match unsafe { heap_value(value) } {
    Some(HeapValue::String(value)) => Some(value),
    _ => None,
  }
}

fn current_context() -> *const Context {
  let isolate = current_isolate();
  if isolate.is_null() {
    return ptr::null();
  }
  unsafe {
    isolate_state(isolate)
      .contexts
      .last()
      .copied()
      .unwrap_or(ptr::null())
  }
}

fn allocate_error(
  message: *const V8String,
  name: &'static str,
) -> *const Value {
  let Some(message) = (unsafe { string_value(message) }) else {
    return ptr::null();
  };
  allocate(
    current_isolate(),
    HeapValue::Error {
      name,
      message: message.to_owned(),
    },
  )
}

fn object_from_template(
  isolate: *mut RealIsolate,
  template: *const crate::ObjectTemplate,
) -> *const Object {
  let (properties, internal_field_count) = match unsafe { heap_value(template) }
  {
    Some(HeapValue::ObjectTemplate(state)) => (
      state.properties.clone(),
      state.internal_field_count.max(0) as usize,
    ),
    _ => (Vec::new(), 0),
  };
  allocate(
    isolate,
    HeapValue::Object(ObjectState {
      properties,
      internal_fields: vec![ptr::null_mut(); internal_field_count],
    }),
  )
}

fn new_object(isolate: *mut RealIsolate) -> *const Object {
  allocate(
    isolate,
    HeapValue::Object(ObjectState {
      properties: Vec::new(),
      internal_fields: Vec::new(),
    }),
  )
}

fn same_property_key(left: *const Data, right: *const Data) -> bool {
  std::ptr::addr_eq(left, right)
    || match (unsafe { string_value(left) }, unsafe {
      string_value(right)
    }) {
      (Some(left), Some(right)) => left == right,
      _ => false,
    }
}

fn properties<T>(value: *const T) -> Option<&'static Vec<TemplateProperty>> {
  match unsafe { heap_value(value) } {
    Some(HeapValue::Object(state)) => Some(&state.properties),
    Some(HeapValue::Array(state)) => Some(&state.properties),
    Some(HeapValue::Function(state)) => Some(&state.properties),
    _ => None,
  }
}

fn properties_mut<T>(
  value: *const T,
) -> Option<&'static mut Vec<TemplateProperty>> {
  match unsafe { heap_value_mut(value) } {
    Some(HeapValue::Object(state)) => Some(&mut state.properties),
    Some(HeapValue::Array(state)) => Some(&mut state.properties),
    Some(HeapValue::Function(state)) => Some(&mut state.properties),
    _ => None,
  }
}

fn allocate_json_value(
  isolate: *mut RealIsolate,
  value: serde_json::Value,
) -> *const Value {
  match value {
    serde_json::Value::Null => allocate(isolate, HeapValue::Null),
    serde_json::Value::Bool(value) => {
      allocate(isolate, HeapValue::Boolean(value))
    }
    serde_json::Value::Number(value) => value
      .as_f64()
      .map(|value| allocate(isolate, HeapValue::Number(value)))
      .unwrap_or(ptr::null()),
    serde_json::Value::String(value) => {
      allocate(isolate, HeapValue::String(value))
    }
    serde_json::Value::Array(values) => {
      let elements = values
        .into_iter()
        .map(|value| allocate_json_value(isolate, value))
        .collect();
      allocate(
        isolate,
        HeapValue::Array(ArrayState {
          elements,
          properties: Vec::new(),
        }),
      )
    }
    serde_json::Value::Object(values) => {
      let properties = values
        .into_iter()
        .map(|(key, value)| TemplateProperty {
          key: new_string(isolate, key).cast(),
          value: allocate_json_value(isolate, value).cast(),
          attributes: 0,
        })
        .collect();
      allocate(
        isolate,
        HeapValue::Object(ObjectState {
          properties,
          internal_fields: Vec::new(),
        }),
      )
    }
  }
}

fn heap_to_json_value(
  value: *const Value,
  ancestors: &mut HashSet<usize>,
) -> Option<serde_json::Value> {
  match unsafe { heap_value(value) }? {
    HeapValue::Null => Some(serde_json::Value::Null),
    HeapValue::Boolean(value) => Some(serde_json::Value::Bool(*value)),
    HeapValue::Number(value) => {
      serde_json::Number::from_f64(*value).map(serde_json::Value::Number)
    }
    HeapValue::String(value) => Some(serde_json::Value::String(value.clone())),
    HeapValue::Array(state) => {
      let identity = value.addr();
      if !ancestors.insert(identity) {
        return None;
      }
      let result = state
        .elements
        .iter()
        .map(|value| {
          heap_to_json_value(*value, ancestors)
            .unwrap_or(serde_json::Value::Null)
        })
        .collect();
      ancestors.remove(&identity);
      Some(serde_json::Value::Array(result))
    }
    HeapValue::Object(state) => {
      let identity = value.addr();
      if !ancestors.insert(identity) {
        return None;
      }
      let mut result = serde_json::Map::new();
      for property in &state.properties {
        let Some(key) = (unsafe { string_value(property.key) }) else {
          continue;
        };
        if let Some(value) =
          heap_to_json_value(property.value.cast(), ancestors)
        {
          result.insert(key.to_owned(), value);
        }
      }
      ancestors.remove(&identity);
      Some(serde_json::Value::Object(result))
    }
    HeapValue::Undefined
    | HeapValue::Function(_)
    | HeapValue::External(_)
    | HeapValue::Error { .. }
    | HeapValue::Context(_)
    | HeapValue::Module(_)
    | HeapValue::Script(_)
    | HeapValue::ObjectTemplate(_)
    | HeapValue::FunctionTemplate(_)
    | HeapValue::FixedArray
    | HeapValue::Promise => None,
  }
}

// --- Platform and isolate ownership ---------------------------------------

fn new_platform(custom_context: *mut c_void) -> *mut Platform {
  Box::into_raw(Box::new(PlatformState { custom_context })).cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__V8__SetFlagsFromString(
  _flags: *const u8,
  _length: usize,
) {
  // Deno supplies V8 optimizer, parser, and GC flags here. The js2wasm
  // backend has none of those runtime subsystems: source semantics are fixed
  // at AOT compile time and memory is managed by WasmGC. Accepting the flags
  // is therefore an intentional compatibility no-op, not a partial parser.
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__V8__InitializePlatform(_platform: *mut Platform) {}

#[unsafe(no_mangle)]
pub extern "C" fn v8__V8__Initialize() {}

#[unsafe(no_mangle)]
pub extern "C" fn v8__V8__Dispose() -> bool {
  true
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__V8__DisposePlatform() {}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Platform__NewDefaultPlatform(
  _thread_pool_size: c_int,
  _idle_task_support: bool,
) -> *mut Platform {
  new_platform(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Platform__NewUnprotectedDefaultPlatform(
  thread_pool_size: c_int,
  idle_task_support: bool,
) -> *mut Platform {
  v8__Platform__NewDefaultPlatform(thread_pool_size, idle_task_support)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Platform__NewSingleThreadedDefaultPlatform(
  idle_task_support: bool,
) -> *mut Platform {
  v8__Platform__NewDefaultPlatform(0, idle_task_support)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Platform__NewCustomPlatform(
  _thread_pool_size: c_int,
  _idle_task_support: bool,
  _unprotected: bool,
  context: *mut c_void,
) -> *mut Platform {
  // `Platform::new_custom` transfers a double-boxed PlatformImpl through this
  // pointer. Keep it alive for the platform lifetime even though an AOT
  // module does not currently post V8 foreground tasks.
  new_platform(context)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Platform__PumpMessageLoop(
  _platform: *mut Platform,
  _isolate: *mut c_void,
  _wait_for_work: bool,
) -> bool {
  // js2wasm has no engine-owned background queue. Deno's ordinary op/event
  // loop remains in deno_core and is driven independently of this hook.
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Platform__DELETE(platform: *mut Platform) {
  if !platform.is_null() {
    let platform = unsafe { Box::from_raw(platform.cast::<PlatformState>()) };
    if !platform.custom_context.is_null() {
      // Match the ownership transfer in rusty_v8's Platform::new_custom.
      unsafe {
        v8__Platform__CustomPlatform__BASE__DROP(platform.custom_context)
      };
    }
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Platform__NotifyIsolateShutdown(
  _platform: *mut Platform,
  _isolate: *mut c_void,
) {
}

#[repr(C)]
struct SharedRepr {
  object: *mut c_void,
  references: *mut AtomicUsize,
}

unsafe fn shared_from_repr<T: crate::support::Shared>(
  repr: SharedRepr,
) -> SharedPtrBase<T> {
  unsafe { std::mem::transmute(repr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__Platform__CONVERT__std__unique_ptr(
  unique: UniquePtr<Platform>,
) -> SharedPtrBase<Platform> {
  let object: *mut c_void = unique.into_raw().cast();
  let references = if object.is_null() {
    ptr::null_mut()
  } else {
    Box::into_raw(Box::new(AtomicUsize::new(1)))
  };
  unsafe { shared_from_repr(SharedRepr { object, references }) }
}

unsafe fn shared_repr<T: crate::support::Shared>(
  shared: *const SharedPtrBase<T>,
) -> SharedRepr {
  if shared.is_null() {
    return SharedRepr {
      object: ptr::null_mut(),
      references: ptr::null_mut(),
    };
  }
  unsafe { ptr::read_unaligned(shared.cast()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__Platform__get(
  shared: *const SharedPtrBase<Platform>,
) -> *mut Platform {
  unsafe { shared_repr(shared).object.cast() }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__Platform__COPY(
  shared: *const SharedPtrBase<Platform>,
) -> SharedPtrBase<Platform> {
  let repr = unsafe { shared_repr(shared) };
  if !repr.references.is_null() {
    unsafe { (*repr.references).fetch_add(1, Ordering::Relaxed) };
  }
  unsafe { shared_from_repr(repr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__Platform__reset(
  shared: *mut SharedPtrBase<Platform>,
) {
  let repr = unsafe { shared_repr(shared) };
  if !repr.references.is_null()
    && unsafe { (*repr.references).fetch_sub(1, Ordering::AcqRel) } == 1
  {
    v8__Platform__DELETE(repr.object.cast());
    unsafe { drop(Box::from_raw(repr.references)) };
  }
  if !shared.is_null() {
    unsafe { ptr::write_bytes(shared, 0, 1) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__Platform__use_count(
  shared: *const SharedPtrBase<Platform>,
) -> long {
  let references = unsafe { shared_repr(shared).references };
  if references.is_null() {
    0
  } else {
    unsafe { (*references).load(Ordering::Acquire) as long }
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__CreateParams__SIZEOF() -> usize {
  size_of::<crate::isolate_create_params::raw::CreateParams>()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__CreateParams__CONSTRUCT(
  params: *mut MaybeUninit<crate::isolate_create_params::raw::CreateParams>,
) {
  if params.is_null() {
    return;
  }
  unsafe {
    ptr::write_bytes(params.cast::<u8>(), 0, size_of_val(&*params));
    let params =
      &mut *params.cast::<crate::isolate_create_params::raw::CreateParams>();
    params.allow_atomics_wait = true;
  }
}

struct RustAllocator;

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__Allocator__NewDefaultAllocator()
-> *mut Allocator {
  Box::into_raw(Box::new(RustAllocator)).cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__Allocator__DELETE(
  allocator: *mut Allocator,
) {
  if !allocator.is_null() {
    unsafe { drop(Box::from_raw(allocator.cast::<RustAllocator>())) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__ArrayBuffer__Allocator__CONVERT__std__unique_ptr(
  unique: UniquePtr<Allocator>,
) -> SharedPtrBase<Allocator> {
  let object: *mut c_void = unique.into_raw().cast();
  let references = if object.is_null() {
    ptr::null_mut()
  } else {
    Box::into_raw(Box::new(AtomicUsize::new(1)))
  };
  unsafe { shared_from_repr(SharedRepr { object, references }) }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__ArrayBuffer__Allocator__get(
  shared: *const SharedPtrBase<Allocator>,
) -> *mut Allocator {
  unsafe { shared_repr(shared).object.cast() }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__ArrayBuffer__Allocator__COPY(
  shared: *const SharedPtrBase<Allocator>,
) -> SharedPtrBase<Allocator> {
  let repr = unsafe { shared_repr(shared) };
  if !repr.references.is_null() {
    unsafe { (*repr.references).fetch_add(1, Ordering::Relaxed) };
  }
  unsafe { shared_from_repr(repr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__ArrayBuffer__Allocator__reset(
  shared: *mut SharedPtrBase<Allocator>,
) {
  let repr = unsafe { shared_repr(shared) };
  if !repr.references.is_null()
    && unsafe { (*repr.references).fetch_sub(1, Ordering::AcqRel) } == 1
  {
    v8__ArrayBuffer__Allocator__DELETE(repr.object.cast());
    unsafe { drop(Box::from_raw(repr.references)) };
  }
  if !shared.is_null() {
    unsafe { ptr::write_bytes(shared, 0, 1) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__ArrayBuffer__Allocator__use_count(
  shared: *const SharedPtrBase<Allocator>,
) -> long {
  let references = unsafe { shared_repr(shared).references };
  if references.is_null() {
    0
  } else {
    unsafe { (*references).load(Ordering::Acquire) as long }
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__New(_params: *const c_void) -> *mut RealIsolate {
  Box::into_raw(Box::new(IsolateState {
    values: Vec::new(),
    contexts: Vec::new(),
    data_slots: [ptr::null_mut(); 4],
    microtasks_policy: crate::MicrotasksPolicy::Auto,
  }))
  .cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__Dispose(isolate: *mut RealIsolate) {
  if isolate.is_null() {
    return;
  }
  let mut state = unsafe { Box::from_raw(isolate.cast::<IsolateState>()) };
  for value in state.values.drain(..) {
    unsafe { drop(Box::from_raw(value)) };
  }
  ENTERED_ISOLATES.with(|entered| {
    entered
      .borrow_mut()
      .retain(|candidate| *candidate != isolate);
  });
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__Enter(isolate: *mut RealIsolate) {
  ENTERED_ISOLATES.with(|entered| entered.borrow_mut().push(isolate));
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__Exit(isolate: *mut RealIsolate) {
  ENTERED_ISOLATES.with(|entered| {
    let mut entered = entered.borrow_mut();
    if entered.last().copied() == Some(isolate) {
      entered.pop();
    }
  });
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__GetCurrent() -> *mut RealIsolate {
  current_isolate()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__GetNumberOfDataSlots(
  _isolate: *const RealIsolate,
) -> u32 {
  4
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__GetData(
  isolate: *const RealIsolate,
  slot: u32,
) -> *mut c_void {
  unsafe {
    isolate_state(isolate.cast_mut())
      .data_slots
      .get(slot as usize)
      .copied()
      .unwrap_or(ptr::null_mut())
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetData(
  isolate: *const RealIsolate,
  slot: u32,
  data: *mut c_void,
) {
  if let Some(target) = unsafe {
    isolate_state(isolate.cast_mut())
      .data_slots
      .get_mut(slot as usize)
  } {
    *target = data;
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetMicrotasksPolicy(
  isolate: *mut RealIsolate,
  policy: crate::MicrotasksPolicy,
) {
  if !isolate.is_null() {
    unsafe { isolate_state(isolate).microtasks_policy = policy };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__GetMicrotasksPolicy(
  isolate: *const RealIsolate,
) -> crate::MicrotasksPolicy {
  if isolate.is_null() {
    crate::MicrotasksPolicy::Auto
  } else {
    unsafe { isolate_state(isolate.cast_mut()).microtasks_policy }
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetCaptureStackTraceForUncaughtExceptions(
  _isolate: *mut RealIsolate,
  _capture: bool,
  _frame_limit: i32,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetPromiseRejectCallback(
  _isolate: *mut RealIsolate,
  _callback: crate::isolate::PromiseRejectCallback,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetPrepareStackTraceCallback(
  _isolate: *mut RealIsolate,
  _callback: crate::isolate::PrepareStackTraceCallback<'static>,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetHostInitializeImportMetaObjectCallback(
  _isolate: *mut RealIsolate,
  _callback: crate::isolate::HostInitializeImportMetaObjectCallback,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetHostImportModuleDynamicallyCallback(
  _isolate: *mut RealIsolate,
  _callback: crate::isolate::RawHostImportModuleDynamicallyCallback,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetHostImportModuleWithPhaseDynamicallyCallback(
  _isolate: *mut RealIsolate,
  _callback: crate::isolate::RawHostImportModuleWithPhaseDynamicallyCallback,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetWasmAsyncResolvePromiseCallback(
  _isolate: *mut RealIsolate,
  _callback: crate::isolate::WasmAsyncResolvePromiseCallback,
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__SetWasmStreamingCallback(
  _isolate: *mut RealIsolate,
  _callback: unsafe extern "C" fn(*const crate::function::FunctionCallbackInfo),
) {
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__GetCurrentContext(
  _isolate: *mut RealIsolate,
) -> *const Context {
  current_context()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__HandleScope__CONSTRUCT(
  scope: *mut usize,
  isolate: *mut RealIsolate,
) {
  if !scope.is_null() {
    unsafe { scope.write(isolate as usize) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__HandleScope__DESTRUCT(_scope: *mut usize) {}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Local__New(
  _isolate: *mut RealIsolate,
  value: *const Data,
) -> *const Data {
  value
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Global__New(
  _isolate: *mut RealIsolate,
  value: *const Data,
) -> *const Data {
  // Values are stable isolate-owned allocations in this backend, so a
  // persistent handle can directly retain the same address.
  value
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Global__Reset(_value: *const Data) {
  // The isolate owns the allocation. Reset only releases the logical handle;
  // there is no moving collector or separate persistent cell to destroy.
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__SnapshotCreator__DESTRUCT(_creator: *mut c_void) {}

#[unsafe(no_mangle)]
pub extern "C" fn v8__StartupData__data__DELETE(_data: *const c_char) {}

// --- Contexts and strings -------------------------------------------------

fn new_object_template(
  isolate: *mut RealIsolate,
) -> *const crate::ObjectTemplate {
  allocate(
    isolate,
    HeapValue::ObjectTemplate(ObjectTemplateState {
      properties: Vec::new(),
      internal_field_count: 0,
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionTemplate__New(
  isolate: *mut RealIsolate,
  callback: crate::FunctionCallback,
  data: *const Value,
  _signature: *const crate::Signature,
  _length: i32,
  _constructor_behavior: crate::ConstructorBehavior,
  _side_effect_type: crate::SideEffectType,
  _c_functions: *const crate::fast_api::CFunction,
  _c_functions_len: usize,
) -> *const crate::FunctionTemplate {
  let prototype_template = new_object_template(isolate);
  let instance_template = new_object_template(isolate);
  allocate(
    isolate,
    HeapValue::FunctionTemplate(FunctionTemplateState {
      callback,
      data,
      properties: Vec::new(),
      prototype_template,
      instance_template,
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionTemplate__PrototypeTemplate(
  template: *const crate::FunctionTemplate,
) -> *const crate::ObjectTemplate {
  match unsafe { heap_value(template) } {
    Some(HeapValue::FunctionTemplate(state)) => state.prototype_template,
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionTemplate__InstanceTemplate(
  template: *const crate::FunctionTemplate,
) -> *const crate::ObjectTemplate {
  match unsafe { heap_value(template) } {
    Some(HeapValue::FunctionTemplate(state)) => state.instance_template,
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ObjectTemplate__New(
  isolate: *mut RealIsolate,
  _constructor: *const crate::FunctionTemplate,
) -> *const crate::ObjectTemplate {
  new_object_template(isolate)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ObjectTemplate__InternalFieldCount(
  template: *const crate::ObjectTemplate,
) -> int {
  match unsafe { heap_value(template) } {
    Some(HeapValue::ObjectTemplate(state)) => state.internal_field_count,
    _ => 0,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ObjectTemplate__SetInternalFieldCount(
  template: *const crate::ObjectTemplate,
  count: int,
) {
  if let Some(HeapValue::ObjectTemplate(state)) =
    unsafe { heap_value_mut(template) }
  {
    state.internal_field_count = count.max(0);
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Template__Set(
  template: *const crate::Template,
  key: *const crate::Name,
  value: *const Data,
  attributes: crate::PropertyAttribute,
) {
  let property = TemplateProperty {
    key,
    value,
    attributes: unsafe { std::mem::transmute(attributes) },
  };
  match unsafe { heap_value_mut(template) } {
    Some(HeapValue::ObjectTemplate(state)) => state.properties.push(property),
    Some(HeapValue::FunctionTemplate(state)) => state.properties.push(property),
    _ => {}
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__New(
  isolate: *mut RealIsolate,
  template: *const c_void,
  global_object: *const c_void,
  _microtask_queue: *mut c_void,
) -> *const Context {
  let global = if global_object.is_null() {
    object_from_template(isolate, template.cast())
  } else {
    global_object.cast()
  };
  let console = new_object(isolate);
  let console_key = new_string(isolate, "console".to_string());
  let extras = new_object(isolate);
  if let Some(properties) = properties_mut(extras) {
    properties.push(TemplateProperty {
      key: console_key.cast(),
      value: console.cast(),
      attributes: 0,
    });
  }
  allocate(
    isolate,
    HeapValue::Context(ContextState {
      global,
      extras,
      embedder_data: vec![ptr::null_mut(); 4],
      deno_core_bootstrap: None,
      deno_core_bootstrap_phase: 0,
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__Global(
  context: *const Context,
) -> *const Object {
  match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state.global,
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__GetExtrasBindingObject(
  context: *const Context,
) -> *const Object {
  match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state.extras,
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__GetNumberOfEmbedderDataFields(
  context: *const Context,
) -> u32 {
  match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state.embedder_data.len() as u32,
    _ => 0,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__GetAlignedPointerFromEmbedderData(
  context: *const Context,
  index: int,
) -> *mut c_void {
  match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) if index >= 0 => state
      .embedder_data
      .get(index as usize)
      .copied()
      .unwrap_or(ptr::null_mut()),
    _ => ptr::null_mut(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__SetAlignedPointerInEmbedderData(
  context: *const Context,
  index: int,
  value: *mut c_void,
) {
  if index < 0 {
    return;
  }
  if let Some(HeapValue::Context(state)) = unsafe { heap_value_mut(context) } {
    let index = index as usize;
    if state.embedder_data.len() <= index {
      state.embedder_data.resize(index + 1, ptr::null_mut());
    }
    state.embedder_data[index] = value;
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ObjectTemplate__NewInstance(
  template: *const crate::ObjectTemplate,
  _context: *const Context,
) -> *const Object {
  object_from_template(current_isolate(), template)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__New(isolate: *mut RealIsolate) -> *const Object {
  new_object(isolate)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Array__New(
  isolate: *mut RealIsolate,
  length: int,
) -> *const Array {
  let undefined = v8__Undefined(isolate).cast();
  allocate(
    isolate,
    HeapValue::Array(ArrayState {
      elements: vec![undefined; length.max(0) as usize],
      properties: Vec::new(),
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Array__New_with_elements(
  isolate: *mut RealIsolate,
  elements: *const *const Value,
  length: usize,
) -> *const Array {
  if length > 0 && elements.is_null() {
    return ptr::null();
  }
  let elements = if length == 0 {
    Vec::new()
  } else {
    unsafe { std::slice::from_raw_parts(elements, length) }.to_vec()
  };
  allocate(
    isolate,
    HeapValue::Array(ArrayState {
      elements,
      properties: Vec::new(),
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Array__Length(array: *const Array) -> u32 {
  match unsafe { heap_value(array) } {
    Some(HeapValue::Array(state)) => {
      state.elements.len().try_into().unwrap_or(u32::MAX)
    }
    _ => 0,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__Get(
  object: *const Object,
  _context: *const Context,
  key: *const Value,
) -> *const Value {
  properties(object)
    .and_then(|properties| {
      properties
        .iter()
        .rev()
        .find(|property| same_property_key(property.key.cast(), key.cast()))
    })
    .map(|property| property.value.cast())
    .unwrap_or_else(|| v8__Undefined(current_isolate()).cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__GetIndex(
  object: *const Object,
  _context: *const Context,
  index: u32,
) -> *const Value {
  match unsafe { heap_value(object) } {
    Some(HeapValue::Array(state)) => state
      .elements
      .get(index as usize)
      .copied()
      .unwrap_or_else(|| v8__Undefined(current_isolate()).cast()),
    _ => v8__Undefined(current_isolate()).cast(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__Set(
  object: *const Object,
  _context: *const Context,
  key: *const Value,
  value: *const Value,
) -> MaybeBool {
  let Some(properties) = properties_mut(object) else {
    return MaybeBool::Nothing;
  };
  if let Some(property) = properties
    .iter_mut()
    .rev()
    .find(|property| same_property_key(property.key.cast(), key.cast()))
  {
    property.value = value.cast();
  } else {
    properties.push(TemplateProperty {
      key: key.cast(),
      value: value.cast(),
      attributes: 0,
    });
  }
  MaybeBool::JustTrue
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__SetIndex(
  object: *const Object,
  _context: *const Context,
  index: u32,
  value: *const Value,
) -> MaybeBool {
  let undefined = v8__Undefined(current_isolate()).cast();
  let Some(HeapValue::Array(state)) = (unsafe { heap_value_mut(object) })
  else {
    return MaybeBool::Nothing;
  };
  let index = index as usize;
  if state.elements.len() <= index {
    state.elements.resize(index + 1, undefined);
  }
  state.elements[index] = if value.is_null() { undefined } else { value };
  MaybeBool::JustTrue
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__InternalFieldCount(object: *const Object) -> int {
  match unsafe { heap_value(object) } {
    Some(HeapValue::Object(state)) => state.internal_fields.len() as int,
    _ => 0,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__GetAlignedPointerFromInternalField(
  object: *const Object,
  index: int,
  _tag: u16,
) -> *const c_void {
  match unsafe { heap_value(object) } {
    Some(HeapValue::Object(state)) if index >= 0 => state
      .internal_fields
      .get(index as usize)
      .copied()
      .unwrap_or(ptr::null_mut())
      .cast_const(),
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__SetAlignedPointerInInternalField(
  object: *const Object,
  index: int,
  value: *const c_void,
  _tag: u16,
) {
  if index < 0 {
    return;
  }
  if let Some(HeapValue::Object(state)) = unsafe { heap_value_mut(object) }
    && let Some(field) = state.internal_fields.get_mut(index as usize)
  {
    *field = value.cast_mut();
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Function__New(
  _context: *const Context,
  callback: crate::FunctionCallback,
  data: *const Value,
  _length: i32,
  _constructor_behavior: crate::ConstructorBehavior,
  _side_effect_type: crate::SideEffectType,
) -> *const crate::Function {
  allocate(
    current_isolate(),
    HeapValue::Function(FunctionState {
      callback,
      data,
      properties: Vec::new(),
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionTemplate__GetFunction(
  template: *const crate::FunctionTemplate,
  _context: *const Context,
) -> *const crate::Function {
  let Some(HeapValue::FunctionTemplate(state)) =
    (unsafe { heap_value(template) })
  else {
    return ptr::null();
  };
  let prototype =
    object_from_template(current_isolate(), state.prototype_template);
  let prototype_key = new_string(current_isolate(), "prototype".to_string());
  let mut function_properties = state.properties.clone();
  function_properties.push(TemplateProperty {
    key: prototype_key.cast(),
    value: prototype.cast(),
    attributes: 0,
  });
  allocate(
    current_isolate(),
    HeapValue::Function(FunctionState {
      callback: state.callback,
      data: state.data,
      properties: function_properties,
    }),
  )
}

unsafe fn callback_info<'a>(
  info: *const crate::function::FunctionCallbackInfo,
) -> Option<&'a mut CallbackInfoState> {
  unsafe { (info as *mut CallbackInfoState).as_mut() }
}

fn invoke_function(
  function: *const crate::Function,
  receiver: *const Value,
  argc: int,
  argv: *const *const Value,
  is_construct: bool,
) -> *const Value {
  let Some(HeapValue::Function(state)) = (unsafe { heap_value(function) })
  else {
    return ptr::null();
  };
  let callback = state.callback;
  let data = state.data;
  let isolate = current_isolate();
  let undefined = v8__Undefined(isolate).cast();
  let receiver = if receiver.is_null() {
    undefined
  } else {
    receiver
  };
  let mut args = Vec::with_capacity(argc.max(0) as usize);
  for index in 0..argc.max(0) as usize {
    let argument = if argv.is_null() {
      undefined
    } else {
      unsafe { *argv.add(index) }
    };
    args.push(if argument.is_null() {
      undefined
    } else {
      argument
    });
  }
  let mut info = Box::new(CallbackInfoState {
    isolate,
    this: receiver,
    data: if data.is_null() { undefined } else { data },
    new_target: if is_construct {
      function.cast()
    } else {
      undefined
    },
    is_construct,
    args,
    return_slot: Box::new(undefined),
  });
  let info_ptr = (&mut *info as *mut CallbackInfoState)
    .cast::<crate::function::FunctionCallbackInfo>();
  unsafe { callback(info_ptr) };
  *info.return_slot
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Function__Call(
  function: *const crate::Function,
  _context: *const Context,
  receiver: *const Value,
  argc: int,
  argv: *const *const Value,
) -> *const Value {
  invoke_function(function, receiver, argc, argv, false)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Function__NewInstance(
  function: *const crate::Function,
  _context: *const Context,
  argc: int,
  argv: *const *const Value,
) -> *const Object {
  let receiver = new_object(current_isolate());
  let result = invoke_function(function, receiver.cast(), argc, argv, true);
  if matches!(
    unsafe { heap_value(result) },
    Some(HeapValue::Object(_) | HeapValue::Function(_))
  ) {
    result.cast()
  } else {
    receiver
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__GetIsolate(
  info: *const crate::function::FunctionCallbackInfo,
) -> *mut RealIsolate {
  unsafe { callback_info(info) }
    .map(|info| info.isolate)
    .unwrap_or_else(current_isolate)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__GetParts(
  info: *const crate::function::FunctionCallbackInfo,
) -> RawFunctionCallbackInfoParts {
  let Some(info) = (unsafe { callback_info(info) }) else {
    return RawFunctionCallbackInfoParts {
      isolate: current_isolate(),
      return_value: 0,
      data: v8__Undefined(current_isolate()).cast(),
      length: 0,
    };
  };
  RawFunctionCallbackInfoParts {
    isolate: info.isolate,
    return_value: (&mut *info.return_slot as *mut *const Value) as usize,
    data: info.data,
    length: info.args.len() as int,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__Data(
  info: *const crate::function::FunctionCallbackInfo,
) -> *const Value {
  unsafe { callback_info(info) }
    .map(|info| info.data)
    .unwrap_or_else(|| v8__Undefined(current_isolate()).cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__This(
  info: *const crate::function::FunctionCallbackInfo,
) -> *const Object {
  unsafe { callback_info(info) }
    .map(|info| info.this.cast())
    .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__NewTarget(
  info: *const crate::function::FunctionCallbackInfo,
) -> *const Value {
  unsafe { callback_info(info) }
    .map(|info| info.new_target)
    .unwrap_or_else(|| v8__Undefined(current_isolate()).cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__IsConstructCall(
  info: *const crate::function::FunctionCallbackInfo,
) -> bool {
  unsafe { callback_info(info) }.is_some_and(|info| info.is_construct)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__Get(
  info: *const crate::function::FunctionCallbackInfo,
  index: int,
) -> *const Value {
  if index >= 0
    && let Some(info) = unsafe { callback_info(info) }
    && let Some(value) = info.args.get(index as usize)
  {
    return *value;
  }
  v8__Undefined(current_isolate()).cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__Length(
  info: *const crate::function::FunctionCallbackInfo,
) -> int {
  unsafe { callback_info(info) }
    .map(|info| info.args.len() as int)
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionCallbackInfo__GetReturnValue(
  info: *const crate::function::FunctionCallbackInfo,
) -> usize {
  unsafe { callback_info(info) }
    .map(|info| (&mut *info.return_slot as *mut *const Value) as usize)
    .unwrap_or(0)
}

// --- Opaque host pointers -------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn v8__External__New(
  isolate: *mut RealIsolate,
  value: *mut c_void,
) -> *const External {
  allocate(isolate, HeapValue::External(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__External__Value(
  external: *const External,
) -> *mut c_void {
  match unsafe { heap_value(external) } {
    Some(HeapValue::External(value)) => *value,
    _ => ptr::null_mut(),
  }
}

// --- Native callback return slots ----------------------------------------

unsafe fn return_value_slot(value: *const RawReturnValue) -> *mut *const Value {
  if value.is_null() {
    return ptr::null_mut();
  }
  unsafe { (*value).0 as *mut *const Value }
}

fn set_return_value(value: *mut RawReturnValue, result: *const Value) {
  let slot = unsafe { return_value_slot(value) };
  if !slot.is_null() {
    unsafe { *slot = result };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__Set(
  value: *mut RawReturnValue,
  result: *const Value,
) {
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__Set__Bool(
  value: *mut RawReturnValue,
  result: bool,
) {
  let result = allocate(current_isolate(), HeapValue::Boolean(result));
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__Set__Int32(
  value: *mut RawReturnValue,
  result: i32,
) {
  let result =
    allocate(current_isolate(), HeapValue::Number(f64::from(result)));
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__Set__Uint32(
  value: *mut RawReturnValue,
  result: u32,
) {
  let result =
    allocate(current_isolate(), HeapValue::Number(f64::from(result)));
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__Set__Double(
  value: *mut RawReturnValue,
  result: f64,
) {
  let result = allocate(current_isolate(), HeapValue::Number(result));
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__SetNull(value: *mut RawReturnValue) {
  let result = allocate(current_isolate(), HeapValue::Null);
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__SetUndefined(
  value: *mut RawReturnValue,
) {
  let result = allocate(current_isolate(), HeapValue::Undefined);
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__SetEmptyString(
  value: *mut RawReturnValue,
) {
  let result = allocate(current_isolate(), HeapValue::String(String::new()));
  set_return_value(value, result);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ReturnValue__Value__Get(
  value: *const RawReturnValue,
) -> *const Value {
  let slot = unsafe { return_value_slot(value) };
  if !slot.is_null() {
    let result = unsafe { *slot };
    if !result.is_null() {
      return result;
    }
  }
  allocate(current_isolate(), HeapValue::Undefined)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__Enter(context: *const Context) {
  let isolate = current_isolate();
  if !isolate.is_null() {
    unsafe { isolate_state(isolate).contexts.push(context) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Context__Exit(context: *const Context) {
  let isolate = current_isolate();
  if isolate.is_null() {
    return;
  }
  let contexts = unsafe { &mut isolate_state(isolate).contexts };
  if contexts.last().copied() == Some(context) {
    contexts.pop();
  }
}

fn new_string(isolate: *mut RealIsolate, value: String) -> *const V8String {
  allocate(isolate, HeapValue::String(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__Empty(
  isolate: *mut RealIsolate,
) -> *const V8String {
  new_string(isolate, String::new())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__NewFromUtf8(
  isolate: *mut RealIsolate,
  data: *const c_char,
  _new_type: NewStringType,
  length: int,
) -> *const V8String {
  if data.is_null() {
    return ptr::null();
  }
  let bytes = unsafe {
    if length < 0 {
      CStr::from_ptr(data).to_bytes()
    } else {
      std::slice::from_raw_parts(data.cast::<u8>(), length as usize)
    }
  };
  new_string(isolate, String::from_utf8_lossy(bytes).into_owned())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__NewFromOneByte(
  isolate: *mut RealIsolate,
  data: *const u8,
  _new_type: NewStringType,
  length: int,
) -> *const V8String {
  if data.is_null() || length < 0 {
    return ptr::null();
  }
  let bytes = unsafe { std::slice::from_raw_parts(data, length as usize) };
  let value: String = bytes.iter().map(|byte| char::from(*byte)).collect();
  new_string(isolate, value)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__NewFromTwoByte(
  isolate: *mut RealIsolate,
  data: *const u16,
  _new_type: NewStringType,
  length: int,
) -> *const V8String {
  if data.is_null() || length < 0 {
    return ptr::null();
  }
  let units = unsafe { std::slice::from_raw_parts(data, length as usize) };
  new_string(isolate, String::from_utf16_lossy(units))
}

#[repr(C)]
struct OneByteConstRepr {
  _vtable: *const c_void,
  data: *const c_char,
  length: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__NewExternalOneByteConst(
  isolate: *mut RealIsolate,
  one_byte: *const crate::OneByteConst,
) -> *const V8String {
  let Some(one_byte) =
    (unsafe { one_byte.cast::<OneByteConstRepr>().as_ref() })
  else {
    return ptr::null();
  };
  v8__String__NewExternalOneByteStatic(
    isolate,
    one_byte.data,
    one_byte.length.try_into().unwrap_or(int::MAX),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__NewExternalOneByteStatic(
  isolate: *mut RealIsolate,
  data: *const c_char,
  length: int,
) -> *const V8String {
  v8__String__NewFromOneByte(
    isolate,
    data.cast(),
    NewStringType::Normal,
    length,
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__Length(value: *const V8String) -> int {
  unsafe {
    string_value(value)
      .map(|value| value.encode_utf16().count() as int)
      .unwrap_or(0)
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__ContainsOnlyOneByte(
  value: *const V8String,
) -> bool {
  unsafe { string_value(value).is_some_and(|value| value.is_ascii()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__IsOneByte(value: *const V8String) -> bool {
  v8__String__ContainsOnlyOneByte(value)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ValueViewState {
  data: *mut c_void,
  length: usize,
  one_byte: usize,
  reserved: usize,
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__ValueView__CONSTRUCT(
  view: *mut ValueView,
  _isolate: *mut RealIsolate,
  value: *const V8String,
) {
  let units: Vec<u16> = unsafe { string_value(value) }
    .map(|value| value.encode_utf16().collect())
    .unwrap_or_default();
  let one_byte = units.iter().all(|unit| *unit <= u8::MAX as u16);
  let length = units.len();
  let data = if one_byte {
    let bytes: Box<[u8]> = units.iter().map(|unit| *unit as u8).collect();
    Box::into_raw(bytes).cast::<u8>().cast()
  } else {
    Box::into_raw(units.into_boxed_slice()).cast::<u16>().cast()
  };
  let state = ValueViewState {
    data,
    length,
    one_byte: usize::from(one_byte),
    reserved: 0,
  };
  unsafe { ptr::write_unaligned(view.cast(), state) };
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__ValueView__DESTRUCT(view: *mut ValueView) {
  if view.is_null() {
    return;
  }
  let state = unsafe { value_view(view) };
  if state.data.is_null() {
    return;
  }
  if state.one_byte != 0 {
    let slice =
      ptr::slice_from_raw_parts_mut(state.data.cast::<u8>(), state.length);
    unsafe { drop(Box::from_raw(slice)) };
  } else {
    let slice =
      ptr::slice_from_raw_parts_mut(state.data.cast::<u16>(), state.length);
    unsafe { drop(Box::from_raw(slice)) };
  }
  unsafe { ptr::write_bytes(view, 0, 1) };
}

unsafe fn value_view(view: *const ValueView) -> ValueViewState {
  unsafe { ptr::read_unaligned(view.cast()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__ValueView__is_one_byte(
  view: *const ValueView,
) -> bool {
  unsafe { value_view(view).one_byte != 0 }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__ValueView__data(
  view: *const ValueView,
) -> *const c_void {
  unsafe { value_view(view).data.cast_const() }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__ValueView__length(view: *const ValueView) -> int {
  unsafe { value_view(view).length as int }
}

// --- JSON -----------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn v8__JSON__Parse(
  context: *const Context,
  json_string: *const V8String,
) -> *const Value {
  if context.is_null() || json_string.is_null() {
    return ptr::null();
  }
  let Some(json_string) = (unsafe { string_value(json_string) }) else {
    return ptr::null();
  };
  let Ok(value) = serde_json::from_str(json_string) else {
    return ptr::null();
  };
  allocate_json_value(current_isolate(), value)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__JSON__Stringify(
  context: *const Context,
  json_object: *const Value,
) -> *const V8String {
  if context.is_null() || json_object.is_null() {
    return ptr::null();
  }
  let Some(value) = heap_to_json_value(json_object, &mut HashSet::new()) else {
    return ptr::null();
  };
  let Ok(value) = serde_json::to_string(&value) else {
    return ptr::null();
  };
  new_string(current_isolate(), value)
}

// --- Error values ---------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn v8__Undefined(isolate: *mut RealIsolate) -> *const Primitive {
  allocate(isolate, HeapValue::Undefined)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Null(isolate: *mut RealIsolate) -> *const Primitive {
  allocate(isolate, HeapValue::Null)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Boolean__New(
  isolate: *mut RealIsolate,
  value: bool,
) -> *const Boolean {
  allocate(isolate, HeapValue::Boolean(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Boolean__Value(value: *const Boolean) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Boolean(true)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Number__New(
  isolate: *mut RealIsolate,
  value: f64,
) -> *const Number {
  allocate(isolate, HeapValue::Number(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Number__Value(value: *const Number) -> f64 {
  match unsafe { heap_value(value) } {
    Some(HeapValue::Number(value)) => *value,
    _ => f64::NAN,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Integer__New(
  isolate: *mut RealIsolate,
  value: i32,
) -> *const Integer {
  allocate(isolate, HeapValue::Number(f64::from(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Integer__NewFromUnsigned(
  isolate: *mut RealIsolate,
  value: u32,
) -> *const Integer {
  allocate(isolate, HeapValue::Number(f64::from(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Integer__Value(value: *const Integer) -> i64 {
  v8__Number__Value(value.cast()) as i64
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Int32__Value(value: *const Int32) -> i32 {
  v8__Number__Value(value.cast()) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Uint32__Value(value: *const Uint32) -> u32 {
  v8__Number__Value(value.cast()) as u32
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsUndefined(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Undefined))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsNull(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Null))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsNullOrUndefined(value: *const Value) -> bool {
  matches!(
    unsafe { heap_value(value) },
    Some(HeapValue::Null | HeapValue::Undefined)
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsBoolean(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Boolean(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsNumber(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Number(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsInt32(value: *const Value) -> bool {
  match unsafe { heap_value(value) } {
    Some(HeapValue::Number(value)) => {
      value.is_finite()
        && value.fract() == 0.0
        && *value >= f64::from(i32::MIN)
        && *value <= f64::from(i32::MAX)
    }
    _ => false,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsUint32(value: *const Value) -> bool {
  match unsafe { heap_value(value) } {
    Some(HeapValue::Number(value)) => {
      value.is_finite()
        && value.fract() == 0.0
        && *value >= 0.0
        && *value <= f64::from(u32::MAX)
    }
    _ => false,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsString(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::String(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsStringObject(_value: *const Value) -> bool {
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsObject(value: *const Value) -> bool {
  matches!(
    unsafe { heap_value(value) },
    Some(
      HeapValue::Object(_)
        | HeapValue::Array(_)
        | HeapValue::Function(_)
        | HeapValue::Error { .. }
    )
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsArray(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Array(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsFunction(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Function(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Exception__Error(
  message: *const V8String,
) -> *const Value {
  allocate_error(message, "Error")
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Exception__RangeError(
  message: *const V8String,
) -> *const Value {
  allocate_error(message, "RangeError")
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Exception__ReferenceError(
  message: *const V8String,
) -> *const Value {
  allocate_error(message, "ReferenceError")
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Exception__SyntaxError(
  message: *const V8String,
) -> *const Value {
  allocate_error(message, "SyntaxError")
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Exception__TypeError(
  message: *const V8String,
) -> *const Value {
  allocate_error(message, "TypeError")
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsNativeError(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Error { .. }))
}

// --- Classic scripts ------------------------------------------------------

const DENO_CORE_PRELINKED_SCRIPTS: [(&str, u64); 3] = [
  ("ext:core/00_primordials.js", 0x49d0_171d_7d2c_3f4d),
  ("ext:core/00_infra.js", 0xe1a2_6738_75ca_364c),
  ("ext:core/01_core.js", 0xd2f9_d9c6_2c03_7a70),
];

fn fnv1a64(bytes: &[u8]) -> u64 {
  let mut hash = 0xcbf2_9ce4_8422_2325_u64;
  for byte in bytes {
    hash ^= u64::from(*byte);
    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
  }
  hash
}

fn named_property<T>(object: *const T, name: &str) -> Option<*const Value> {
  properties(object).and_then(|properties| {
    properties.iter().rev().find_map(|property| {
      (unsafe { string_value(property.key) } == Some(name))
        .then_some(property.value.cast())
    })
  })
}

fn set_named_property<T>(
  object: *const T,
  name: &str,
  value: *const Value,
) -> Result<(), String> {
  let key = new_string(current_isolate(), name.to_string());
  let Some(properties) = properties_mut(object) else {
    return Err(format!("{name} receiver is not a Rust-owned v8x object"));
  };
  if let Some(property) = properties
    .iter_mut()
    .rev()
    .find(|property| same_property_key(property.key.cast(), key.cast()))
  {
    property.value = value.cast();
  } else {
    properties.push(TemplateProperty {
      key: key.cast(),
      value: value.cast(),
      attributes: 0,
    });
  }
  Ok(())
}

unsafe extern "C" fn prelinked_set_up_async_stub(
  info: *const crate::function::FunctionCallbackInfo,
) {
  let Some(info) = (unsafe { callback_info(info) }) else {
    return;
  };
  let undefined = v8__Undefined(info.isolate).cast();
  let op = info.args.get(1).copied().unwrap_or(undefined);

  // The three-argument form installs an async method on a class prototype.
  // Mirror that Rust-visible side effect; callers intentionally ignore its
  // return value. The two-argument form returns the wrapped top-level op.
  if let (Some(name), Some(constructor)) = (
    info
      .args
      .first()
      .and_then(|value| unsafe { string_value(*value) }),
    info.args.get(2).copied(),
  ) && let Some(prototype) = named_property(constructor, "prototype")
  {
    let _ = set_named_property(prototype, name, op);
    *info.return_slot = undefined;
  } else {
    *info.return_slot = op;
  }
}

fn install_prelinked_deno_core_bridge(
  context: *const Context,
) -> Result<(), String> {
  let global = match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state.global,
    _ => return Err("classic script has no live v8x context".to_string()),
  };
  let deno = named_property(global, "Deno")
    .ok_or_else(|| "Rust-owned global has no Deno object".to_string())?;
  let core = named_property(deno, "core")
    .ok_or_else(|| "Rust-owned Deno object has no core object".to_string())?;
  let stub = allocate(
    current_isolate(),
    HeapValue::Function(FunctionState {
      callback: prelinked_set_up_async_stub,
      data: ptr::null(),
      properties: Vec::new(),
    }),
  );
  set_named_property(core, "setUpAsyncStub", stub)
}

fn run_prelinked_deno_core_script(
  context: *const Context,
  state: &ScriptState,
) -> Result<bool, String> {
  let Some((phase, (_, expected_hash))) = DENO_CORE_PRELINKED_SCRIPTS
    .iter()
    .enumerate()
    .find(|(_, (specifier, _))| *specifier == state.specifier)
  else {
    return Ok(false);
  };
  let actual_hash = fnv1a64(state.source.as_bytes());
  if actual_hash != *expected_hash {
    return Err(format!(
      "prelinked script {:?} has FNV-1a hash {actual_hash:#018x}, expected {expected_hash:#018x}",
      state.specifier,
    ));
  }
  let current_phase = match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state.deno_core_bootstrap_phase,
    _ => return Err("classic script has no live v8x context".to_string()),
  };
  if phase != current_phase {
    return Err(format!(
      "prelinked Deno core script order mismatch: received {:?} at phase {current_phase}, expected {:?}",
      state.specifier,
      DENO_CORE_PRELINKED_SCRIPTS
        .get(current_phase)
        .map(|(specifier, _)| *specifier)
        .unwrap_or("<complete>"),
    ));
  }

  let runtime = if phase == 0 {
    Some(crate::js2wasm_spike::deno_core_bootstrap_runtime_from_env()?)
  } else {
    None
  };
  if phase == 0 {
    install_prelinked_deno_core_bridge(context)?;
  }
  let Some(HeapValue::Context(context_state)) =
    (unsafe { heap_value_mut(context) })
  else {
    return Err("classic script context disappeared".to_string());
  };
  if let Some(runtime) = runtime {
    context_state.deno_core_bootstrap = Some(runtime);
  }
  context_state.deno_core_bootstrap_phase += 1;
  Ok(true)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Script__Compile(
  _context: *const Context,
  source: *const V8String,
  origin: *const ScriptOrigin,
) -> *const Script {
  let Some(source) = (unsafe { string_value(source) }) else {
    return ptr::null();
  };
  let resource_name = if origin.is_null() {
    "<anonymous>".to_string()
  } else {
    let resource = unsafe { origin.cast::<usize>().read() as *const Value };
    unsafe { string_value(resource) }
      .unwrap_or("<anonymous>")
      .to_string()
  };
  allocate(
    current_isolate(),
    HeapValue::Script(ScriptState {
      source: source.to_string(),
      specifier: resource_name,
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Script__Run(
  script: *const Script,
  context: *const Context,
) -> *const Value {
  let Some(HeapValue::Script(state)) = (unsafe { heap_value(script) }) else {
    return ptr::null();
  };
  match run_prelinked_deno_core_script(context, state) {
    Ok(true) => return v8__Undefined(current_isolate()).cast(),
    Ok(false) => {}
    Err(error) => {
      eprintln!("v8x/js2wasm: {error}");
      return ptr::null();
    }
  }
  eprintln!(
    "v8x/js2wasm: cannot run classic script {:?} ({} bytes): it is not part of the audited prelinked bootstrap manifest",
    state.specifier,
    state.source.len(),
  );
  ptr::null()
}

// --- Source-text modules --------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn v8__ScriptOrigin__CONSTRUCT(
  origin: *mut MaybeUninit<ScriptOrigin>,
  resource_name: *const Value,
  _line_offset: i32,
  _column_offset: i32,
  _shared_cross_origin: bool,
  _script_id: i32,
  _source_map_url: *const Value,
  _opaque: bool,
  _wasm: bool,
  _module: bool,
  _host_options: *const Data,
) {
  if origin.is_null() {
    return;
  }
  unsafe {
    ptr::write_bytes(origin.cast::<u8>(), 0, size_of_val(&*origin));
    origin.cast::<usize>().write(resource_name as usize);
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ScriptCompiler__Source__CONSTRUCT(
  source: *mut MaybeUninit<Source>,
  source_string: *const V8String,
  origin: *const ScriptOrigin,
  _cached_data: *mut CachedData,
) {
  if source.is_null() {
    return;
  }
  unsafe {
    ptr::write_bytes(source.cast::<u8>(), 0, size_of_val(&*source));
    let words = source.cast::<usize>();
    words.write(source_string as usize);
    if !origin.is_null() {
      words.add(1).write((origin.cast::<usize>()).read());
    }
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ScriptCompiler__Source__DESTRUCT(_source: *mut Source) {}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ScriptCompiler__Source__GetCachedData<'a>(
  _source: *const Source,
) -> *const CachedData<'a> {
  ptr::null()
}

fn quoted_specifier(text: &str) -> Option<String> {
  let start = text.find(['\'', '"'])?;
  let quote = text.as_bytes()[start];
  let rest = &text[start + 1..];
  let end = rest.as_bytes().iter().position(|byte| *byte == quote)?;
  Some(rest[..end].to_string())
}

fn parse_static_imports(source: &str) -> Vec<String> {
  let mut imports = Vec::new();
  for statement in source.split(';') {
    let statement = statement.trim();
    let candidate = if let Some(rest) = statement.strip_prefix("import ") {
      if rest.starts_with(['\'', '"']) {
        Some(rest)
      } else {
        rest.rsplit_once(" from ").map(|(_, value)| value)
      }
    } else if statement.starts_with("export ") {
      statement.rsplit_once(" from ").map(|(_, value)| value)
    } else {
      None
    };
    if let Some(specifier) = candidate.and_then(quoted_specifier) {
      imports.push(specifier);
    }
  }
  imports
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ScriptCompiler__CompileModule(
  isolate: *mut RealIsolate,
  source: *mut Source,
  _options: CompileOptions,
  _no_cache_reason: NoCacheReason,
) -> *const Module {
  if source.is_null() {
    return ptr::null();
  }
  let words = source.cast::<usize>();
  let source_string = unsafe { words.read() as *const V8String };
  let resource_name = unsafe { words.add(1).read() as *const V8String };
  let Some(source) = (unsafe { string_value(source_string) }) else {
    return ptr::null();
  };
  let specifier = unsafe { string_value(resource_name) }
    .unwrap_or_default()
    .to_string();
  allocate(
    isolate,
    HeapValue::Module(ModuleState {
      status: STATUS_UNINSTANTIATED,
      source: source.to_string(),
      specifier,
      imports: parse_static_imports(source),
      dependencies: Vec::new(),
      runtime: None,
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__GetStatus(module: *const Module) -> ModuleStatus {
  match unsafe { module_state(module) }.map(|state| state.status) {
    Some(STATUS_UNINSTANTIATED) => ModuleStatus::Uninstantiated,
    Some(STATUS_INSTANTIATING) => ModuleStatus::Instantiating,
    Some(STATUS_INSTANTIATED) => ModuleStatus::Instantiated,
    Some(STATUS_EVALUATING) => ModuleStatus::Evaluating,
    Some(STATUS_EVALUATED) => ModuleStatus::Evaluated,
    _ => ModuleStatus::Errored,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__InstantiateModule(
  module: *const Module,
  context: *const Context,
  callback: ResolveModuleCallback,
  source_callback: Option<ResolveSourceCallback>,
) -> MaybeBool {
  let Some(state) = (unsafe { module_state(module) }) else {
    return MaybeBool::Nothing;
  };
  if state.status != STATUS_UNINSTANTIATED {
    return MaybeBool::JustTrue;
  }
  state.status = STATUS_INSTANTIATING;
  let imports = state.imports.clone();
  let isolate = current_isolate();
  let mut dependencies = Vec::with_capacity(imports.len());

  for specifier in imports {
    let specifier = new_string(isolate, specifier);
    let attributes = allocate::<FixedArray>(isolate, HeapValue::FixedArray);
    let Some(context_local) = (unsafe { crate::Local::from_raw(context) })
    else {
      return MaybeBool::Nothing;
    };
    let specifier_local = unsafe { crate::Local::from_raw(specifier) }.unwrap();
    let attributes_local =
      unsafe { crate::Local::from_raw(attributes) }.unwrap();
    let module_local = unsafe { crate::Local::from_raw(module) }.unwrap();
    let returned = unsafe {
      callback(
        context_local,
        specifier_local,
        attributes_local,
        module_local,
      )
    };
    let dependency = unsafe {
      *(&returned as *const ResolveModuleCallbackRet as *const *const Module)
    };
    if dependency.is_null() {
      if let Some(state) = unsafe { module_state(module) } {
        state.status = STATUS_ERRORED;
      }
      return MaybeBool::Nothing;
    }
    if unsafe { module_state(dependency) }
      .is_some_and(|state| state.status == STATUS_UNINSTANTIATED)
      && v8__Module__InstantiateModule(
        dependency,
        context,
        callback,
        source_callback,
      ) != MaybeBool::JustTrue
    {
      if let Some(state) = unsafe { module_state(module) } {
        state.status = STATUS_ERRORED;
      }
      return MaybeBool::Nothing;
    }
    dependencies.push(dependency);
  }

  if let Some(state) = unsafe { module_state(module) } {
    state.dependencies = dependencies;
    state.status = STATUS_INSTANTIATED;
  }
  MaybeBool::JustTrue
}

fn collect_graph(
  module: *const Module,
  seen: &mut HashSet<usize>,
  sources: &mut Vec<crate::js2wasm_spike::SourceModule>,
) -> Result<(), String> {
  if !seen.insert(module as usize) {
    return Ok(());
  }
  let (specifier, source, dependencies) = unsafe { module_state(module) }
    .map(|state| {
      (
        state.specifier.clone(),
        state.source.clone(),
        state.dependencies.clone(),
      )
    })
    .ok_or_else(|| "v8x/js2wasm: invalid module handle".to_string())?;
  sources.push(crate::js2wasm_spike::SourceModule { specifier, source });
  for dependency in dependencies {
    collect_graph(dependency, seen, sources)?;
  }
  Ok(())
}

fn mark_evaluated(module: *const Module, seen: &mut HashSet<usize>) {
  if !seen.insert(module as usize) {
    return;
  }
  let dependencies = match unsafe { module_state(module) } {
    Some(state) => {
      state.status = STATUS_EVALUATED;
      state.dependencies.clone()
    }
    None => return,
  };
  for dependency in dependencies {
    mark_evaluated(dependency, seen);
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__Evaluate(
  module: *const Module,
  _context: *const Context,
) -> *const Value {
  let Some(state) = (unsafe { module_state(module) }) else {
    return ptr::null();
  };
  if state.status == STATUS_EVALUATED {
    return allocate(current_isolate(), HeapValue::Promise);
  }
  if state.status != STATUS_INSTANTIATED {
    state.status = STATUS_ERRORED;
    return ptr::null();
  }
  state.status = STATUS_EVALUATING;
  let entry = state.specifier.clone();
  let mut seen = HashSet::new();
  let mut sources = Vec::new();
  let runtime =
    match collect_graph(module, &mut seen, &mut sources).and_then(|()| {
      crate::js2wasm_spike::compile_and_instantiate(&entry, &sources)
    }) {
      Ok(runtime) => runtime,
      Err(error) => {
        eprintln!("{error}");
        if let Some(state) = unsafe { module_state(module) } {
          state.status = STATUS_ERRORED;
        }
        return ptr::null();
      }
    };
  if let Some(state) = unsafe { module_state(module) } {
    state.runtime = Some(runtime);
  }
  mark_evaluated(module, &mut HashSet::new());
  allocate(current_isolate(), HeapValue::Promise)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__IsSourceTextModule(
  _module: *const Module,
) -> bool {
  true
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__IsSyntheticModule(
  _module: *const Module,
) -> bool {
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__IsGraphAsync(_module: *const Module) -> bool {
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__ScriptId(_module: *const Module) -> int {
  1
}
