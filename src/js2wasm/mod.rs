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
  ModuleStatus, ResolveModuleCallback, ResolveSourceCallback,
  SyntheticModuleEvaluationSteps,
};
#[cfg(not(target_os = "windows"))]
use crate::module::{
  ResolveModuleCallbackRet, SyntheticModuleEvaluationStepsRet,
};
use crate::script::ScriptOrigin;
use crate::script_compiler::{
  CachedData, CompileOptions, NoCacheReason, Source,
};
use crate::string::{NewStringType, ValueView};
use crate::support::{Maybe, MaybeBool, SharedPtrBase, UniquePtr, int, long};
use crate::{
  Allocator, Array, ArrayBuffer, ArrayBufferView, BackingStore,
  BackingStoreDeleterCallback, BigInt, BigInt64Array, BigUint64Array, Boolean,
  Context, Data, External, FixedArray, Int32, Int32Array, Integer,
  KeyConversionMode, Message, Module, Number, Object, Platform, Primitive,
  Promise, PromiseResolver, PromiseState, PropertyFilter, RealIsolate, Script,
  StackTrace, String as V8String, TypedArray, Uint8Array, Uint16Array, Uint32,
  Uint32Array, UnboundModuleScript, Value,
};
#[cfg(feature = "js2wasm_deno_poc_replay")]
use sha2::{Digest, Sha256};
use std::cell::RefCell;
use std::collections::HashSet;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::mem::{MaybeUninit, size_of};
use std::ptr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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
  synthetic: Option<SyntheticModuleState>,
  evaluation_result: *const Value,
  exception: *const Value,
  unbound_script: *const UnboundModuleScript,
  module_requests: *const FixedArray,
  namespace: *const Object,
  prelinked_deno_module: bool,
}

struct SyntheticModuleState {
  export_names: Vec<String>,
  evaluation_steps: SyntheticModuleEvaluationSteps<'static>,
  namespace: *const Object,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromiseSettlement {
  Pending,
  Fulfilled,
  Rejected,
}

struct PromiseStateData {
  settlement: PromiseSettlement,
  result: *const Value,
  handled: bool,
}

struct PromiseResolverState {
  promise: *const Promise,
}

struct UnboundModuleScriptState {
  source_mapping_url: *const Value,
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
  class_name: *const V8String,
  properties: Vec<TemplateProperty>,
  prototype_template: *const crate::ObjectTemplate,
  instance_template: *const crate::ObjectTemplate,
}

struct ObjectState {
  // None means the ordinary realm prototype is not materialized yet. Some
  // embedding APIs (notably Deno's import-meta root) provide an explicit
  // prototype, including an exact V8 null handle, which must be retained.
  prototype: Option<*const Value>,
  properties: Vec<TemplateProperty>,
  internal_fields: Vec<*mut c_void>,
}

struct ArrayState {
  elements: Vec<*const Value>,
  properties: Vec<TemplateProperty>,
}

struct BackingStoreState {
  data: *mut c_void,
  byte_length: usize,
  deleter: BackingStoreDeleterCallback,
  deleter_data: *mut c_void,
}

impl Drop for BackingStoreState {
  fn drop(&mut self) {
    if !self.data.is_null() {
      unsafe { (self.deleter)(self.data, self.byte_length, self.deleter_data) };
    }
  }
}

struct ArrayBufferState {
  backing_store: SharedRepr,
}

impl Drop for ArrayBufferState {
  fn drop(&mut self) {
    release_backing_store(self.backing_store);
  }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TypedArrayKind {
  Uint8,
  Uint16,
  Uint32,
  Int32,
  BigUint64,
  BigInt64,
}

impl TypedArrayKind {
  fn element_size(self) -> usize {
    match self {
      Self::Uint8 => size_of::<u8>(),
      Self::Uint16 => size_of::<u16>(),
      Self::Uint32 => size_of::<u32>(),
      Self::Int32 => size_of::<i32>(),
      Self::BigUint64 => size_of::<u64>(),
      Self::BigInt64 => size_of::<i64>(),
    }
  }
}

struct TypedArrayState {
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
  kind: TypedArrayKind,
  properties: Vec<TemplateProperty>,
}

struct FunctionState {
  callback: crate::FunctionCallback,
  data: *const Value,
  name: *const V8String,
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
  deno_core_bootstrap: Option<Rc<RefCell<crate::js2wasm_spike::DenoRuntime>>>,
  deno_core_bootstrap_phase: usize,
}

enum HeapValue {
  String(String),
  Context(ContextState),
  Object(ObjectState),
  Array(ArrayState),
  ArrayBuffer(ArrayBufferState),
  TypedArray(TypedArrayState),
  Function(FunctionState),
  Module(ModuleState),
  Script(ScriptState),
  ObjectTemplate(ObjectTemplateState),
  FunctionTemplate(FunctionTemplateState),
  FixedArray(Vec<*const Data>),
  UnboundModuleScript(UnboundModuleScriptState),
  Promise(PromiseStateData),
  PromiseResolver(PromiseResolverState),
  Error { name: &'static str, message: String },
  External(*mut c_void),
  Boolean(bool),
  Number(f64),
  BigInt(i128),
  Null,
  Undefined,
}

#[repr(C)]
pub(crate) struct RawReturnValue(usize);

#[repr(C)]
struct MaybeMirror<T> {
  has_value: bool,
  value: T,
}

unsafe fn write_maybe<T: Copy + Default>(out: *mut Maybe<T>, value: Option<T>) {
  if out.is_null() {
    return;
  }
  let (has_value, value) = value
    .map(|value| (true, value))
    .unwrap_or_else(|| (false, T::default()));
  unsafe {
    out
      .cast::<MaybeMirror<T>>()
      .write(MaybeMirror { has_value, value });
  }
}

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
  microtasks: Vec<*const crate::Function>,
  running_microtasks: bool,
  terminating: AtomicBool,
  active_try_catch: *mut TryCatchAbiState,
  pending_exception: *const Value,
}

// rusty_v8's raw TryCatch is an inline `[MaybeUninit<usize>; 6]`. Keep all
// lifecycle state in those exact six words so the public Rust API retains its
// stack discipline without allocating an engine-side shadow object.
#[repr(C)]
#[derive(Clone, Copy)]
struct TryCatchAbiState {
  isolate: *mut RealIsolate,
  previous: *mut TryCatchAbiState,
  exception: *const Value,
  flags: usize,
  reserved: [usize; 2],
}

const _: [(); 6 * size_of::<usize>()] = [(); size_of::<TryCatchAbiState>()];
const _: [(); std::mem::align_of::<usize>()] =
  [(); std::mem::align_of::<TryCatchAbiState>()];

const TRY_CATCH_RETHROWN: usize = 1 << 0;
const TRY_CATCH_VERBOSE: usize = 1 << 1;
const TRY_CATCH_CAPTURE_MESSAGE: usize = 1 << 2;

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

unsafe fn promise_state<'a>(
  promise: *const Promise,
) -> Option<&'a mut PromiseStateData> {
  match unsafe { (promise as *mut HeapValue).as_mut() } {
    Some(HeapValue::Promise(state)) => Some(state),
    _ => None,
  }
}

unsafe fn promise_resolver_state<'a>(
  resolver: *const PromiseResolver,
) -> Option<&'a PromiseResolverState> {
  match unsafe { (resolver as *const HeapValue).as_ref() } {
    Some(HeapValue::PromiseResolver(state)) => Some(state),
    _ => None,
  }
}

fn allocate_promise(
  isolate: *mut RealIsolate,
  settlement: PromiseSettlement,
  result: *const Value,
) -> *const Promise {
  allocate(
    isolate,
    HeapValue::Promise(PromiseStateData {
      settlement,
      result,
      handled: false,
    }),
  )
}

fn allocate_fulfilled_promise(
  isolate: *mut RealIsolate,
  result: *const Value,
) -> *const Promise {
  allocate_promise(isolate, PromiseSettlement::Fulfilled, result)
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

fn deno_core_runtime(
  context: *const Context,
) -> Result<Rc<RefCell<crate::js2wasm_spike::DenoRuntime>>, String> {
  match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state
      .deno_core_bootstrap
      .clone()
      .ok_or_else(|| "prelinked Deno core runtime disappeared".to_string()),
    _ => Err("prelinked Deno operation has no live context".to_string()),
  }
}

fn with_deno_core_runtime<T>(
  context: *const Context,
  operation: &str,
  callback: impl FnOnce(&mut crate::js2wasm_spike::DenoRuntime) -> Result<T, String>,
) -> Result<T, String> {
  // The runtime lives in its own allocation. Clone the slot while borrowing
  // ContextState, then end that borrow before entering Wasmtime: host imports
  // may synchronously invoke ordinary rusty_v8 APIs that read the context.
  let runtime = deno_core_runtime(context)?;
  let mut runtime = runtime.try_borrow_mut().map_err(|_| {
    format!("prelinked Deno core runtime re-entered during {operation}")
  })?;
  callback(&mut runtime)
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
      prototype: None,
      properties,
      internal_fields: vec![ptr::null_mut(); internal_field_count],
    }),
  )
}

fn new_object(isolate: *mut RealIsolate) -> *const Object {
  allocate(
    isolate,
    HeapValue::Object(ObjectState {
      prototype: None,
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
    Some(HeapValue::TypedArray(state)) => Some(&state.properties),
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
    Some(HeapValue::TypedArray(state)) => Some(&mut state.properties),
    Some(HeapValue::Function(state)) => Some(&mut state.properties),
    _ => None,
  }
}

fn own_property<T>(
  value: *const T,
  key: *const Data,
) -> Option<TemplateProperty> {
  properties(value).and_then(|properties| {
    properties
      .iter()
      .rev()
      .find(|property| same_property_key(property.key.cast(), key))
      .copied()
  })
}

fn property_on_chain<T>(
  value: *const T,
  key: *const Data,
) -> Option<TemplateProperty> {
  let mut current = value.cast::<Value>();
  let mut seen = HashSet::new();
  while !current.is_null() && seen.insert(current.addr()) {
    if let Some(property) = own_property(current, key) {
      return Some(property);
    }
    current = match unsafe { heap_value(current) } {
      Some(HeapValue::Object(state)) => match state.prototype {
        Some(prototype)
          if !matches!(
            unsafe { heap_value(prototype) },
            Some(HeapValue::Null)
          ) =>
        {
          prototype
        }
        _ => ptr::null(),
      },
      _ => ptr::null(),
    };
  }
  None
}

fn is_valid_prototype(value: *const Value) -> bool {
  matches!(
    unsafe { heap_value(value) },
    Some(
      HeapValue::Null
        | HeapValue::Object(_)
        | HeapValue::Array(_)
        | HeapValue::Function(_)
    )
  )
}

fn prototype_would_cycle(
  object: *const Object,
  prototype: *const Value,
) -> bool {
  let mut current = prototype;
  let mut seen = HashSet::new();
  while !current.is_null() && seen.insert(current.addr()) {
    if std::ptr::addr_eq(current, object.cast::<Value>()) {
      return true;
    }
    current = match unsafe { heap_value(current) } {
      Some(HeapValue::Object(state)) => match state.prototype {
        Some(prototype)
          if !matches!(
            unsafe { heap_value(prototype) },
            Some(HeapValue::Null)
          ) =>
        {
          prototype
        }
        _ => ptr::null(),
      },
      _ => ptr::null(),
    };
  }
  false
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
    serde_json::Value::String(value) => value
      .strip_prefix("\u{0}v8x-bigint:")
      .and_then(|value| value.parse::<i128>().ok())
      .map(|value| allocate(isolate, HeapValue::BigInt(value)))
      .unwrap_or_else(|| allocate(isolate, HeapValue::String(value))),
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
      if values.len() == 2
        && let Some(serde_json::Value::String(kind)) =
          values.get("\u{0}v8x-typed-array")
        && let Some(serde_json::Value::Array(elements)) = values.get("values")
        && let Some(value) = allocate_json_typed_array(isolate, kind, elements)
      {
        return value;
      }
      if values.len() == 1
        && values.get("\u{0}v8x-undefined")
          == Some(&serde_json::Value::Bool(true))
      {
        return allocate(isolate, HeapValue::Undefined);
      }
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
          prototype: None,
          properties,
          internal_fields: Vec::new(),
        }),
      )
    }
  }
}

fn allocate_json_typed_array(
  isolate: *mut RealIsolate,
  kind: &str,
  elements: &[serde_json::Value],
) -> Option<*const Value> {
  let kind = match kind {
    "Uint8Array" => TypedArrayKind::Uint8,
    "Uint16Array" => TypedArrayKind::Uint16,
    "Uint32Array" => TypedArrayKind::Uint32,
    "Int32Array" => TypedArrayKind::Int32,
    "BigUint64Array" => TypedArrayKind::BigUint64,
    "BigInt64Array" => TypedArrayKind::BigInt64,
    _ => return None,
  };
  let byte_length = elements.len().checked_mul(kind.element_size())?;
  let mut bytes = vec![0_u8; byte_length].into_boxed_slice();
  for (index, value) in elements.iter().enumerate() {
    let offset = index * kind.element_size();
    let data = unsafe { bytes.as_mut_ptr().add(offset) };
    match kind {
      TypedArrayKind::Uint8 => unsafe {
        data.write(value.as_u64()? as u8);
      },
      TypedArrayKind::Uint16 => unsafe {
        data.cast::<u16>().write_unaligned(value.as_u64()? as u16);
        continue;
      },
      TypedArrayKind::Uint32 => unsafe {
        data.cast::<u32>().write_unaligned(value.as_u64()? as u32);
        continue;
      },
      TypedArrayKind::Int32 => unsafe {
        data.cast::<i32>().write_unaligned(value.as_i64()? as i32);
        continue;
      },
      TypedArrayKind::BigUint64 => {
        let value = value
          .as_str()?
          .strip_prefix("\u{0}v8x-bigint:")?
          .parse::<u64>()
          .ok()?;
        unsafe { data.cast::<u64>().write_unaligned(value) };
        continue;
      }
      TypedArrayKind::BigInt64 => {
        let value = value
          .as_str()?
          .strip_prefix("\u{0}v8x-bigint:")?
          .parse::<i64>()
          .ok()?;
        unsafe { data.cast::<i64>().write_unaligned(value) };
      }
    }
  }
  let data = Box::into_raw(bytes).cast::<u8>().cast::<c_void>();
  let backing_store = Box::into_raw(Box::new(BackingStoreState {
    data,
    byte_length,
    deleter: drop_json_typed_array_bytes,
    deleter_data: ptr::null_mut(),
  }));
  let buffer: *const ArrayBuffer = allocate(
    isolate,
    HeapValue::ArrayBuffer(ArrayBufferState {
      backing_store: SharedRepr {
        object: backing_store.cast(),
        references: Box::into_raw(Box::new(AtomicUsize::new(1))),
      },
    }),
  );
  Some(allocate(
    isolate,
    HeapValue::TypedArray(TypedArrayState {
      buffer,
      byte_offset: 0,
      length: elements.len(),
      kind,
      properties: Vec::new(),
    }),
  ))
}

fn allocate_script_json_value(
  isolate: *mut RealIsolate,
  source: &str,
  value: serde_json::Value,
) -> *const Value {
  // The Deno POC lowers these six constructor expressions to provider-local
  // arrays. Restore the V8 brand and backing-store layout at the host boundary;
  // scalar/equality results bypass this path. Subarray windows are applied
  // below because the provider's interpreter arrays have no subarray method.
  let kind = [
    ("new BigUint64Array(", "BigUint64Array"),
    ("new BigInt64Array(", "BigInt64Array"),
    ("new Uint16Array(", "Uint16Array"),
    ("new Uint32Array(", "Uint32Array"),
    ("new Uint8Array(", "Uint8Array"),
    ("new Int32Array(", "Int32Array"),
  ]
  .into_iter()
  .find_map(|(needle, kind)| source.contains(needle).then_some(kind));
  if let (Some(kind), serde_json::Value::Array(elements)) = (kind, &value)
    && let Some((start, end)) =
      typed_array_source_window(source, elements.len())
    && let Some(value) =
      allocate_json_typed_array(isolate, kind, &elements[start..end])
  {
    return value;
  }
  allocate_json_value(isolate, value)
}

fn typed_array_source_window(
  source: &str,
  length: usize,
) -> Option<(usize, usize)> {
  let Some(arguments) = source
    .rfind(".subarray(")
    .map(|start| &source[start + ".subarray(".len()..])
  else {
    return Some((0, length));
  };
  let arguments = arguments.split_once(')')?.0;
  let mut arguments = arguments.split(',').map(str::trim);
  let start = arguments.next()?.parse::<usize>().ok()?.min(length);
  let end = arguments
    .next()
    .and_then(|end| end.parse::<usize>().ok())
    .unwrap_or(length)
    .clamp(start, length);
  Some((start, end))
}

unsafe extern "C" fn drop_json_typed_array_bytes(
  data: *mut c_void,
  byte_length: usize,
  _deleter_data: *mut c_void,
) {
  if !data.is_null() {
    let slice = ptr::slice_from_raw_parts_mut(data.cast::<u8>(), byte_length);
    unsafe { drop(Box::from_raw(slice)) };
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
    HeapValue::BigInt(value) => Some(serde_json::Value::String(format!(
      "\u{0}v8x-bigint:{value}"
    ))),
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
    HeapValue::TypedArray(state) => {
      let result = (0..state.length)
        .map(|index| {
          if matches!(
            state.kind,
            TypedArrayKind::BigUint64 | TypedArrayKind::BigInt64
          ) {
            return typed_array_bigint(state, index)
              .map(|value| {
                serde_json::Value::String(format!("\u{0}v8x-bigint:{value}"))
              })
              .unwrap_or(serde_json::Value::Null);
          }
          typed_array_element(state, index)
            .and_then(serde_json::Number::from_f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null)
        })
        .collect();
      Some(serde_json::Value::Array(result))
    }
    HeapValue::Undefined
    | HeapValue::Function(_)
    | HeapValue::ArrayBuffer(_)
    | HeapValue::External(_)
    | HeapValue::Error { .. }
    | HeapValue::Context(_)
    | HeapValue::Module(_)
    | HeapValue::Script(_)
    | HeapValue::ObjectTemplate(_)
    | HeapValue::FunctionTemplate(_)
    | HeapValue::FixedArray(_)
    | HeapValue::UnboundModuleScript(_)
    | HeapValue::Promise(_)
    | HeapValue::PromiseResolver(_) => None,
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
#[derive(Clone, Copy)]
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

unsafe fn backing_store_state<'a>(
  backing_store: *const BackingStore,
) -> Option<&'a BackingStoreState> {
  unsafe { (backing_store as *const BackingStoreState).as_ref() }
}

fn retain_backing_store(repr: SharedRepr) -> SharedRepr {
  if !repr.references.is_null() {
    unsafe { (*repr.references).fetch_add(1, Ordering::Relaxed) };
  }
  repr
}

fn release_backing_store(repr: SharedRepr) {
  if repr.object.is_null() {
    return;
  }
  if repr.references.is_null() {
    unsafe { drop(Box::from_raw(repr.object.cast::<BackingStoreState>())) };
    return;
  }
  if unsafe { (*repr.references).fetch_sub(1, Ordering::AcqRel) } == 1 {
    unsafe {
      drop(Box::from_raw(repr.object.cast::<BackingStoreState>()));
      drop(Box::from_raw(repr.references));
    }
  }
}

unsafe fn backing_store_shared_ref(
  repr: SharedRepr,
) -> crate::support::SharedRef<BackingStore> {
  unsafe { std::mem::transmute(repr) }
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
pub extern "C" fn v8__ArrayBuffer__NewBackingStore__with_byte_length(
  _isolate: *mut RealIsolate,
  byte_length: usize,
) -> *mut BackingStore {
  let bytes = vec![0_u8; byte_length].into_boxed_slice();
  let data = Box::into_raw(bytes).cast::<u8>().cast::<c_void>();
  Box::into_raw(Box::new(BackingStoreState {
    data,
    byte_length,
    deleter: drop_json_typed_array_bytes,
    deleter_data: ptr::null_mut(),
  }))
  .cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__NewBackingStore__with_data(
  data: *mut c_void,
  byte_length: usize,
  deleter: BackingStoreDeleterCallback,
  deleter_data: *mut c_void,
) -> *mut BackingStore {
  if data.is_null() && byte_length != 0 {
    return ptr::null_mut();
  }
  Box::into_raw(Box::new(BackingStoreState {
    data,
    byte_length,
    deleter,
    deleter_data,
  }))
  .cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__New__with_byte_length(
  isolate: *mut RealIsolate,
  byte_length: usize,
) -> *const ArrayBuffer {
  if isolate.is_null() {
    return ptr::null();
  }
  let backing_store =
    v8__ArrayBuffer__NewBackingStore__with_byte_length(isolate, byte_length);
  allocate(
    isolate,
    HeapValue::ArrayBuffer(ArrayBufferState {
      backing_store: SharedRepr {
        object: backing_store.cast(),
        references: Box::into_raw(Box::new(AtomicUsize::new(1))),
      },
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BackingStore__Data(
  backing_store: *const BackingStore,
) -> *mut c_void {
  unsafe { backing_store_state(backing_store) }
    .map(|state| state.data)
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BackingStore__ByteLength(
  backing_store: *const BackingStore,
) -> usize {
  unsafe { backing_store_state(backing_store) }
    .map(|state| state.byte_length)
    .unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BackingStore__IsShared(
  _backing_store: *const BackingStore,
) -> bool {
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BackingStore__IsResizableByUserJavaScript(
  _backing_store: *const BackingStore,
) -> bool {
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BackingStore__DELETE(backing_store: *mut BackingStore) {
  if !backing_store.is_null() {
    unsafe { drop(Box::from_raw(backing_store.cast::<BackingStoreState>())) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__BackingStore__CONVERT__std__unique_ptr(
  unique: UniquePtr<BackingStore>,
) -> SharedPtrBase<BackingStore> {
  let object: *mut c_void = unique.into_raw().cast();
  let references = if object.is_null() {
    ptr::null_mut()
  } else {
    Box::into_raw(Box::new(AtomicUsize::new(1)))
  };
  unsafe { shared_from_repr(SharedRepr { object, references }) }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__BackingStore__get(
  shared: *const SharedPtrBase<BackingStore>,
) -> *mut BackingStore {
  unsafe { shared_repr(shared).object.cast() }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__BackingStore__COPY(
  shared: *const SharedPtrBase<BackingStore>,
) -> SharedPtrBase<BackingStore> {
  let repr = retain_backing_store(unsafe { shared_repr(shared) });
  unsafe { shared_from_repr(repr) }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__BackingStore__reset(
  shared: *mut SharedPtrBase<BackingStore>,
) {
  let repr = unsafe { shared_repr(shared) };
  release_backing_store(repr);
  if !shared.is_null() {
    unsafe { ptr::write_bytes(shared, 0, 1) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn std__shared_ptr__v8__BackingStore__use_count(
  shared: *const SharedPtrBase<BackingStore>,
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
    microtasks: Vec::new(),
    running_microtasks: false,
    terminating: AtomicBool::new(false),
    active_try_catch: ptr::null_mut(),
    pending_exception: ptr::null(),
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
pub extern "C" fn v8__Isolate__IsExecutionTerminating(
  isolate: *const RealIsolate,
) -> bool {
  if isolate.is_null() {
    return false;
  }
  unsafe { isolate_state(isolate.cast_mut()) }
    .terminating
    .load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__TerminateExecution(isolate: *const RealIsolate) {
  if isolate.is_null() {
    return;
  }
  unsafe { isolate_state(isolate.cast_mut()) }
    .terminating
    .store(true, Ordering::Release);
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__CancelTerminateExecution(
  isolate: *const RealIsolate,
) {
  if isolate.is_null() {
    return;
  }
  unsafe { isolate_state(isolate.cast_mut()) }
    .terminating
    .store(false, Ordering::Release);
}

fn record_exception(isolate: *mut RealIsolate, exception: *const Value) {
  if isolate.is_null() || exception.is_null() {
    return;
  }
  let isolate = unsafe { isolate_state(isolate) };
  if let Some(try_catch) = unsafe { isolate.active_try_catch.as_mut() } {
    try_catch.exception = exception;
    isolate.pending_exception = ptr::null();
  } else {
    isolate.pending_exception = exception;
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__ThrowException(
  isolate: *mut RealIsolate,
  exception: *const Value,
) -> *const Value {
  record_exception(isolate, exception);
  // V8 schedules the supplied exception but deliberately returns undefined.
  v8__Undefined(isolate).cast()
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
pub extern "C" fn v8__Isolate__EnqueueMicrotask(
  isolate: *mut RealIsolate,
  function: *const crate::Function,
) {
  if isolate.is_null()
    || !matches!(
      unsafe { heap_value(function) },
      Some(HeapValue::Function(_))
    )
  {
    return;
  }
  unsafe { isolate_state(isolate).microtasks.push(function) };
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Isolate__PerformMicrotaskCheckpoint(
  isolate: *mut RealIsolate,
) {
  if isolate.is_null() {
    return;
  }
  let state = unsafe { isolate_state(isolate) };
  if state.running_microtasks {
    return;
  }
  state.running_microtasks = true;

  loop {
    let function = {
      let state = unsafe { isolate_state(isolate) };
      if state.microtasks.is_empty() {
        state.running_microtasks = false;
        break;
      }
      state.microtasks.remove(0)
    };
    let receiver = v8__Undefined(isolate).cast();
    let _ = invoke_function(function, receiver, 0, ptr::null(), false);
    // V8 reports and clears exceptions thrown by microtasks rather than
    // leaking them into the caller of PerformMicrotaskCheckpoint.
    let state = unsafe { isolate_state(isolate) };
    state.pending_exception = ptr::null();
    if let Some(try_catch) = unsafe { state.active_try_catch.as_mut() } {
      try_catch.exception = ptr::null();
    }
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

unsafe fn try_catch_state<'a>(
  this: *const usize,
) -> Option<&'a TryCatchAbiState> {
  unsafe { this.cast::<TryCatchAbiState>().as_ref() }
}

unsafe fn try_catch_state_mut<'a>(
  this: *mut usize,
) -> Option<&'a mut TryCatchAbiState> {
  unsafe { this.cast::<TryCatchAbiState>().as_mut() }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__CONSTRUCT(
  buffer: *mut usize,
  isolate: *mut RealIsolate,
) {
  if buffer.is_null() {
    return;
  }
  let previous = if isolate.is_null() {
    ptr::null_mut()
  } else {
    unsafe { isolate_state(isolate).active_try_catch }
  };
  let try_catch = buffer.cast::<TryCatchAbiState>();
  unsafe {
    try_catch.write(TryCatchAbiState {
      isolate,
      previous,
      exception: ptr::null(),
      flags: TRY_CATCH_CAPTURE_MESSAGE,
      reserved: [0; 2],
    });
  }
  if !isolate.is_null() {
    unsafe { isolate_state(isolate).active_try_catch = try_catch };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__DESTRUCT(this: *mut usize) {
  let Some(try_catch) = (unsafe { try_catch_state(this) }).copied() else {
    return;
  };
  if try_catch.isolate.is_null() {
    return;
  }

  let isolate = unsafe { isolate_state(try_catch.isolate) };
  if isolate.active_try_catch != this.cast::<TryCatchAbiState>() {
    eprintln!("v8x/js2wasm: TryCatch scopes were destroyed out of order");
    std::process::abort();
  }
  isolate.active_try_catch = try_catch.previous;

  if try_catch.flags & TRY_CATCH_RETHROWN != 0 && !try_catch.exception.is_null()
  {
    if let Some(previous) = unsafe { try_catch.previous.as_mut() } {
      previous.exception = try_catch.exception;
    } else {
      isolate.pending_exception = try_catch.exception;
    }
  }
  unsafe { ptr::write_bytes(this, 0, 6) };
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__HasCaught(this: *const usize) -> bool {
  unsafe { try_catch_state(this) }
    .is_some_and(|try_catch| !try_catch.exception.is_null())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__CanContinue(_this: *const usize) -> bool {
  // This backend does not yet implement execution termination, so all caught
  // JavaScript exceptions remain continuable.
  true
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__HasTerminated(_this: *const usize) -> bool {
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__IsVerbose(this: *const usize) -> bool {
  unsafe { try_catch_state(this) }
    .is_some_and(|try_catch| try_catch.flags & TRY_CATCH_VERBOSE != 0)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__SetVerbose(this: *mut usize, value: bool) {
  let Some(try_catch) = (unsafe { try_catch_state_mut(this) }) else {
    return;
  };
  if value {
    try_catch.flags |= TRY_CATCH_VERBOSE;
  } else {
    try_catch.flags &= !TRY_CATCH_VERBOSE;
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__SetCaptureMessage(
  this: *mut usize,
  value: bool,
) {
  let Some(try_catch) = (unsafe { try_catch_state_mut(this) }) else {
    return;
  };
  if value {
    try_catch.flags |= TRY_CATCH_CAPTURE_MESSAGE;
  } else {
    try_catch.flags &= !TRY_CATCH_CAPTURE_MESSAGE;
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__Reset(this: *mut usize) {
  let Some(try_catch) = (unsafe { try_catch_state_mut(this) }) else {
    return;
  };
  // V8 deliberately keeps a rethrown exception live even if Reset is called
  // before the inner scope unwinds.
  if try_catch.flags & TRY_CATCH_RETHROWN == 0 {
    try_catch.exception = ptr::null();
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__Exception(this: *const usize) -> *const Value {
  unsafe { try_catch_state(this) }
    .map(|try_catch| try_catch.exception)
    .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TryCatch__ReThrow(this: *mut usize) -> *const Value {
  let Some(try_catch) = (unsafe { try_catch_state_mut(this) }) else {
    return ptr::null();
  };
  if try_catch.exception.is_null() {
    return ptr::null();
  }
  try_catch.flags |= TRY_CATCH_RETHROWN;
  try_catch.exception
}

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
pub extern "C" fn v8__Global__NewWeak(
  isolate: *mut RealIsolate,
  value: *const Data,
  _parameter: *const c_void,
  _callback: unsafe extern "C" fn(*const c_void),
) -> *const Data {
  if isolate.is_null() || value.is_null() {
    return ptr::null();
  }
  // HeapValue allocations never move and remain owned until Isolate::Dispose,
  // so a weak handle observes the exact same address for that lifetime. Do not
  // retain the callback parameter or invoke the callback early: rusty_v8 owns
  // WeakData and drains guaranteed finalizers from its isolate annex before
  // disposal. Retaining that borrowed parameter here would outlive WeakData.
  value
}

#[cold]
fn unexpected_weak_callback_info_access() -> ! {
  eprintln!("v8x/js2wasm: weak callback info is unreachable without engine GC");
  std::process::abort()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__WeakCallbackInfo__GetIsolate(
  _info: *const c_void,
) -> *mut RealIsolate {
  unexpected_weak_callback_info_access()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__WeakCallbackInfo__GetParameter(
  _info: *const c_void,
) -> *mut c_void {
  unexpected_weak_callback_info_access()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__WeakCallbackInfo__SetSecondPassCallback(
  _info: *const c_void,
  _callback: unsafe extern "C" fn(*const c_void),
) {
  unexpected_weak_callback_info_access()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Global__Reset(_value: *const Data) {
  // The isolate owns the allocation. Reset only releases the logical handle;
  // there is no moving collector or separate persistent cell to destroy.
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Data__EQ(left: *const Data, right: *const Data) -> bool {
  // Handles are direct pointers into the stable, isolate-owned HeapValue
  // arena. Therefore V8 identity equality is exact pointer equality.
  ptr::eq(left, right)
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
      class_name: ptr::null(),
      properties: Vec::new(),
      prototype_template,
      instance_template,
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FunctionTemplate__SetClassName(
  template: *const crate::FunctionTemplate,
  name: *const V8String,
) {
  if name.is_null() {
    return;
  }
  if let Some(HeapValue::FunctionTemplate(state)) =
    (unsafe { heap_value_mut(template) })
  {
    // Strings are stable isolate-owned allocations, so preserving this exact
    // pointer also preserves V8 handle identity when GetFunction materializes
    // the constructor.
    state.class_name = name;
  }
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
pub extern "C" fn v8__Object__New__with_prototype_and_properties(
  isolate: *mut RealIsolate,
  prototype_or_null: *const Value,
  names: *const *const crate::Name,
  values: *const *const Value,
  length: usize,
) -> *const Object {
  if isolate.is_null()
    || prototype_or_null.is_null()
    || !is_valid_prototype(prototype_or_null)
    || (length != 0 && (names.is_null() || values.is_null()))
  {
    return ptr::null();
  }

  let mut properties = Vec::with_capacity(length);
  for index in 0..length {
    let name = unsafe { *names.add(index) };
    let value = unsafe { *values.add(index) };
    if name.is_null() || value.is_null() {
      return ptr::null();
    }
    properties.push(TemplateProperty {
      key: name,
      value: value.cast(),
      attributes: 0,
    });
  }

  allocate(
    isolate,
    HeapValue::Object(ObjectState {
      prototype: Some(prototype_or_null),
      properties,
      internal_fields: Vec::new(),
    }),
  )
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

unsafe fn array_buffer_state<'a>(
  array_buffer: *const ArrayBuffer,
) -> Option<&'a ArrayBufferState> {
  match unsafe { heap_value(array_buffer) } {
    Some(HeapValue::ArrayBuffer(state)) => Some(state),
    _ => None,
  }
}

unsafe fn typed_array_state<'a, T>(
  typed_array: *const T,
) -> Option<&'a TypedArrayState> {
  match unsafe { heap_value(typed_array) } {
    Some(HeapValue::TypedArray(state)) => Some(state),
    _ => None,
  }
}

fn typed_array_backing_store(
  typed_array: &TypedArrayState,
) -> Option<&'static BackingStoreState> {
  let buffer = unsafe { array_buffer_state(typed_array.buffer) }?;
  unsafe { backing_store_state(buffer.backing_store.object.cast()) }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__New__with_backing_store(
  isolate: *mut RealIsolate,
  backing_store: *const crate::support::SharedRef<BackingStore>,
) -> *const ArrayBuffer {
  if isolate.is_null() || backing_store.is_null() {
    return ptr::null();
  }
  let repr =
    unsafe { shared_repr(backing_store.cast::<SharedPtrBase<BackingStore>>()) };
  if repr.object.is_null() || repr.references.is_null() {
    return ptr::null();
  }
  allocate(
    isolate,
    HeapValue::ArrayBuffer(ArrayBufferState {
      backing_store: retain_backing_store(repr),
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__Data(
  array_buffer: *const ArrayBuffer,
) -> *mut c_void {
  unsafe { array_buffer_state(array_buffer) }
    .and_then(|state| unsafe {
      backing_store_state(state.backing_store.object.cast())
    })
    .map(|state| state.data)
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__ByteLength(
  array_buffer: *const ArrayBuffer,
) -> usize {
  unsafe { array_buffer_state(array_buffer) }
    .and_then(|state| unsafe {
      backing_store_state(state.backing_store.object.cast())
    })
    .map(|state| state.byte_length)
    .unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBuffer__GetBackingStore(
  array_buffer: *const ArrayBuffer,
) -> crate::support::SharedRef<BackingStore> {
  let Some(state) = (unsafe { array_buffer_state(array_buffer) }) else {
    eprintln!("v8x/js2wasm: get backing store of a non-ArrayBuffer value");
    std::process::abort();
  };
  unsafe { backing_store_shared_ref(retain_backing_store(state.backing_store)) }
}

fn new_typed_array<T>(
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
  kind: TypedArrayKind,
) -> *const T {
  let Some(buffer_state) = (unsafe { array_buffer_state(buffer) }) else {
    return ptr::null();
  };
  let Some(backing_store) =
    (unsafe { backing_store_state(buffer_state.backing_store.object.cast()) })
  else {
    return ptr::null();
  };
  let element_size = kind.element_size();
  let Some(byte_length) = length.checked_mul(element_size) else {
    return ptr::null();
  };
  let Some(end) = byte_offset.checked_add(byte_length) else {
    return ptr::null();
  };
  if byte_offset % element_size != 0 || end > backing_store.byte_length {
    return ptr::null();
  }
  allocate(
    current_isolate(),
    HeapValue::TypedArray(TypedArrayState {
      buffer,
      byte_offset,
      length,
      kind,
      properties: Vec::new(),
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Uint8Array__New(
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
) -> *const Uint8Array {
  new_typed_array(buffer, byte_offset, length, TypedArrayKind::Uint8)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Uint16Array__New(
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
) -> *const Uint16Array {
  new_typed_array(buffer, byte_offset, length, TypedArrayKind::Uint16)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Uint32Array__New(
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
) -> *const Uint32Array {
  new_typed_array(buffer, byte_offset, length, TypedArrayKind::Uint32)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Int32Array__New(
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
) -> *const Int32Array {
  new_typed_array(buffer, byte_offset, length, TypedArrayKind::Int32)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigUint64Array__New(
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
) -> *const BigUint64Array {
  new_typed_array(buffer, byte_offset, length, TypedArrayKind::BigUint64)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigInt64Array__New(
  buffer: *const ArrayBuffer,
  byte_offset: usize,
  length: usize,
) -> *const BigInt64Array {
  new_typed_array(buffer, byte_offset, length, TypedArrayKind::BigInt64)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBufferView__Buffer(
  view: *const ArrayBufferView,
) -> *const ArrayBuffer {
  unsafe { typed_array_state(view) }
    .map(|state| state.buffer)
    .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBufferView__Buffer__Data(
  view: *const ArrayBufferView,
) -> *mut c_void {
  unsafe { typed_array_state(view) }
    .and_then(typed_array_backing_store)
    .map(|state| state.data)
    .unwrap_or(ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBufferView__ByteLength(
  view: *const ArrayBufferView,
) -> usize {
  unsafe { typed_array_state(view) }
    .map(|state| state.length * state.kind.element_size())
    .unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBufferView__ByteOffset(
  view: *const ArrayBufferView,
) -> usize {
  unsafe { typed_array_state(view) }
    .map(|state| state.byte_offset)
    .unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ArrayBufferView__HasBuffer(
  view: *const ArrayBufferView,
) -> bool {
  unsafe { typed_array_state(view) }.is_some()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__TypedArray__Length(
  typed_array: *const TypedArray,
) -> usize {
  unsafe { typed_array_state(typed_array) }
    .map(|state| state.length)
    .unwrap_or_default()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__Get(
  object: *const Object,
  _context: *const Context,
  key: *const Value,
) -> *const Value {
  property_on_chain(object, key.cast())
    .map(|property| property.value.cast())
    .unwrap_or_else(|| v8__Undefined(current_isolate()).cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__GetOwnPropertyNames(
  object: *const Object,
  _context: *const Context,
  filter: PropertyFilter,
  key_conversion: KeyConversionMode,
) -> *const Array {
  if object.is_null() {
    return ptr::null();
  }

  let isolate = current_isolate();
  let mut elements = Vec::new();
  let indexed_length = match unsafe { heap_value(object) } {
    Some(HeapValue::Array(state)) => state.elements.len(),
    Some(HeapValue::TypedArray(state)) => state.length,
    _ => 0,
  };
  if !matches!(key_conversion, KeyConversionMode::NoNumbers) {
    elements.extend((0..indexed_length).map(|index| match key_conversion {
      KeyConversionMode::KeepNumbers => {
        allocate(isolate, HeapValue::Number(index as f64))
      }
      KeyConversionMode::ConvertToString => {
        new_string(isolate, index.to_string()).cast()
      }
      KeyConversionMode::NoNumbers => unreachable!(),
    }));
  }

  if !filter.is_skip_strings() {
    let mut seen = HashSet::new();
    if let Some(properties) = properties(object) {
      for property in properties {
        let attributes = property.attributes;
        if (filter.is_only_writable() && attributes & 1 != 0)
          || (filter.is_only_enumerable() && attributes & (1 << 1) != 0)
          || (filter.is_only_configurable() && attributes & (1 << 2) != 0)
          || !seen.insert(property.key.addr())
        {
          continue;
        }
        elements.push(property.key.cast());
      }
    }
  }

  allocate(
    isolate,
    HeapValue::Array(ArrayState {
      elements,
      properties: Vec::new(),
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__GetPrototype(
  object: *const Object,
) -> *const Value {
  match unsafe { heap_value(object) } {
    Some(HeapValue::Object(state)) => state
      .prototype
      .unwrap_or_else(|| v8__Null(current_isolate()).cast()),
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__SetPrototype(
  object: *const Object,
  _context: *const Context,
  prototype: *const Value,
) -> MaybeBool {
  if prototype.is_null() || !is_valid_prototype(prototype) {
    return MaybeBool::Nothing;
  }
  if prototype_would_cycle(object, prototype) {
    return MaybeBool::JustFalse;
  }
  let Some(HeapValue::Object(state)) = (unsafe { heap_value_mut(object) })
  else {
    return MaybeBool::Nothing;
  };
  state.prototype = Some(prototype);
  MaybeBool::JustTrue
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__Has(
  object: *const Object,
  _context: *const Context,
  key: *const Value,
) -> MaybeBool {
  if object.is_null() || key.is_null() {
    return MaybeBool::Nothing;
  }
  if property_on_chain(object, key.cast()).is_some() {
    MaybeBool::JustTrue
  } else {
    MaybeBool::JustFalse
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__HasOwnProperty(
  object: *const Object,
  _context: *const Context,
  key: *const crate::Name,
) -> MaybeBool {
  if object.is_null() || key.is_null() {
    return MaybeBool::Nothing;
  }
  if own_property(object, key.cast()).is_some() {
    MaybeBool::JustTrue
  } else {
    MaybeBool::JustFalse
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Object__Delete(
  object: *const Object,
  _context: *const Context,
  key: *const Value,
) -> MaybeBool {
  if object.is_null() || key.is_null() {
    return MaybeBool::Nothing;
  }
  let Some(properties) = properties_mut(object) else {
    return MaybeBool::Nothing;
  };
  let Some(index) = properties
    .iter()
    .rposition(|property| same_property_key(property.key.cast(), key.cast()))
  else {
    return MaybeBool::JustTrue;
  };
  if properties[index].attributes & (1 << 2) != 0 {
    return MaybeBool::JustFalse;
  }
  properties
    .retain(|property| !same_property_key(property.key.cast(), key.cast()));
  MaybeBool::JustTrue
}

fn typed_array_element(state: &TypedArrayState, index: usize) -> Option<f64> {
  if index >= state.length {
    return None;
  }
  let backing_store = typed_array_backing_store(state)?;
  let byte_offset = state
    .byte_offset
    .checked_add(index.checked_mul(state.kind.element_size())?)?;
  let data = unsafe { backing_store.data.cast::<u8>().add(byte_offset) };
  Some(unsafe {
    match state.kind {
      TypedArrayKind::Uint8 => f64::from(data.read()),
      TypedArrayKind::Uint16 => f64::from(data.cast::<u16>().read_unaligned()),
      TypedArrayKind::Uint32 => f64::from(data.cast::<u32>().read_unaligned()),
      TypedArrayKind::Int32 => f64::from(data.cast::<i32>().read_unaligned()),
      TypedArrayKind::BigUint64 => data.cast::<u64>().read_unaligned() as f64,
      TypedArrayKind::BigInt64 => data.cast::<i64>().read_unaligned() as f64,
    }
  })
}

fn typed_array_bigint(state: &TypedArrayState, index: usize) -> Option<i128> {
  if index >= state.length {
    return None;
  }
  let backing_store = typed_array_backing_store(state)?;
  let byte_offset = state
    .byte_offset
    .checked_add(index.checked_mul(state.kind.element_size())?)?;
  let data = unsafe { backing_store.data.cast::<u8>().add(byte_offset) };
  match state.kind {
    TypedArrayKind::BigUint64 => {
      Some(i128::from(unsafe { data.cast::<u64>().read_unaligned() }))
    }
    TypedArrayKind::BigInt64 => {
      Some(i128::from(unsafe { data.cast::<i64>().read_unaligned() }))
    }
    _ => None,
  }
}

fn integer_modulo(number: f64, modulus: f64) -> f64 {
  if !number.is_finite() || number == 0.0 {
    0.0
  } else {
    number.trunc().rem_euclid(modulus)
  }
}

fn set_typed_array_element(
  state: &TypedArrayState,
  index: usize,
  number: f64,
) -> bool {
  if index >= state.length {
    return true;
  }
  let Some(backing_store) = typed_array_backing_store(state) else {
    return false;
  };
  let Some(byte_offset) = index
    .checked_mul(state.kind.element_size())
    .and_then(|offset| state.byte_offset.checked_add(offset))
  else {
    return false;
  };
  let data = unsafe { backing_store.data.cast::<u8>().add(byte_offset) };
  unsafe {
    match state.kind {
      TypedArrayKind::Uint8 => {
        data.write(integer_modulo(number, 256.0) as u8);
      }
      TypedArrayKind::Uint16 => {
        data
          .cast::<u16>()
          .write_unaligned(integer_modulo(number, 65_536.0) as u16);
      }
      TypedArrayKind::Uint32 => {
        data
          .cast::<u32>()
          .write_unaligned(integer_modulo(number, 4_294_967_296.0) as u32);
      }
      TypedArrayKind::Int32 => {
        data.cast::<i32>().write_unaligned(
          (integer_modulo(number, 4_294_967_296.0) as u32) as i32,
        );
      }
      TypedArrayKind::BigUint64 => {
        data.cast::<u64>().write_unaligned(number as u64);
      }
      TypedArrayKind::BigInt64 => {
        data.cast::<i64>().write_unaligned(number as i64);
      }
    }
  }
  true
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
    Some(HeapValue::TypedArray(state)) => {
      typed_array_element(state, index as usize)
        .map(|value| allocate(current_isolate(), HeapValue::Number(value)))
        .unwrap_or_else(|| v8__Undefined(current_isolate()).cast())
    }
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
  if let Some(HeapValue::TypedArray(state)) = unsafe { heap_value(object) } {
    let Some(HeapValue::Number(number)) = (unsafe { heap_value(value) }) else {
      return MaybeBool::Nothing;
    };
    return if set_typed_array_element(state, index as usize, *number) {
      MaybeBool::JustTrue
    } else {
      MaybeBool::Nothing
    };
  }
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
  allocate_function(current_isolate(), callback, data, Vec::new(), ptr::null())
}

fn allocate_function(
  isolate: *mut RealIsolate,
  callback: crate::FunctionCallback,
  data: *const Value,
  mut properties: Vec<TemplateProperty>,
  name: *const V8String,
) -> *const crate::Function {
  let name = if name.is_null() {
    new_string(isolate, String::new())
  } else {
    name
  };
  let name_key = new_string(isolate, "name".to_string());
  if let Some(property) = properties
    .iter_mut()
    .rev()
    .find(|property| same_property_key(property.key.cast(), name_key.cast()))
  {
    property.value = name.cast();
  } else {
    properties.push(TemplateProperty {
      key: name_key.cast(),
      value: name.cast(),
      attributes: 0,
    });
  }
  allocate(
    isolate,
    HeapValue::Function(FunctionState {
      callback,
      data,
      name,
      properties,
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
  allocate_function(
    current_isolate(),
    state.callback,
    state.data,
    function_properties,
    state.class_name,
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Function__GetName(
  function: *const crate::Function,
) -> *const V8String {
  match unsafe { heap_value(function) } {
    Some(HeapValue::Function(state)) => state.name,
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Function__SetName(
  function: *const crate::Function,
  name: *const V8String,
) {
  if name.is_null() {
    return;
  }
  let name_key = new_string(current_isolate(), "name".to_string());
  let Some(HeapValue::Function(state)) = (unsafe { heap_value_mut(function) })
  else {
    return;
  };
  state.name = name;
  if let Some(property) = state
    .properties
    .iter_mut()
    .rev()
    .find(|property| same_property_key(property.key.cast(), name_key.cast()))
  {
    property.value = name.cast();
  } else {
    state.properties.push(TemplateProperty {
      key: name_key.cast(),
      value: name.cast(),
      attributes: 0,
    });
  }
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

fn prelinked_deno_exception(value: *const Value) -> (u32, String) {
  match unsafe { heap_value(value) } {
    Some(HeapValue::Error { name, message }) => {
      let kind = if *name == "TypeError" { 1 } else { 2 };
      (kind, message.clone())
    }
    Some(HeapValue::String(message)) => (2, message.clone()),
    _ => (
      2,
      "Rust Deno op threw an unsupported exception value".to_string(),
    ),
  }
}

fn invoke_prelinked_deno_function(
  function: usize,
  arguments: &[*const Value],
) -> Result<*const Value, (u32, String)> {
  let isolate = current_isolate();
  if isolate.is_null() {
    return Err((2, "Rust Deno op has no current isolate".to_string()));
  }
  let function = function as *const crate::Function;
  if !matches!(
    unsafe { heap_value(function) },
    Some(HeapValue::Function(_))
  ) {
    return Err((2, "Rust Deno op handle is not a Function".to_string()));
  }

  let mut try_catch = [0_usize; 6];
  v8__TryCatch__CONSTRUCT(try_catch.as_mut_ptr(), isolate);
  let receiver = v8__Undefined(isolate).cast();
  let result = invoke_function(
    function,
    receiver,
    arguments.len().try_into().unwrap_or(int::MAX),
    arguments.as_ptr(),
    false,
  );
  let exception = v8__TryCatch__Exception(try_catch.as_ptr());
  let outcome = if exception.is_null() {
    if result.is_null() {
      Err((
        2,
        "Rust Deno op returned an empty value without throwing".to_string(),
      ))
    } else {
      Ok(result)
    }
  } else {
    Err(prelinked_deno_exception(exception))
  };
  v8__TryCatch__Reset(try_catch.as_mut_ptr());
  v8__TryCatch__DESTRUCT(try_catch.as_mut_ptr());
  outcome
}

pub(crate) fn invoke_prelinked_deno_sum(
  function: usize,
  is_array: bool,
  values: &[f64],
) -> Result<f64, (u32, String)> {
  let isolate = current_isolate();
  if isolate.is_null() {
    return Err((2, "Rust Deno op_sum has no current isolate".to_string()));
  }
  let argument: *const Value = if is_array {
    let elements = values
      .iter()
      .map(|value| v8__Number__New(isolate, *value).cast())
      .collect();
    allocate(
      isolate,
      HeapValue::Array(ArrayState {
        elements,
        properties: Vec::new(),
      }),
    )
  } else {
    let Some(value) = values.first().copied().filter(|_| values.len() == 1)
    else {
      return Err((
        2,
        "Rust Deno op_sum scalar bridge expected exactly one value".to_string(),
      ));
    };
    v8__Number__New(isolate, value).cast()
  };
  let result = invoke_prelinked_deno_function(function, &[argument])?;
  match unsafe { heap_value(result) } {
    Some(HeapValue::Number(value)) => Ok(*value),
    _ => Err((
      2,
      "Rust Deno op_sum returned a non-number value".to_string(),
    )),
  }
}

pub(crate) fn invoke_prelinked_deno_print(
  function: usize,
  units: &[u16],
  is_error: bool,
) -> Result<(), (u32, String)> {
  let isolate = current_isolate();
  if isolate.is_null() {
    return Err((2, "Rust Deno op_print has no current isolate".to_string()));
  }
  let message = new_string(isolate, String::from_utf16_lossy(units));
  let is_error = v8__Boolean__New(isolate, is_error).cast();
  invoke_prelinked_deno_function(function, &[message.cast(), is_error])?;
  Ok(())
}

pub(crate) fn invoke_prelinked_deno_test_fn(
  function: usize,
) -> Result<Option<String>, (u32, String)> {
  let result = invoke_prelinked_deno_function(function, &[])?;
  if matches!(unsafe { heap_value(result) }, Some(HeapValue::Undefined)) {
    return Ok(None);
  }
  let value =
    heap_to_json_value(result, &mut HashSet::new()).ok_or_else(|| {
      (
        2,
        "Rust test_fn returned a value unsupported by the scalar JSON bridge"
          .to_string(),
      )
    })?;
  serde_json::to_string(&value)
    .map(Some)
    .map_err(|error| (2, format!("encode Rust test_fn result: {error}")))
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
pub extern "C" fn v8__String__Utf8Length(
  value: *const V8String,
  _isolate: *mut RealIsolate,
) -> int {
  unsafe {
    string_value(value)
      .map(|value| value.len() as int)
      .unwrap_or(0)
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__Write_v2(
  value: *const V8String,
  _isolate: *mut RealIsolate,
  offset: u32,
  length: u32,
  buffer: *mut u16,
  _flags: int,
) {
  if buffer.is_null() {
    return;
  }
  let Some(value) = (unsafe { string_value(value) }) else {
    return;
  };
  for (target, unit) in value
    .encode_utf16()
    .skip(offset as usize)
    .take(length as usize)
    .enumerate()
  {
    unsafe { buffer.add(target).write(unit) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__WriteOneByte_v2(
  value: *const V8String,
  _isolate: *mut RealIsolate,
  offset: u32,
  length: u32,
  buffer: *mut u8,
  _flags: int,
) {
  if buffer.is_null() {
    return;
  }
  let Some(value) = (unsafe { string_value(value) }) else {
    return;
  };
  for (target, unit) in value
    .encode_utf16()
    .skip(offset as usize)
    .take(length as usize)
    .enumerate()
  {
    unsafe { buffer.add(target).write(unit as u8) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__String__WriteUtf8_v2(
  value: *const V8String,
  _isolate: *mut RealIsolate,
  buffer: *mut c_char,
  capacity: usize,
  _flags: int,
  processed_characters_return: *mut usize,
) -> int {
  if !processed_characters_return.is_null() {
    unsafe { processed_characters_return.write(0) };
  }
  if buffer.is_null() {
    return 0;
  }
  let Some(value) = (unsafe { string_value(value) }) else {
    return 0;
  };

  let mut written = 0;
  let mut processed = 0;
  for character in value.chars() {
    let mut encoded = [0; 4];
    let bytes = character.encode_utf8(&mut encoded).as_bytes();
    if written + bytes.len() > capacity {
      break;
    }
    unsafe {
      ptr::copy_nonoverlapping(
        bytes.as_ptr(),
        buffer.cast::<u8>().add(written),
        bytes.len(),
      );
    }
    written += bytes.len();
    processed += character.len_utf16();
  }
  if written < capacity {
    unsafe { buffer.cast::<u8>().add(written).write(0) };
  }
  if !processed_characters_return.is_null() {
    unsafe { processed_characters_return.write(processed) };
  }
  written.try_into().unwrap_or(int::MAX)
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
pub extern "C" fn v8__Value__IsTrue(value: *const Value) -> bool {
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
pub extern "C" fn v8__BigInt__New(
  isolate: *mut RealIsolate,
  value: i64,
) -> *const BigInt {
  allocate(isolate, HeapValue::BigInt(i128::from(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigInt__NewFromUnsigned(
  isolate: *mut RealIsolate,
  value: u64,
) -> *const BigInt {
  allocate(isolate, HeapValue::BigInt(i128::from(value)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigInt__NewFromWords(
  _context: *const Context,
  sign_bit: int,
  word_count: int,
  words: *const u64,
) -> *const BigInt {
  if word_count < 0 || (word_count > 0 && words.is_null()) {
    return ptr::null();
  }
  let words = if word_count == 0 {
    &[][..]
  } else {
    unsafe { std::slice::from_raw_parts(words, word_count as usize) }
  };
  let mut magnitude = 0_u128;
  for (index, word) in words.iter().take(2).enumerate() {
    magnitude |= u128::from(*word) << (index * 64);
  }
  let value = if sign_bit != 0 {
    -(magnitude as i128)
  } else {
    magnitude as i128
  };
  allocate(current_isolate(), HeapValue::BigInt(value))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigInt__Uint64Value(
  value: *const BigInt,
  lossless: *mut bool,
) -> u64 {
  let Some(HeapValue::BigInt(value)) = (unsafe { heap_value(value) }) else {
    if !lossless.is_null() {
      unsafe { lossless.write(false) };
    }
    return 0;
  };
  let converted = *value as u64;
  if !lossless.is_null() {
    unsafe { lossless.write(*value >= 0 && i128::from(converted) == *value) };
  }
  converted
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigInt__Int64Value(
  value: *const BigInt,
  lossless: *mut bool,
) -> i64 {
  let Some(HeapValue::BigInt(value)) = (unsafe { heap_value(value) }) else {
    if !lossless.is_null() {
      unsafe { lossless.write(false) };
    }
    return 0;
  };
  let converted = *value as i64;
  if !lossless.is_null() {
    unsafe { lossless.write(i128::from(converted) == *value) };
  }
  converted
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigInt__WordCount(value: *const BigInt) -> int {
  match unsafe { heap_value(value) } {
    Some(HeapValue::BigInt(0)) => 0,
    Some(HeapValue::BigInt(value))
      if value.unsigned_abs() > u128::from(u64::MAX) =>
    {
      2
    }
    Some(HeapValue::BigInt(_)) => 1,
    _ => 0,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__BigInt__ToWordsArray(
  value: *const BigInt,
  sign_bit: *mut int,
  word_count: *mut int,
  words: *mut u64,
) {
  let Some(HeapValue::BigInt(value)) = (unsafe { heap_value(value) }) else {
    return;
  };
  let available = if word_count.is_null() {
    0
  } else {
    unsafe { (*word_count).max(0) as usize }
  };
  let magnitude = value.unsigned_abs();
  let required =
    usize::from(magnitude > 0) + usize::from(magnitude > u128::from(u64::MAX));
  if !sign_bit.is_null() {
    unsafe { sign_bit.write(int::from(*value < 0)) };
  }
  if !words.is_null() {
    if available > 0 {
      unsafe { words.write(magnitude as u64) };
    }
    if available > 1 {
      unsafe { words.add(1).write((magnitude >> 64) as u64) };
    }
  }
  if !word_count.is_null() {
    unsafe { word_count.write(required.min(available) as int) };
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Number__Value(value: *const Number) -> f64 {
  match unsafe { heap_value(value) } {
    Some(HeapValue::Number(value)) => *value,
    _ => f64::NAN,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__NumberValue(
  value: *const Value,
  context: *const Context,
  out: *mut Maybe<f64>,
) {
  let value =
    if matches!(unsafe { heap_value(context) }, Some(HeapValue::Context(_))) {
      match unsafe { heap_value(value) } {
        Some(HeapValue::Number(value)) => Some(*value),
        _ => None,
      }
    } else {
      None
    };
  unsafe { write_maybe(out, value) };
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
pub extern "C" fn v8__Value__IsName(value: *const Value) -> bool {
  v8__Value__IsString(value)
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
        | HeapValue::ArrayBuffer(_)
        | HeapValue::TypedArray(_)
        | HeapValue::Function(_)
        | HeapValue::Promise(_)
        | HeapValue::PromiseResolver(_)
        | HeapValue::Error { .. }
    )
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__ToObject(
  value: *const Value,
  _context: *const Context,
) -> *const Object {
  match unsafe { heap_value(value) } {
    Some(
      HeapValue::Object(_)
      | HeapValue::Array(_)
      | HeapValue::ArrayBuffer(_)
      | HeapValue::TypedArray(_)
      | HeapValue::Function(_)
      | HeapValue::Promise(_)
      | HeapValue::PromiseResolver(_)
      | HeapValue::Error { .. },
    ) => value.cast(),
    Some(HeapValue::Null | HeapValue::Undefined) | None => {
      let message = new_string(
        current_isolate(),
        "Cannot convert undefined or null to object".to_string(),
      );
      let exception = allocate_error(message, "TypeError");
      record_exception(current_isolate(), exception);
      ptr::null()
    }
    Some(_) => {
      let object = new_object(current_isolate());
      let _ = set_named_property(object, "value", value);
      object
    }
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__ToString(
  value: *const Value,
  _context: *const Context,
) -> *const V8String {
  let isolate = current_isolate();
  match unsafe { heap_value(value) } {
    Some(HeapValue::String(_)) => value.cast(),
    Some(HeapValue::Boolean(value)) => new_string(isolate, value.to_string()),
    Some(HeapValue::Number(value)) => {
      let string = if value.is_nan() {
        "NaN".to_string()
      } else if *value == f64::INFINITY {
        "Infinity".to_string()
      } else if *value == f64::NEG_INFINITY {
        "-Infinity".to_string()
      } else if *value == 0.0 {
        "0".to_string()
      } else {
        value.to_string()
      };
      new_string(isolate, string)
    }
    Some(HeapValue::BigInt(value)) => new_string(isolate, value.to_string()),
    Some(HeapValue::Null) => new_string(isolate, "null".to_string()),
    Some(HeapValue::Undefined) => new_string(isolate, "undefined".to_string()),
    Some(HeapValue::Error { name, message }) => {
      new_string(isolate, format!("{name}: {message}"))
    }
    Some(HeapValue::Array(_)) => new_string(isolate, String::new()),
    Some(
      HeapValue::Object(_)
      | HeapValue::Function(_)
      | HeapValue::TypedArray(_)
      | HeapValue::ArrayBuffer(_),
    ) => new_string(isolate, "[object Object]".to_string()),
    Some(_) | None => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsModuleNamespaceObject(
  value: *const Value,
) -> bool {
  if value.is_null() {
    return false;
  }
  let isolate = current_isolate();
  if isolate.is_null() {
    return false;
  }

  // Source-text and synthetic namespaces are stable Rust-owned objects. Keep
  // their module-exotic brand at the owning ModuleState boundary rather than
  // teaching every ordinary Object operation about a new storage variant.
  // A Local<Value> is only valid in its isolate, so the isolate-owned module
  // table is also the exact identity set this predicate may observe.
  unsafe { isolate_state(isolate) }
    .values
    .iter()
    .any(|candidate| {
      matches!(
        unsafe { candidate.as_ref() },
        Some(HeapValue::Module(state))
          if std::ptr::addr_eq(state.namespace, value)
      )
    })
}

macro_rules! unsupported_value_predicates {
  ($($name:ident),* $(,)?) => {
    $(
      #[unsafe(no_mangle)]
      pub extern "C" fn $name(_value: *const Value) -> bool {
        false
      }
    )*
  };
}

// These brands are queried, in this order, by rusty_v8's Value::type_repr()
// before it reaches Number. None has a corresponding HeapValue variant in
// this backend, so a strong false answer is exact and avoids falling through
// to the diagnostic abort stubs while formatting serde_v8 type errors.
unsupported_value_predicates!(
  v8__Value__IsWasmModuleObject,
  v8__Value__IsWasmMemoryObject,
  v8__Value__IsProxy,
  v8__Value__IsSharedArrayBuffer,
  v8__Value__IsDataView,
  v8__Value__IsFloat64Array,
  v8__Value__IsFloat32Array,
  v8__Value__IsInt16Array,
  v8__Value__IsInt8Array,
  v8__Value__IsUint8ClampedArray,
  v8__Value__IsWeakSet,
  v8__Value__IsWeakMap,
  v8__Value__IsSetIterator,
  v8__Value__IsMapIterator,
  v8__Value__IsSet,
  v8__Value__IsMap,
  v8__Value__IsGeneratorFunction,
  v8__Value__IsAsyncFunction,
  v8__Value__IsRegExp,
  v8__Value__IsDate,
  v8__Value__IsSymbol,
  v8__Value__IsSymbolObject,
);

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsArray(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Array(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsArrayBuffer(value: *const Value) -> bool {
  matches!(
    unsafe { heap_value(value) },
    Some(HeapValue::ArrayBuffer(_))
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsArrayBufferView(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::TypedArray(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsTypedArray(value: *const Value) -> bool {
  v8__Value__IsArrayBufferView(value)
}

fn is_typed_array_kind(value: *const Value, kind: TypedArrayKind) -> bool {
  matches!(
    unsafe { heap_value(value) },
    Some(HeapValue::TypedArray(state)) if state.kind == kind
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsUint8Array(value: *const Value) -> bool {
  is_typed_array_kind(value, TypedArrayKind::Uint8)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsUint16Array(value: *const Value) -> bool {
  is_typed_array_kind(value, TypedArrayKind::Uint16)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsUint32Array(value: *const Value) -> bool {
  is_typed_array_kind(value, TypedArrayKind::Uint32)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsInt32Array(value: *const Value) -> bool {
  is_typed_array_kind(value, TypedArrayKind::Int32)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsBigUint64Array(value: *const Value) -> bool {
  is_typed_array_kind(value, TypedArrayKind::BigUint64)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsBigInt64Array(value: *const Value) -> bool {
  is_typed_array_kind(value, TypedArrayKind::BigInt64)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsBigInt(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::BigInt(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsFunction(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Function(_)))
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsPromise(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Promise(_)))
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
pub extern "C" fn v8__Exception__CreateMessage(
  _isolate: *mut RealIsolate,
  exception: *const Value,
) -> *const Message {
  exception.cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Exception__GetStackTrace(
  _exception: *const Value,
) -> *const StackTrace {
  ptr::null()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Message__Get(message: *const Message) -> *const V8String {
  match unsafe { heap_value(message) } {
    Some(HeapValue::Error { name, message }) => {
      new_string(current_isolate(), format!("Uncaught {name}: {message}"))
    }
    Some(HeapValue::String(message)) => {
      new_string(current_isolate(), format!("Uncaught {message}"))
    }
    _ => new_string(current_isolate(), "Uncaught Error".to_string()),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Message__GetScriptResourceName(
  _message: *const Message,
) -> *const Value {
  v8__Undefined(current_isolate()).cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Message__GetLineNumber(
  _message: *const Message,
  _context: *const Context,
) -> int {
  -1
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Message__GetStartColumn(_message: *const Message) -> int {
  0
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Message__GetStackTrace(
  _message: *const Message,
) -> *const StackTrace {
  ptr::null()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Value__IsNativeError(value: *const Value) -> bool {
  matches!(unsafe { heap_value(value) }, Some(HeapValue::Error { .. }))
}

// --- Promises -------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__Resolver__New(
  context: *const Context,
) -> *const PromiseResolver {
  if context.is_null()
    || !matches!(unsafe { heap_value(context) }, Some(HeapValue::Context(_)))
  {
    return ptr::null();
  }
  let isolate = current_isolate();
  if isolate.is_null() {
    return ptr::null();
  }
  let promise =
    allocate_promise(isolate, PromiseSettlement::Pending, ptr::null());
  allocate(
    isolate,
    HeapValue::PromiseResolver(PromiseResolverState { promise }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__Resolver__GetPromise(
  resolver: *const PromiseResolver,
) -> *const Promise {
  unsafe { promise_resolver_state(resolver) }
    .map(|state| state.promise)
    .unwrap_or(ptr::null())
}

fn settle_promise(
  resolver: *const PromiseResolver,
  context: *const Context,
  value: *const Value,
  settlement: PromiseSettlement,
) -> MaybeBool {
  if context.is_null()
    || value.is_null()
    || !matches!(unsafe { heap_value(context) }, Some(HeapValue::Context(_)))
  {
    return MaybeBool::Nothing;
  }
  let Some(promise) =
    (unsafe { promise_resolver_state(resolver) }).map(|state| state.promise)
  else {
    return MaybeBool::Nothing;
  };
  let Some(state) = (unsafe { promise_state(promise) }) else {
    return MaybeBool::Nothing;
  };
  if state.settlement == PromiseSettlement::Pending {
    state.settlement = settlement;
    state.result = value;
  }
  MaybeBool::JustTrue
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__Resolver__Resolve(
  resolver: *const PromiseResolver,
  context: *const Context,
  value: *const Value,
) -> MaybeBool {
  settle_promise(resolver, context, value, PromiseSettlement::Fulfilled)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__Resolver__Reject(
  resolver: *const PromiseResolver,
  context: *const Context,
  value: *const Value,
) -> MaybeBool {
  settle_promise(resolver, context, value, PromiseSettlement::Rejected)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__State(promise: *const Promise) -> PromiseState {
  match unsafe { promise_state(promise) }
    .map(|state| state.settlement)
    .unwrap_or(PromiseSettlement::Pending)
  {
    PromiseSettlement::Pending => PromiseState::Pending,
    PromiseSettlement::Fulfilled => PromiseState::Fulfilled,
    PromiseSettlement::Rejected => PromiseState::Rejected,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__HasHandler(promise: *const Promise) -> bool {
  unsafe { promise_state(promise) }.is_some_and(|state| state.handled)
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__MarkAsHandled(promise: *const Promise) {
  if let Some(state) = unsafe { promise_state(promise) } {
    state.handled = true;
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Promise__Result(promise: *const Promise) -> *const Value {
  let Some(state) = (unsafe { promise_state(promise) }) else {
    return ptr::null();
  };
  if state.settlement == PromiseSettlement::Pending {
    ptr::null()
  } else {
    state.result
  }
}

// --- Classic scripts ------------------------------------------------------

const DENO_CORE_PRELINKED_SCRIPTS: [(&str, u64); 4] = [
  ("ext:core/00_primordials.js", 0x49d0_171d_7d2c_3f4d),
  ("ext:core/00_infra.js", 0xe1a2_6738_75ca_364c),
  ("ext:core/02_timers.js", 0xcbd2_6ee0_c68d_cb66),
  ("ext:core/01_core.js", 0xd2f9_d9c6_2c03_7a70),
];
#[cfg(feature = "js2wasm_deno_poc_replay")]
const DENO_CORE_REPLAY_SHA256: [&str; 4] = [
  "5a2dfbdc4bb81412575d035901a11788001c7e0110e3f736d16289891af44a52",
  "33984000be930f3b02a2d1149ac0319724e8d95891623c8cc74699da4ce97287",
  "305596528c679be30d0ac61fa049ec0f1777c287054d119ff4b341575afac7f9",
  "6e67972322cc5385a2b642a4f7e941fccb6f992c9de662a5111d11fd0aaf1a3a",
];
// The pinned Deno integration patch skips globals that the compatibility shim
// has already installed. The compiled artifact is still built from the
// pristine DENO_REF source, so accept this one audited host-side variant while
// keeping every other bootstrap script hash-exact.
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
const PATCHED_DENO_CORE_01_CORE_HASH: u64 = 0x9a86_06e5_0118_e568;
const DENO_CORE_MODULE_SPECIFIER: &str = "ext:core/mod.js";
const DENO_CORE_MODULE_HASH: u64 = 0xcb8e_ac50_51e4_21a4;
#[cfg(feature = "js2wasm_deno_poc_replay")]
const DENO_CORE_MODULE_SHA256: &str =
  "6850db621a5325d8737ad87d2d24cbc35b7010d5e5f36c88dc53c16610cc40e5";
const DENO_CORE_USAGE_SPECIFIER: &str = "<usage>";
const DENO_CORE_USAGE_HASH: u64 = 0xd9c8_b2cb_5b20_c3bc;
#[cfg(feature = "js2wasm_deno_poc_replay")]
const DENO_CORE_USAGE_SHA256: &str =
  "33bf6b9698833319ad98c0cf88f2fb4dd7634859816ec784aa8902b3eeba1804";

fn fnv1a64(bytes: &[u8]) -> u64 {
  let mut hash = 0xcbf2_9ce4_8422_2325_u64;
  for byte in bytes {
    hash ^= u64::from(*byte);
    hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
  }
  hash
}

#[cfg(feature = "js2wasm_deno_poc_replay")]
fn sha256_hex(bytes: &[u8]) -> String {
  format!("{:x}", Sha256::digest(bytes))
}

fn is_prelinked_deno_module_source(source: &str) -> bool {
  #[cfg(feature = "js2wasm_deno_poc_replay")]
  {
    sha256_hex(source.as_bytes()) == DENO_CORE_MODULE_SHA256
  }
  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  {
    fnv1a64(source.as_bytes()) == DENO_CORE_MODULE_HASH
  }
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

const PRELINKED_DENO_CORE_CALLBACKS: &[&str] = &[
  "__eventLoopTick",
  "__processTimers",
  "__drainNextTickAndMacrotasks",
  "__handleRejections",
  "buildCustomError",
  "runImmediateCallbacks",
  "__setTickInfo",
  "__setImmediateInfo",
  "__setTimerInfo",
];

fn exact_typed_array_values(
  value: *const Value,
  expected_kind: TypedArrayKind,
  expected_length: usize,
  name: &str,
) -> Result<Vec<f64>, String> {
  let Some(state) = (unsafe { typed_array_state(value) }) else {
    return Err(format!(
      "prelinked Deno callback {name} expected a typed-array argument"
    ));
  };
  if state.kind != expected_kind || state.length != expected_length {
    return Err(format!(
      "prelinked Deno callback {name} received the wrong typed-array kind or length"
    ));
  }
  (0..state.length)
    .map(|index| {
      typed_array_element(state, index).ok_or_else(|| {
        format!("prelinked Deno callback {name} could not read element {index}")
      })
    })
    .collect()
}

fn invoke_prelinked_deno_setter(
  name: &str,
  argument: *const Value,
) -> Result<(), String> {
  let context = current_context();
  match name {
    "__setTickInfo" => {
      let values =
        exact_typed_array_values(argument, TypedArrayKind::Uint8, 2, name)?;
      with_deno_core_runtime(context, name, |runtime| {
        runtime.set_deno_tick_info([values[0] as u8, values[1] as u8])
      })
    }
    "__setImmediateInfo" => {
      let values =
        exact_typed_array_values(argument, TypedArrayKind::Uint32, 3, name)?;
      with_deno_core_runtime(context, name, |runtime| {
        runtime.set_deno_immediate_info([
          values[0] as u32,
          values[1] as u32,
          values[2] as u32,
        ])
      })
    }
    "__setTimerInfo" => {
      let values =
        exact_typed_array_values(argument, TypedArrayKind::Int32, 1, name)?;
      with_deno_core_runtime(context, name, |runtime| {
        runtime.set_deno_timer_info(values[0] as i32)
      })
    }
    _ => Err(format!("prelinked Deno callback {name} is not a setter")),
  }
}

fn throw_prelinked_callback_error(
  info: &mut CallbackInfoState,
  message: String,
) {
  let message = new_string(info.isolate, message);
  let exception = allocate_error(message, "Error");
  v8__Isolate__ThrowException(info.isolate, exception);
  *info.return_slot = ptr::null();
}

fn prelinked_error_name(class: &str) -> &'static str {
  match class {
    "RangeError" => "RangeError",
    "ReferenceError" => "ReferenceError",
    "SyntaxError" => "SyntaxError",
    "TypeError" => "TypeError",
    "URIError" => "URIError",
    _ => "Error",
  }
}

fn build_prelinked_custom_error(
  info: &mut CallbackInfoState,
) -> Result<(), String> {
  let class = info
    .args
    .first()
    .and_then(|value| unsafe { string_value(*value) })
    .ok_or_else(|| {
      "prelinked Deno buildCustomError expected a string class".to_string()
    })?;
  let message = info
    .args
    .get(1)
    .copied()
    .and_then(|value| unsafe { string_value(value) })
    .ok_or_else(|| {
      "prelinked Deno buildCustomError expected a string message".to_string()
    })?;
  let message = new_string(info.isolate, message.to_owned());
  *info.return_slot = allocate_error(message, prelinked_error_name(class));
  Ok(())
}

unsafe extern "C" fn prelinked_deno_core_callback(
  info: *const crate::function::FunctionCallbackInfo,
) {
  let Some(info) = (unsafe { callback_info(info) }) else {
    return;
  };
  let name = unsafe { string_value(info.data) }
    .unwrap_or("<unknown>")
    .to_owned();
  if name == "buildCustomError" {
    if let Err(error) = build_prelinked_custom_error(info) {
      throw_prelinked_callback_error(info, error);
    }
    return;
  }
  if matches!(
    name.as_str(),
    "__setTickInfo" | "__setImmediateInfo" | "__setTimerInfo"
  ) {
    let argument = info
      .args
      .first()
      .copied()
      .unwrap_or_else(|| v8__Undefined(info.isolate).cast());
    match invoke_prelinked_deno_setter(&name, argument) {
      Ok(()) => {
        *info.return_slot = v8__Undefined(info.isolate).cast();
      }
      Err(error) => throw_prelinked_callback_error(info, error),
    }
    return;
  }
  throw_prelinked_callback_error(
    info,
    format!("prelinked Deno callback {name} is not bridged yet"),
  );
}

fn install_prelinked_deno_core_callbacks(
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
  for name in PRELINKED_DENO_CORE_CALLBACKS {
    let name_value = new_string(current_isolate(), (*name).to_string());
    let function = allocate_function(
      current_isolate(),
      prelinked_deno_core_callback,
      name_value.cast(),
      Vec::new(),
      name_value,
    );
    set_named_property(core, name, function.cast())?;
  }
  let error_constructors = new_object(current_isolate());
  set_named_property(core, "errorConstructors", error_constructors.cast())
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
  let stub = allocate_function(
    current_isolate(),
    prelinked_set_up_async_stub,
    ptr::null(),
    Vec::new(),
    ptr::null(),
  );
  set_named_property(core, "setUpAsyncStub", stub.cast())
}

fn resolve_prelinked_deno_ops(
  context: *const Context,
) -> Result<(Option<usize>, Option<usize>), String> {
  let global = match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state.global,
    _ => return Err("classic script has no live v8x context".to_string()),
  };
  let deno = named_property(global, "Deno")
    .ok_or_else(|| "Rust-owned global has no Deno object".to_string())?;
  let core = named_property(deno, "core")
    .ok_or_else(|| "Rust-owned Deno object has no core object".to_string())?;
  let ops = named_property(core, "ops")
    .ok_or_else(|| "Rust-owned Deno.core has no ops object".to_string())?;
  let resolve = |name| {
    let Some(function) = named_property(ops, name) else {
      return Ok(None);
    };
    if !matches!(
      unsafe { heap_value(function) },
      Some(HeapValue::Function(_))
    ) {
      return Err(format!("Rust-owned Deno.core.ops.{name} is not a Function"));
    }
    Ok(Some(function as usize))
  };
  Ok((resolve("op_print")?, resolve("op_sum")?))
}

fn resolve_prelinked_test_fn(
  context: *const Context,
) -> Result<Option<usize>, String> {
  let global = match unsafe { heap_value(context) } {
    Some(HeapValue::Context(state)) => state.global,
    _ => return Err("classic script has no live v8x context".to_string()),
  };
  let Some(function) = named_property(global, "test_fn") else {
    return Ok(None);
  };
  if !matches!(
    unsafe { heap_value(function) },
    Some(HeapValue::Function(_))
  ) {
    return Err("Rust-owned global test_fn is not a Function".to_string());
  }
  Ok(Some(function as usize))
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

  #[cfg(feature = "js2wasm_deno_poc_replay")]
  {
    let actual_sha256 = sha256_hex(state.source.as_bytes());
    let expected_sha256 = DENO_CORE_REPLAY_SHA256[phase];
    if actual_sha256 != expected_sha256 {
      return Err(format!(
        "closed-world Deno replay source {:?} has SHA-256 {actual_sha256}, expected {expected_sha256}",
        state.specifier,
      ));
    }
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  {
    let actual_hash = fnv1a64(state.source.as_bytes());
    let is_patched_01_core = state.specifier == "ext:core/01_core.js"
      && actual_hash == PATCHED_DENO_CORE_01_CORE_HASH;
    if actual_hash != *expected_hash && !is_patched_01_core {
      return Err(format!(
        "prelinked script {:?} has FNV-1a hash {actual_hash:#018x}, expected {expected_hash:#018x}",
        state.specifier,
      ));
    }
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
  let final_wrapper = phase + 1 == DENO_CORE_PRELINKED_SCRIPTS.len();
  let deno_ops = if final_wrapper {
    Some(resolve_prelinked_deno_ops(context)?)
  } else {
    None
  };
  if let Some(runtime) = runtime {
    let Some(HeapValue::Context(context_state)) =
      (unsafe { heap_value_mut(context) })
    else {
      return Err("classic script context disappeared".to_string());
    };
    context_state.deno_core_bootstrap = Some(Rc::new(RefCell::new(runtime)));
  }
  if final_wrapper {
    let (print, sum) = deno_ops
      .ok_or_else(|| "prelinked Deno op handles disappeared".to_string())?;
    with_deno_core_runtime(context, "wrapper initialization", |runtime| {
      runtime.bind_deno_ops(print, sum)?;
      #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
      {
        // Older three/four-wrapper artifacts expose only the legacy bootstrap
        // probe. The staged artifact advances an explicit state machine here so
        // the later module evaluation re-enters this exact Wasmtime instance.
        let _ = runtime.advance_deno_core_wrappers()?;
      }
      Ok(())
    })?;
  }
  let Some(HeapValue::Context(context_state)) =
    (unsafe { heap_value_mut(context) })
  else {
    return Err("classic script context disappeared".to_string());
  };
  context_state.deno_core_bootstrap_phase += 1;
  if final_wrapper {
    install_prelinked_deno_core_callbacks(context)?;
  }
  Ok(true)
}

fn run_prelinked_deno_usage_script(
  context: *const Context,
  state: &ScriptState,
) -> Result<bool, String> {
  if state.specifier != DENO_CORE_USAGE_SPECIFIER {
    return Ok(false);
  }

  #[cfg(feature = "js2wasm_deno_poc_replay")]
  {
    let actual_sha256 = sha256_hex(state.source.as_bytes());
    if actual_sha256 != DENO_CORE_USAGE_SHA256 {
      return Err(format!(
        "closed-world Deno replay source {:?} has SHA-256 {actual_sha256}, expected {DENO_CORE_USAGE_SHA256}",
        state.specifier,
      ));
    }
    let phase = match unsafe { heap_value(context) } {
      Some(HeapValue::Context(state)) => state.deno_core_bootstrap_phase,
      _ => return Err("usage script has no live v8x context".to_string()),
    };
    if phase != DENO_CORE_PRELINKED_SCRIPTS.len() {
      return Err(format!(
        "closed-world Deno replay usage ran at bootstrap phase {}, expected {}",
        phase,
        DENO_CORE_PRELINKED_SCRIPTS.len(),
      ));
    }
    // The verified AOT application embeds these same bytes and its
    // __v8x_run_classic_script export evaluates them through the verified
    // runtime-eval provider. Returning false deliberately continues into that
    // ordinary Script::Run path instead of the legacy staged substitute.
    return Ok(false);
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  {
    let actual_hash = fnv1a64(state.source.as_bytes());
    if actual_hash != DENO_CORE_USAGE_HASH {
      return Err(format!(
        "prelinked script {:?} has FNV-1a hash {actual_hash:#018x}, expected {DENO_CORE_USAGE_HASH:#018x}",
        state.specifier,
      ));
    }
    let phase = match unsafe { heap_value(context) } {
      Some(HeapValue::Context(state)) => state.deno_core_bootstrap_phase,
      _ => return Err("usage script has no live v8x context".to_string()),
    };
    if phase != DENO_CORE_PRELINKED_SCRIPTS.len() {
      return Err(format!(
        "prelinked Deno usage ran at bootstrap phase {}, expected {}",
        phase,
        DENO_CORE_PRELINKED_SCRIPTS.len(),
      ));
    }
    with_deno_core_runtime(context, "usage evaluation", |runtime| {
      runtime.advance_deno_core_usage()
    })?;
    Ok(true)
  }
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
  match run_prelinked_deno_usage_script(context, state) {
    Ok(true) => return v8__Undefined(current_isolate()).cast(),
    Ok(false) => {}
    Err(error) => {
      eprintln!("v8x/js2wasm: {error}");
      return ptr::null();
    }
  }
  let test_fn = match resolve_prelinked_test_fn(context) {
    Ok(function) => function,
    Err(error) => {
      eprintln!("v8x/js2wasm: {error}");
      return ptr::null();
    }
  };
  match with_deno_core_runtime(
    context,
    "classic script evaluation",
    |runtime| {
      runtime.bind_test_fn(test_fn)?;
      runtime.run_classic_script(&state.source)
    },
  ) {
    Ok(crate::js2wasm_spike::DenoScriptResult::Undefined) => {
      v8__Undefined(current_isolate()).cast()
    }
    Ok(crate::js2wasm_spike::DenoScriptResult::Json(value)) => {
      allocate_script_json_value(current_isolate(), &state.source, value)
    }
    Ok(crate::js2wasm_spike::DenoScriptResult::Thrown { name, message }) => {
      eprintln!("v8x/js2wasm: classic script threw {name}: {message}");
      let name = prelinked_error_name(&name);
      let message = new_string(current_isolate(), message);
      let exception = allocate_error(message, name);
      record_exception(current_isolate(), exception);
      ptr::null()
    }
    Err(error) => {
      eprintln!("v8x/js2wasm: {error}");
      let message = new_string(current_isolate(), error);
      let exception = allocate_error(message, "Error");
      record_exception(current_isolate(), exception);
      ptr::null()
    }
  }
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
  if isolate.is_null() || source.is_null() {
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
  let imports = parse_static_imports(source);
  let source_mapping_url = allocate::<Value>(isolate, HeapValue::Undefined);
  let unbound_script = allocate::<UnboundModuleScript>(
    isolate,
    HeapValue::UnboundModuleScript(UnboundModuleScriptState {
      source_mapping_url,
    }),
  );
  // This first metadata slice intentionally handles the exact zero-import
  // Deno bootstrap module. A non-empty request list needs real ModuleRequest
  // objects rather than a silently empty array, so leave that later boundary
  // fail-loud by returning a null requests handle.
  let module_requests = if imports.is_empty() {
    allocate::<FixedArray>(isolate, HeapValue::FixedArray(Vec::new()))
  } else {
    ptr::null()
  };
  let namespace = new_object(isolate);
  let deno_core_module = specifier == DENO_CORE_MODULE_SPECIFIER;
  let prelinked_deno_module =
    deno_core_module && is_prelinked_deno_module_source(source);
  #[cfg(feature = "js2wasm_deno_poc_replay")]
  if deno_core_module && !prelinked_deno_module {
    eprintln!(
      "v8x/js2wasm: closed-world Deno replay source {specifier:?} has SHA-256 {}, expected {DENO_CORE_MODULE_SHA256}",
      sha256_hex(source.as_bytes()),
    );
    return ptr::null();
  }
  allocate(
    isolate,
    HeapValue::Module(ModuleState {
      status: STATUS_UNINSTANTIATED,
      source: source.to_string(),
      specifier,
      imports,
      dependencies: Vec::new(),
      runtime: None,
      synthetic: None,
      evaluation_result: ptr::null(),
      exception: ptr::null(),
      unbound_script,
      module_requests,
      namespace,
      prelinked_deno_module,
    }),
  )
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__CreateSyntheticModule(
  isolate: *const RealIsolate,
  module_name: *const V8String,
  export_names_len: usize,
  export_names_raw: *const *const V8String,
  evaluation_steps: SyntheticModuleEvaluationSteps,
) -> *const Module {
  let isolate = isolate.cast_mut();
  if isolate.is_null()
    || module_name.is_null()
    || (export_names_len != 0 && export_names_raw.is_null())
  {
    return ptr::null();
  }
  let Some(specifier) = (unsafe { string_value(module_name) }) else {
    return ptr::null();
  };

  let namespace = new_object(isolate);
  let undefined = allocate::<Value>(isolate, HeapValue::Undefined);
  let mut export_names = Vec::with_capacity(export_names_len);
  let mut unique_names = HashSet::with_capacity(export_names_len);

  for index in 0..export_names_len {
    let export_name = unsafe { *export_names_raw.add(index) };
    let Some(name) = (unsafe { string_value(export_name) }) else {
      return ptr::null();
    };
    if !unique_names.insert(name.to_owned()) {
      return ptr::null();
    }
    export_names.push(name.to_owned());
    let Some(properties) = properties_mut(namespace) else {
      return ptr::null();
    };
    properties.push(TemplateProperty {
      key: export_name.cast(),
      value: undefined.cast(),
      attributes: 0,
    });
  }

  let evaluation_steps: SyntheticModuleEvaluationSteps<'static> =
    unsafe { std::mem::transmute(evaluation_steps) };
  allocate(
    isolate,
    HeapValue::Module(ModuleState {
      status: STATUS_UNINSTANTIATED,
      source: String::new(),
      specifier: specifier.to_owned(),
      imports: Vec::new(),
      dependencies: Vec::new(),
      runtime: None,
      synthetic: Some(SyntheticModuleState {
        export_names,
        evaluation_steps,
        namespace,
      }),
      evaluation_result: ptr::null(),
      exception: ptr::null(),
      unbound_script: ptr::null(),
      module_requests: ptr::null(),
      namespace,
      prelinked_deno_module: false,
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
pub extern "C" fn v8__Module__GetUnboundModuleScript(
  module: *const Module,
) -> *const UnboundModuleScript {
  unsafe { module_state(module) }
    .map(|state| state.unbound_script)
    .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__UnboundModuleScript__GetSourceMappingURL(
  script: *const UnboundModuleScript,
) -> *const Value {
  match unsafe { heap_value(script) } {
    Some(HeapValue::UnboundModuleScript(state)) => state.source_mapping_url,
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__GetModuleRequests(
  module: *const Module,
) -> *const FixedArray {
  unsafe { module_state(module) }
    .map(|state| state.module_requests)
    .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FixedArray__Length(array: *const FixedArray) -> int {
  match unsafe { heap_value(array) } {
    Some(HeapValue::FixedArray(elements)) => {
      int::try_from(elements.len()).unwrap_or(int::MAX)
    }
    _ => 0,
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__FixedArray__Get(
  array: *const FixedArray,
  index: int,
) -> *const Data {
  let Ok(index) = usize::try_from(index) else {
    return ptr::null();
  };
  match unsafe { heap_value(array) } {
    Some(HeapValue::FixedArray(elements)) => {
      elements.get(index).copied().unwrap_or(ptr::null())
    }
    _ => ptr::null(),
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__GetIdentityHash(module: *const Module) -> int {
  if module.is_null() || unsafe { module_state(module) }.is_none() {
    return 1;
  }

  // Module allocations never move in the Rust-owned heap, so folding their
  // address provides a stable identity hash. V8 only promises a non-zero hash,
  // not uniqueness; Data::EQ disambiguates the rare collision.
  let address = module as usize as u64;
  let folded = address ^ (address >> 32);
  let hash = (folded as u32 & i32::MAX as u32) as int;
  if hash == 0 { 1 } else { hash }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__GetException(
  module: *const Module,
) -> *const Value {
  unsafe { module_state(module) }
    .map(|state| state.exception)
    .unwrap_or(ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__GetModuleNamespace(
  module: *const Module,
) -> *const Value {
  let Some(state) = (unsafe { module_state(module) }) else {
    return ptr::null();
  };
  if state.status < STATUS_INSTANTIATED {
    return ptr::null();
  }
  state.namespace.cast()
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__SetSyntheticModuleExport(
  module: *const Module,
  isolate: *const RealIsolate,
  export_name: *const V8String,
  export_value: *const Value,
) -> MaybeBool {
  if isolate.is_null() || export_name.is_null() || export_value.is_null() {
    return MaybeBool::Nothing;
  }
  let Some(name) = (unsafe { string_value(export_name) }) else {
    return MaybeBool::Nothing;
  };
  let Some((namespace, declared)) =
    (unsafe { module_state(module) }).map(|state| {
      state
        .synthetic
        .as_ref()
        .map(|synthetic| {
          (
            synthetic.namespace,
            synthetic
              .export_names
              .iter()
              .any(|candidate| candidate == name),
          )
        })
        .unwrap_or((ptr::null(), false))
    })
  else {
    return MaybeBool::Nothing;
  };
  if namespace.is_null() || !declared {
    return MaybeBool::Nothing;
  }
  let Some(properties) = properties_mut(namespace) else {
    return MaybeBool::Nothing;
  };
  let Some(property) = properties.iter_mut().find(|property| {
    same_property_key(property.key.cast(), export_name.cast())
  }) else {
    return MaybeBool::Nothing;
  };
  property.value = export_value.cast();
  MaybeBool::JustTrue
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
    let attributes =
      allocate::<FixedArray>(isolate, HeapValue::FixedArray(Vec::new()));
    let Some(context_local) = (unsafe { crate::Local::from_raw(context) })
    else {
      return MaybeBool::Nothing;
    };
    let specifier_local = unsafe { crate::Local::from_raw(specifier) }.unwrap();
    let attributes_local =
      unsafe { crate::Local::from_raw(attributes) }.unwrap();
    let module_local = unsafe { crate::Local::from_raw(module) }.unwrap();
    #[cfg(not(target_os = "windows"))]
    let dependency = {
      let returned = unsafe {
        callback(
          context_local,
          specifier_local,
          attributes_local,
          module_local,
        )
      };
      unsafe {
        *(&returned as *const ResolveModuleCallbackRet as *const *const Module)
      }
    };
    #[cfg(target_os = "windows")]
    let dependency = {
      let mut dependency = ptr::null();
      unsafe {
        callback(
          &mut dependency,
          context_local,
          specifier_local,
          attributes_local,
          module_local,
        );
      }
      dependency
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
      if state.synthetic.is_some() {
        return None;
      }
      Some((
        state.specifier.clone(),
        state.source.clone(),
        state.dependencies.clone(),
      ))
    })
    .flatten()
    .ok_or_else(|| {
      "v8x/js2wasm: source graph contains a synthetic module".to_string()
    })?;
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

fn fail_module_evaluation(module: *const Module, message: &str) {
  let message = new_string(current_isolate(), message.to_owned());
  let exception = allocate_error(message, "Error");
  if let Some(state) = unsafe { module_state(module) } {
    state.status = STATUS_ERRORED;
    state.exception = exception;
  }
}

fn evaluate_synthetic_module(
  module: *const Module,
  context: *const Context,
  evaluation_steps: SyntheticModuleEvaluationSteps<'static>,
) -> *const Value {
  let Some(context_local) = (unsafe { crate::Local::from_raw(context) }) else {
    fail_module_evaluation(module, "synthetic module has no current context");
    return ptr::null();
  };
  let Some(module_local) = (unsafe { crate::Local::from_raw(module) }) else {
    fail_module_evaluation(module, "synthetic module handle is invalid");
    return ptr::null();
  };

  #[cfg(not(target_os = "windows"))]
  let result = {
    let returned = unsafe { evaluation_steps(context_local, module_local) };
    unsafe {
      *(&returned as *const SyntheticModuleEvaluationStepsRet
        as *const *const Value)
    }
  };
  #[cfg(target_os = "windows")]
  let result = {
    let mut result = ptr::null();
    unsafe {
      evaluation_steps(&mut result, context_local, module_local);
    }
    result
  };

  if result.is_null() {
    fail_module_evaluation(
      module,
      "synthetic module evaluation callback returned an empty value",
    );
    return ptr::null();
  }
  if let Some(state) = unsafe { module_state(module) } {
    state.status = STATUS_EVALUATED;
    state.evaluation_result = result;
  }
  result
}

fn evaluate_prelinked_deno_module(
  module: *const Module,
  context: *const Context,
) -> *const Value {
  #[cfg(feature = "js2wasm_deno_poc_replay")]
  {
    let phase = match unsafe { heap_value(context) } {
      Some(HeapValue::Context(state)) => state.deno_core_bootstrap_phase,
      _ => {
        fail_module_evaluation(
          module,
          "closed-world Deno replay module has no live v8x context",
        );
        return ptr::null();
      }
    };
    if phase != DENO_CORE_PRELINKED_SCRIPTS.len() {
      let error = format!(
        "closed-world Deno replay module evaluated at bootstrap phase {}, expected {}",
        phase,
        DENO_CORE_PRELINKED_SCRIPTS.len(),
      );
      fail_module_evaluation(module, &error);
      return ptr::null();
    }
  }

  #[cfg(not(feature = "js2wasm_deno_poc_replay"))]
  {
    let result =
      with_deno_core_runtime(context, "module evaluation", |runtime| {
        runtime.advance_deno_core_module()
      });
    if let Err(error) = result {
      eprintln!("v8x/js2wasm: {error}");
      fail_module_evaluation(module, &error);
      return ptr::null();
    }
  }

  let undefined = allocate::<Value>(current_isolate(), HeapValue::Undefined);
  let promise = allocate_fulfilled_promise(current_isolate(), undefined).cast();
  if let Some(state) = unsafe { module_state(module) } {
    state.status = STATUS_EVALUATED;
    state.evaluation_result = promise;
  }
  promise
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__Evaluate(
  module: *const Module,
  context: *const Context,
) -> *const Value {
  let Some(state) = (unsafe { module_state(module) }) else {
    return ptr::null();
  };
  if state.status == STATUS_EVALUATED {
    if !state.evaluation_result.is_null() {
      return state.evaluation_result;
    }
    let undefined = allocate::<Value>(current_isolate(), HeapValue::Undefined);
    let promise =
      allocate_fulfilled_promise(current_isolate(), undefined).cast();
    state.evaluation_result = promise;
    return promise;
  }
  if state.status != STATUS_INSTANTIATED {
    state.status = STATUS_ERRORED;
    return ptr::null();
  }
  state.status = STATUS_EVALUATING;
  let synthetic_evaluation_steps = state
    .synthetic
    .as_ref()
    .map(|synthetic| synthetic.evaluation_steps);
  let prelinked_deno_module = state.prelinked_deno_module;
  let entry = state.specifier.clone();
  if let Some(evaluation_steps) = synthetic_evaluation_steps {
    return evaluate_synthetic_module(module, context, evaluation_steps);
  }
  if prelinked_deno_module {
    return evaluate_prelinked_deno_module(module, context);
  }

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
  let undefined = allocate::<Value>(current_isolate(), HeapValue::Undefined);
  let promise = allocate_fulfilled_promise(current_isolate(), undefined).cast();
  if let Some(state) = unsafe { module_state(module) } {
    state.evaluation_result = promise;
  }
  mark_evaluated(module, &mut HashSet::new());
  promise
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__IsSourceTextModule(
  module: *const Module,
) -> bool {
  unsafe { module_state(module) }.is_some_and(|state| state.synthetic.is_none())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__IsSyntheticModule(module: *const Module) -> bool {
  unsafe { module_state(module) }.is_some_and(|state| state.synthetic.is_some())
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__IsGraphAsync(_module: *const Module) -> bool {
  false
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__Module__ScriptId(_module: *const Module) -> int {
  1
}
