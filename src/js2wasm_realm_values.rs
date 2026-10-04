use super::*;

/// Opt-in diagnostic accounting. Inclusive calls may nest through host callbacks.
pub(super) struct RealmCallProfile {
  entries: Option<HashMap<String, (u64, std::time::Duration)>>,
}
impl RealmCallProfile {
  pub(super) fn new() -> Self {
    Self {
      entries: std::env::var_os("V8X_JS2WASM_PROFILE_REALM")
        .map(|_| HashMap::new()),
    }
  }
  fn start(&self) -> Option<std::time::Instant> {
    self.entries.as_ref().map(|_| std::time::Instant::now())
  }
  fn finish(&mut self, name: &str, start: Option<std::time::Instant>) {
    if let (Some(entries), Some(start)) = (&mut self.entries, start) {
      let entry = entries.entry(name.to_owned()).or_default();
      entry.0 += 1;
      entry.1 += start.elapsed();
    }
  }
}
impl Drop for RealmCallProfile {
  fn drop(&mut self) {
    if let Some(entries) = &self.entries {
      let mut rows: Vec<_> = entries.iter().collect();
      rows.sort_by_key(|(_, (_, elapsed))| std::cmp::Reverse(*elapsed));
      for (name, (count, elapsed)) in rows {
        eprintln!(
          "V8X_REALM {name} {count} calls {:.3} ms (inclusive)",
          elapsed.as_secs_f64() * 1000.0
        );
      }
    }
  }
}

/// A handle is valid only in the store that allocated its realm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RealmValue {
  owner: usize,
  handle: f64,
}

impl RealmValue {
  pub(super) fn checked_handle(self, realm: usize) -> Result<f64, String> {
    if self.owner != realm {
      return Err("value belongs to a different Wasmtime realm".to_string());
    }
    Ok(self.handle)
  }
}

impl RealmAccess for DenoRuntime {
  fn realm_native_promise_settle(
    &mut self,
    packet: RealmValue,
    promise: RealmValue,
    value: RealmValue,
    rejected: bool,
  ) -> Result<(), String> {
    let handles = [
      self.realm_check(packet)?,
      self.realm_check(promise)?,
      self.realm_check(value)?,
    ];
    settle_native_promise_in_store(
      &mut self.store,
      self.realm_instance,
      handles,
      rejected,
    )
  }
  fn realm_run_aot_script(
    &mut self,
    specifier: &str,
    source: &str,
    preserve_completion: bool,
  ) -> Result<Option<(bool, RealmValue)>, String> {
    self.run_aot_script(specifier, source, preserve_completion)
  }
  fn realm_try_graph_reflection(
    &mut self,
    handles: &[f64],
    operation: &str,
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::reflection(
      &mut self.store,
      self.realm_instance,
      handles,
      operation,
    )
  }
  fn realm_native_error_snapshot(
    &mut self,
    handle: f64,
  ) -> Result<Option<(f64, f64)>, String> {
    graph_calls::native_error_snapshot(
      &mut self.store,
      self.realm_instance,
      handle,
    )
  }
  fn realm_promise_handler(
    &mut self,
    handle: f64,
    mark: bool,
  ) -> Result<Option<bool>, String> {
    graph_calls::promise_handler(
      &mut self.store,
      self.realm_instance,
      handle,
      mark,
    )
  }
  fn realm_try_graph_promise_then(
    &mut self,
    handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::promise_then(&mut self.store, self.realm_instance, handles)
  }
  fn realm_capture_stack(
    &mut self,
  ) -> Result<Vec<(u32, Option<String>)>, String> {
    Ok(
      wasmtime::WasmBacktrace::force_capture(&self.store)
        .frames()
        .iter()
        .map(|frame| (frame.func_index(), frame.func_name().map(str::to_owned)))
        .collect(),
    )
  }
  fn realm_promise_snapshot(
    &mut self,
    handle: f64,
  ) -> Result<Option<(i32, f64)>, String> {
    graph_calls::promise_snapshot(&mut self.store, self.realm_instance, handle)
  }
  fn realm_try_graph_get_prototype(
    &mut self,
    handles: [f64; 1],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::get_prototype(&mut self.store, self.realm_instance, handles)
  }
  fn realm_try_graph_set_prototype(
    &mut self,
    handles: [f64; 2],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::set_prototype(&mut self.store, self.realm_instance, handles)
  }
  fn realm_try_graph_set(
    &mut self,
    handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::set(&mut self.store, self.realm_instance, handles)
  }
  fn realm_try_graph_get(
    &mut self,
    handles: [f64; 2],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::get(&mut self.store, self.realm_instance, handles)
  }
  fn realm_try_graph_call(
    &mut self,
    handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::call(&mut self.store, self.realm_instance, handles)
  }
  fn realm_bulk_utf16(
    &mut self,
    handle: f64,
  ) -> Result<Option<Vec<u16>>, String> {
    use wasmtime::AsContextMut;
    shared_strings::read(
      self.store.as_context_mut(),
      self.realm_instance,
      handle,
    )
  }
  fn realm_string_cache(&mut self) -> &mut HashMap<Vec<u16>, f64> {
    &mut self.store.data_mut().string_handles
  }
  fn realm_packet(&mut self, bytes: &[u8]) -> Result<f64, String> {
    use wasmtime::AsContextMut;
    shared_buffers::packet(
      self.store.as_context_mut(),
      self.realm_instance,
      bytes,
    )
  }
  fn realm_has_export(&mut self, name: &str) -> bool {
    self
      .realm_instance
      .get_func(&mut self.store, name)
      .is_some()
  }
  fn realm_adopt_buffer(
    &mut self,
    host: crate::js2wasm::RetainedHostBuffer,
  ) -> Result<RealmValue, String> {
    use wasmtime::AsContextMut;
    let handle = shared_buffers::adopt(
      self.store.as_context_mut(),
      self.realm_instance,
      host,
    )?;
    self.realm_from_handle(handle)
  }
  fn realm_id(&self) -> usize {
    self.realm_id
  }
  fn realm_raw(
    &mut self,
    name: &str,
    args: &[f64],
    returns: bool,
  ) -> Result<f64, String> {
    let start = self.store.data().realm_profile.start();
    let function = self
      .realm_instance
      .get_func(&mut self.store, name)
      .ok_or_else(|| {
        format!("core artifact lacks realm bridge export {name}")
      })?;
    let args: Vec<_> = args
      .iter()
      .map(|v| wasmtime::Val::F64(v.to_bits()))
      .collect();
    let mut result = if returns {
      vec![wasmtime::Val::F64(0)]
    } else {
      vec![]
    };
    let outcome = function.call(&mut self.store, &args, &mut result);
    self.store.data_mut().realm_profile.finish(name, start);
    outcome.map_err(|error| {
      let payload =
        render_pending_wasm_exception(&mut self.store, self.realm_instance);
      format!("call realm bridge {name}: {error:#}; {payload}")
    })?;
    if !returns {
      return Ok(0.0);
    }
    match result[0] {
      wasmtime::Val::F64(bits) => Ok(f64::from_bits(bits)),
      _ => Err(format!("realm bridge {name} returned a non-number")),
    }
  }
}

pub(crate) trait RealmAccess {
  fn realm_instantiate_callback_graph(
    &mut self,
    _module: &wasmtime::Module,
    _bindings: &NativeModuleGraph,
  ) -> Result<(bool, RealmValue), String> {
    Err("nested module instantiation requires active Caller access".into())
  }
  fn realm_native_promise_settle(
    &mut self,
    packet: RealmValue,
    promise: RealmValue,
    value: RealmValue,
    rejected: bool,
  ) -> Result<(), String>;
  fn realm_run_aot_script(
    &mut self,
    _specifier: &str,
    _source: &str,
    _preserve_completion: bool,
  ) -> Result<Option<(bool, RealmValue)>, String> {
    Err("this realm does not support AOT Script instantiation".into())
  }
  fn realm_try_graph_reflection(
    &mut self,
    _handles: &[f64],
    _operation: &str,
  ) -> Result<Option<(bool, f64)>, String> {
    Ok(None)
  }
  fn realm_native_error_snapshot(
    &mut self,
    _handle: f64,
  ) -> Result<Option<(f64, f64)>, String> {
    Ok(None)
  }
  fn realm_promise_handler(
    &mut self,
    _handle: f64,
    _mark: bool,
  ) -> Result<Option<bool>, String> {
    Err("this realm does not support Promise handler state".to_string())
  }
  fn realm_try_graph_promise_then(
    &mut self,
    _handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    Ok(None)
  }
  fn realm_capture_stack(
    &mut self,
  ) -> Result<Vec<(u32, Option<String>)>, String> {
    Err("this realm does not support native stack capture".to_string())
  }
  fn realm_promise_snapshot(
    &mut self,
    _handle: f64,
  ) -> Result<Option<(i32, f64)>, String> {
    Ok(None)
  }
  fn realm_try_graph_get_prototype(
    &mut self,
    _handles: [f64; 1],
  ) -> Result<Option<(bool, f64)>, String> {
    Ok(None)
  }
  fn realm_try_graph_set_prototype(
    &mut self,
    _handles: [f64; 2],
  ) -> Result<Option<(bool, f64)>, String> {
    Ok(None)
  }
  fn realm_try_graph_set(
    &mut self,
    _handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    Ok(None)
  }
  fn realm_try_graph_get(
    &mut self,
    _handles: [f64; 2],
  ) -> Result<Option<(bool, f64)>, String> {
    Ok(None)
  }
  fn realm_try_graph_call(
    &mut self,
    _handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    Ok(None)
  }
  fn realm_bulk_utf16(
    &mut self,
    handle: f64,
  ) -> Result<Option<Vec<u16>>, String>;
  fn realm_string_cache(&mut self) -> &mut HashMap<Vec<u16>, f64>;
  fn realm_packet(&mut self, bytes: &[u8]) -> Result<f64, String>;
  fn realm_has_export(&mut self, name: &str) -> bool;
  fn realm_adopt_buffer(
    &mut self,
    host: crate::js2wasm::RetainedHostBuffer,
  ) -> Result<RealmValue, String>;
  fn realm_id(&self) -> usize;
  fn realm_raw(
    &mut self,
    name: &str,
    args: &[f64],
    returns: bool,
  ) -> Result<f64, String>;
  fn realm_handle(
    &mut self,
    name: &str,
    args: &[f64],
  ) -> Result<RealmValue, String> {
    let handle = self.realm_raw(name, args, true)?;
    self.realm_from_handle(handle)
  }

  fn realm_from_handle(&self, handle: f64) -> Result<RealmValue, String> {
    if !handle.is_finite()
      || handle < 0.0
      || handle.fract() != 0.0
      || handle > 9_007_199_254_740_991.0
    {
      return Err("realm bridge returned an invalid handle".to_string());
    }
    Ok(RealmValue {
      owner: self.realm_id(),
      handle,
    })
  }

  fn realm_check(&self, value: RealmValue) -> Result<f64, String> {
    value.checked_handle(self.realm_id())
  }

  fn realm_undefined(&self) -> RealmValue {
    RealmValue {
      owner: self.realm_id(),
      handle: 0.0,
    }
  }

  fn realm_kind(&mut self, value: RealmValue) -> Result<u8, String> {
    let handle = self.realm_check(value)?;
    let kind = self.realm_raw("__v8x_value_kind", &[handle], true)?;
    if !kind.is_finite() || kind < 0.0 || kind > 9.0 || kind.fract() != 0.0 {
      return Err("invalid realm value kind".to_string());
    }
    Ok(kind as u8)
  }

  fn realm_null(&mut self) -> Result<RealmValue, String> {
    self.realm_handle("__v8x_value_null", &[])
  }

  fn realm_boolean(&mut self, value: bool) -> Result<RealmValue, String> {
    self.realm_handle("__v8x_value_boolean", &[if value { 1.0 } else { 0.0 }])
  }

  fn realm_as_boolean(&mut self, value: RealmValue) -> Result<bool, String> {
    let handle = self.realm_check(value)?;
    match self.realm_raw("__v8x_value_as_boolean", &[handle], true)? {
      0.0 => Ok(false),
      1.0 => Ok(true),
      _ => Err("invalid realm boolean encoding".to_string()),
    }
  }

  fn realm_array(&mut self) -> Result<RealmValue, String> {
    self.realm_handle("__v8x_value_array", &[])
  }

  fn realm_global(&mut self) -> Result<RealmValue, String> {
    self.realm_handle("__v8x_value_global", &[])
  }

  fn realm_number(&mut self, value: f64) -> Result<RealmValue, String> {
    self.realm_handle("__v8x_value_number", &[value])
  }

  fn realm_as_number(&mut self, value: RealmValue) -> Result<f64, String> {
    let handle = self.realm_check(value)?;
    self.realm_raw("__v8x_value_as_number", &[handle], true)
  }

  fn realm_string(&mut self, units: &[u16]) -> Result<RealmValue, String> {
    if units.len() <= 64 {
      if let Some(handle) = self.realm_string_cache().get(units).copied() {
        return self.realm_from_handle(handle);
      }
    }
    let value = if self.realm_has_export("__v8x_value_string_from_buffer") {
      let bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
      let packet = self.realm_packet(&bytes)?;
      self.realm_handle("__v8x_value_string_from_buffer", &[packet])?
    } else {
      let mut value = self.realm_handle("__v8x_value_string_empty", &[])?;
      for unit in units {
        value = self.realm_handle(
          "__v8x_value_string_append",
          &[value.handle, f64::from(*unit)],
        )?;
      }
      value
    };
    // Property names dominate repeat transfers. Do not retain unbounded text.
    if units.len() <= 64 {
      let cache = self.realm_string_cache();
      if cache.len() >= 512 {
        cache.clear();
      }
      cache.insert(units.to_vec(), value.handle);
    }
    Ok(value)
  }

  fn realm_as_utf16(&mut self, value: RealmValue) -> Result<Vec<u16>, String> {
    let handle = self.realm_check(value)?;
    if let Some(units) = self.realm_bulk_utf16(handle)? {
      return Ok(units);
    }
    let length = self.realm_raw("__v8x_value_utf16_length", &[handle], true)?;
    if !length.is_finite()
      || length < 0.0
      || length.fract() != 0.0
      || length > u32::MAX as f64
    {
      return Err("realm bridge returned an invalid UTF-16 length".to_string());
    }
    let mut units = Vec::new();
    units
      .try_reserve(length as usize)
      .map_err(|error| format!("allocate realm string: {error}"))?;
    for index in 0..length as usize {
      let unit = self.realm_raw(
        "__v8x_value_utf16_unit",
        &[handle, index as f64],
        true,
      )?;
      if !unit.is_finite()
        || unit < 0.0
        || unit > 65535.0
        || unit.fract() != 0.0
      {
        return Err("realm bridge returned an invalid UTF-16 unit".to_string());
      }
      units.push(unit as u16);
    }
    Ok(units)
  }

  fn realm_get_prototype(
    &mut self,
    object: RealmValue,
  ) -> Result<RealmValue, String> {
    let object = self.realm_check(object)?;
    self.realm_handle("__v8x_value_get_prototype", &[object])
  }

  fn realm_set_prototype(
    &mut self,
    object: RealmValue,
    prototype: RealmValue,
  ) -> Result<bool, String> {
    let object = self.realm_check(object)?;
    let prototype = self.realm_check(prototype)?;
    match self.realm_raw(
      "__v8x_value_set_prototype",
      &[object, prototype],
      true,
    )? {
      0.0 => Ok(false),
      1.0 => Ok(true),
      _ => Err("invalid prototype update result".into()),
    }
  }

  fn realm_get(
    &mut self,
    object: RealmValue,
    key: RealmValue,
  ) -> Result<RealmValue, String> {
    let object = self.realm_check(object)?;
    let key = self.realm_check(key)?;
    self.realm_handle("__v8x_value_get", &[object, key])
  }

  fn realm_set(
    &mut self,
    object: RealmValue,
    key: RealmValue,
    value: RealmValue,
  ) -> Result<(), String> {
    let object = self.realm_check(object)?;
    let key = self.realm_check(key)?;
    let value = self.realm_check(value)?;
    self.realm_raw("__v8x_value_set", &[object, key, value], false)?;
    Ok(())
  }

  fn realm_object(&mut self) -> Result<RealmValue, String> {
    self.realm_handle("__v8x_value_object", &[])
  }

  fn realm_define_data(
    &mut self,
    object: RealmValue,
    key: RealmValue,
    value: RealmValue,
    attributes: u32,
  ) -> Result<(), String> {
    let object = self.realm_check(object)?;
    let key = self.realm_check(key)?;
    let value = self.realm_check(value)?;
    self.realm_raw(
      "__v8x_value_define_data",
      &[object, key, value, attributes as f64],
      false,
    )?;
    Ok(())
  }

  fn realm_define_getter(
    &mut self,
    object: RealmValue,
    key: RealmValue,
    getter: RealmValue,
    attributes: u32,
  ) -> Result<(), String> {
    let object = self.realm_check(object)?;
    let key = self.realm_check(key)?;
    let getter = self.realm_check(getter)?;
    self.realm_raw(
      "__v8x_value_define_getter",
      &[object, key, getter, attributes as f64],
      false,
    )?;
    Ok(())
  }

  fn realm_define_many(
    &mut self,
    object: RealmValue,
    entries: &[(RealmValue, RealmValue, u32)],
  ) -> Result<(), String> {
    let owner = self.realm_check(object)?;
    if entries.len() < 4 || !self.realm_has_export("__v8x_value_define_packet")
    {
      for &(key, value, flags) in entries {
        self.realm_define_data(object, key, value, flags)?;
      }
      return Ok(());
    }
    let mut bytes = Vec::new();
    for &(key, value, flags) in entries {
      for handle in [
        self.realm_check(key)?,
        self.realm_check(value)?,
        flags as f64,
      ] {
        if !handle.is_finite()
          || handle < 0.0
          || handle > u32::MAX as f64
          || handle.fract() != 0.0
        {
          return Err("property packet value exceeds u32 ABI".into());
        }
        bytes.extend_from_slice(&(handle as u32).to_le_bytes());
      }
    }
    let packet = self.realm_packet(&bytes)?;
    self.realm_raw("__v8x_value_define_packet", &[owner, packet], false)?;
    Ok(())
  }

  fn realm_call(
    &mut self,
    callable: RealmValue,
    receiver: RealmValue,
    args: RealmValue,
  ) -> Result<RealmValue, String> {
    let callable = self.realm_check(callable)?;
    let receiver = self.realm_check(receiver)?;
    let args = self.realm_check(args)?;
    self.realm_handle("__v8x_value_call", &[callable, receiver, args])
  }
}

#[cfg(feature = "js2wasm_runtime_compile")]
pub(crate) fn load_realm_for_test(
  path: &Path,
) -> Result<Rc<RefCell<DenoRuntime>>, String> {
  // Match production graph attachment: all modules use the same Engine.
  let shared = shared_runtime()?;
  let bytes = fs::read(path).map_err(|e| e.to_string())?;
  let module = Module::new(&shared.engine, bytes).map_err(|e| e.to_string())?;
  if let Some(output) = std::env::var_os("V8X_JS2WASM_CONTEXT_AOT_OUTPUT") {
    let bytes = module.serialize().map_err(|error| error.to_string())?;
    atomic_write(Path::new(&output), &bytes)?;
  }
  let prepared = shared.prepare_module(&module)?;
  DenoRuntime::instantiate(&shared, &prepared, PathBuf::from("."), 0)
    .map(|runtime| Rc::new(RefCell::new(runtime)))
}

/// Only trusted, immutable artifacts generated by the test build pipeline.
/// Uses the same deserialization/configuration contract as deployment inputs.
#[cfg(not(feature = "js2wasm_deno_poc_replay"))]
pub(crate) fn load_precompiled_realm_for_test(
  path: &Path,
) -> Result<Rc<RefCell<DenoRuntime>>, String> {
  let shared = shared_runtime()?;
  let prepared = shared.precompiled_file(path)?;
  DenoRuntime::instantiate(shared, &prepared, PathBuf::from("."), 0)
    .map(|runtime| Rc::new(RefCell::new(runtime)))
}

#[cfg(feature = "js2wasm_runtime_compile")]
pub(crate) fn load_graph_for_test(
  runtime: &mut DenoRuntime,
  path: &Path,
) -> Result<(), String> {
  let shared = shared_runtime()?;
  let bytes = fs::read(path).map_err(|e| e.to_string())?;
  let module = Module::new(&shared.engine, bytes).map_err(|e| e.to_string())?;
  let prepared = shared.prepare_module(&module)?;
  runtime.instantiate_graph(shared, &prepared)
}

#[cfg(feature = "js2wasm_runtime_compile")]
pub fn js2wasm_test_realm_values(path: &Path) -> Result<(), String> {
  let shared = SharedDenoRuntime::new()?;
  let bytes = fs::read(path).map_err(|e| e.to_string())?;
  let module = Module::new(&shared.engine, bytes).map_err(|e| e.to_string())?;
  let prepared = shared.prepare_module(&module)?;
  let mut runtime =
    DenoRuntime::instantiate(&shared, &prepared, PathBuf::from("."), 0)?;
  let global = runtime.realm_global()?;
  let key =
    runtime.realm_string(&"greeting".encode_utf16().collect::<Vec<_>>())?;
  let units: Vec<_> = "Grüße 😀".encode_utf16().collect();
  let text = runtime.realm_string(&units)?;
  runtime.realm_set(global, key, text)?;
  let read = runtime.realm_get(global, key)?;
  assert_eq!(text, read);
  assert_eq!(runtime.realm_as_utf16(read)?, units);
  let obj = runtime.realm_handle("__v8x_value_object", &[])?;
  runtime.realm_set(global, key, obj)?;
  assert_eq!(runtime.realm_get(global, key)?, obj);
  let args = runtime.realm_handle("__v8x_value_array", &[])?;
  let zero = runtime.realm_string(&[48])?;
  runtime.realm_set(args, zero, obj)?;
  let name =
    runtime.realm_string(&"identity".encode_utf16().collect::<Vec<_>>())?;
  let callable = runtime.realm_get(global, name)?;
  assert_eq!(runtime.realm_call(callable, global, args)?, obj);
  if runtime.realm_has_export("__v8x_value_define_getter") {
    let getter_name = runtime
      .realm_string(&"liveExportGetter".encode_utf16().collect::<Vec<_>>())?;
    let getter = runtime.realm_get(global, getter_name)?;
    let slot = runtime
      .realm_string(&"liveExportSlot".encode_utf16().collect::<Vec<_>>())?;
    let property = runtime
      .realm_string(&"live-export".encode_utf16().collect::<Vec<_>>())?;
    let receiver = runtime.realm_object()?;
    runtime.realm_set(global, slot, obj)?;
    runtime.realm_define_getter(receiver, property, getter, 4)?;
    assert_eq!(runtime.realm_get(receiver, property)?, obj);
    let replacement = runtime.realm_object()?;
    runtime.realm_set(global, slot, replacement)?;
    assert_eq!(runtime.realm_get(receiver, property)?, replacement);
    runtime.store.gc(None).map_err(|error| error.to_string())?;
    assert_eq!(runtime.realm_get(receiver, property)?, replacement);
    assert!(runtime.realm_set(receiver, property, obj).is_err());
    assert_eq!(runtime.realm_get(receiver, property)?, replacement);
    eprintln!(
      "PASS: native live getter dispatch, replacement identity and GC retention"
    );
  }
  let negative_zero = runtime.realm_number(-0.0)?;
  assert_eq!(
    runtime.realm_as_number(negative_zero)?.to_bits(),
    (-0.0f64).to_bits()
  );
  let nan = runtime.realm_number(f64::NAN)?;
  assert!(runtime.realm_as_number(nan)?.is_nan());
  let lone = runtime.realm_string(&[0xd800])?;
  assert_eq!(runtime.realm_as_utf16(lone)?, vec![0xd800]);
  if runtime.realm_has_export("__v8x_value_string_from_buffer") {
    for units in [vec![], vec![0, 0xd800, 0xdc00, 0xffff], vec![97; 1024]] {
      let value = runtime.realm_string(&units)?;
      assert_eq!(runtime.realm_as_utf16(value)?, units);
    }
    let mut definitions = Vec::new();
    for index in 0..8 {
      let key = runtime.realm_string(&[b'a' as u16 + index])?;
      let value = runtime.realm_number(index as f64)?;
      definitions.push((key, value, 0));
    }
    runtime.realm_define_many(obj, &definitions)?;
    for &(key, value, _) in &definitions {
      assert_eq!(runtime.realm_get(obj, key)?, value);
    }
    // Definition order remains significant when a key appears twice.
    definitions[7].0 = definitions[0].0;
    runtime.realm_define_many(obj, &definitions)?;
    assert_eq!(runtime.realm_get(obj, definitions[0].0)?, definitions[7].1);
    eprintln!("PASS: bulk UTF-16 round trips and ordered property packets");
  }
  // Cache eviction must preserve the canonical handle, including lone UTF-16.
  assert_eq!(runtime.realm_string(&[0xd800])?, lone);
  for index in 0..600 {
    runtime.realm_string(
      &format!("cache-key-{index}")
        .encode_utf16()
        .collect::<Vec<_>>(),
    )?;
  }
  assert!(runtime.store.data().string_handles.len() <= 512);
  assert_eq!(runtime.realm_string(&[0xd800])?, lone);
  let long_key = vec![42; 65];
  runtime.realm_string(&long_key)?;
  assert!(!runtime.store.data().string_handles.contains_key(&long_key));
  // Exercise root relocation, not merely allocation, under a moving collector.
  if runtime.realm_has_export("__v8x_value_string_storage") {
    let name = runtime
      .realm_string(&"stringStorageCases".encode_utf16().collect::<Vec<_>>())?;
    let make = runtime.realm_get(global, name)?;
    let seed_units = vec![0xd800, 65, 0, 0xffff, 0xdc00];
    let seed = runtime.realm_string(&seed_units)?;
    let inputs = runtime.realm_array()?;
    let zero = runtime.realm_string(&[48])?;
    runtime.realm_set(inputs, zero, seed)?;
    let cases = runtime.realm_call(make, global, inputs)?;
    let expected = [
      seed_units[1..4].to_vec(),
      seed_units.repeat(2),
      seed_units.repeat(129),
      vec![],
    ];
    for pass in 0..2 {
      runtime.store.gc(None).map_err(|error| error.to_string())?;
      for (index, units) in expected.iter().enumerate() {
        let key = runtime.realm_string(&[48 + index as u16])?;
        let value = runtime.realm_get(cases, key)?;
        assert_eq!(
          runtime.realm_as_utf16(value)?,
          *units,
          "bulk string case {index}, GC pass {pass}"
        );
      }
    }
    assert!(runtime.realm_as_utf16(obj).is_err());
    eprintln!(
      "PASS: bulk UTF-16 slices, ropes, deep concatenation and invalid type after GC"
    );
  }
  // The same check also runs with the historical DRC collector.
  runtime.store.gc(None).map_err(|error| error.to_string())?;
  assert_eq!(runtime.realm_as_utf16(text)?, units);
  assert_eq!(runtime.realm_get(global, key)?, obj);
  assert_eq!(runtime.realm_call(callable, global, args)?, obj);
  runtime.store.gc(None).map_err(|error| error.to_string())?;
  assert_eq!(
    runtime.realm_as_number(negative_zero)?.to_bits(),
    (-0.0f64).to_bits()
  );
  // Graph execution must not switch the table used by existing realm handles.
  let alternate = DenoRuntime::instantiate_in_store(
    &shared,
    &prepared,
    &mut runtime.store,
    &mut runtime._runtime_eval_provider,
    Some(runtime.realm_instance),
  )?;
  let primary = std::mem::replace(&mut runtime.instance, alternate);
  assert_eq!(runtime.realm_get(global, key)?, obj);
  assert_eq!(runtime.realm_call(callable, global, args)?, obj);
  runtime.instance = primary;
  let mut other =
    DenoRuntime::instantiate(&shared, &prepared, PathBuf::from("."), 0)?;
  assert!(
    other
      .realm_as_utf16(text)
      .unwrap_err()
      .contains("different Wasmtime realm")
  );
  Ok(())
}

pub(crate) struct CallerRealm<'a> {
  caller: Caller<'a, DenoHostState>,
  realm_id: usize,
  realm_instance: Instance,
}

fn settle_native_promise_in_store(
  store: &mut impl wasmtime::AsContextMut<Data = DenoHostState>,
  realm: Instance,
  handles: [f64; 3],
  rejected: bool,
) -> Result<(), String> {
  let settle = realm
    .get_typed_func::<(f64, f64, f64), ()>(
      &mut *store,
      "__v8x_value_native_promise_settle",
    )
    .map_err(|error| format!("native Promise settlement ABI: {error:#}"))?;
  store
    .as_context_mut()
    .data_mut()
    .suppressed_native_promise_rejections
    .push(handles[1]);
  let result = settle.call(
    &mut *store,
    (handles[0], handles[2], if rejected { 1.0 } else { 0.0 }),
  );
  store
    .as_context_mut()
    .data_mut()
    .suppressed_native_promise_rejections
    .pop();
  result.map_err(|error| format!("settle native Promise copy: {error:#}"))
}
impl<'a> CallerRealm<'a> {
  pub(super) fn into_caller(self) -> Caller<'a, DenoHostState> {
    self.caller
  }

  pub(super) fn new(caller: Caller<'a, DenoHostState>) -> Result<Self, String> {
    let realm_id = caller.data().realm_id;
    let realm_instance = caller.data().realm_instance.ok_or_else(|| {
      "host callback invoked before realm publication".to_string()
    })?;
    if realm_id == 0 {
      return Err("host callback has no realm identity".to_string());
    }
    Ok(Self {
      caller,
      realm_id,
      realm_instance,
    })
  }
}
impl RealmAccess for CallerRealm<'_> {
  fn realm_instantiate_callback_graph(
    &mut self,
    module: &wasmtime::Module,
    bindings: &NativeModuleGraph,
  ) -> Result<(bool, RealmValue), String> {
    let (normal, handle) = DenoRuntime::instantiate_callback_graph_in_context(
      &mut self.caller,
      self.realm_instance,
      module,
      bindings,
    )?;
    Ok((normal, self.realm_from_handle(handle)?))
  }
  fn realm_native_promise_settle(
    &mut self,
    packet: RealmValue,
    promise: RealmValue,
    value: RealmValue,
    rejected: bool,
  ) -> Result<(), String> {
    let handles = [
      self.realm_check(packet)?,
      self.realm_check(promise)?,
      self.realm_check(value)?,
    ];
    settle_native_promise_in_store(
      &mut self.caller,
      self.realm_instance,
      handles,
      rejected,
    )
  }
  fn realm_run_aot_script(
    &mut self,
    specifier: &str,
    source: &str,
    _preserve_completion: bool,
  ) -> Result<Option<(bool, RealmValue)>, String> {
    let result = DenoRuntime::run_aot_script_in_context(
      &mut self.caller,
      self.realm_instance,
      None,
      specifier,
      source,
      true,
    )?;
    result
      .map(|(normal, handle)| Ok((normal, self.realm_from_handle(handle)?)))
      .transpose()
  }
  fn realm_try_graph_reflection(
    &mut self,
    handles: &[f64],
    operation: &str,
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::reflection(
      &mut self.caller,
      self.realm_instance,
      handles,
      operation,
    )
  }
  fn realm_native_error_snapshot(
    &mut self,
    handle: f64,
  ) -> Result<Option<(f64, f64)>, String> {
    graph_calls::native_error_snapshot(
      &mut self.caller,
      self.realm_instance,
      handle,
    )
  }
  fn realm_promise_handler(
    &mut self,
    handle: f64,
    mark: bool,
  ) -> Result<Option<bool>, String> {
    graph_calls::promise_handler(
      &mut self.caller,
      self.realm_instance,
      handle,
      mark,
    )
  }
  fn realm_try_graph_promise_then(
    &mut self,
    handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::promise_then(&mut self.caller, self.realm_instance, handles)
  }
  fn realm_capture_stack(
    &mut self,
  ) -> Result<Vec<(u32, Option<String>)>, String> {
    Ok(
      wasmtime::WasmBacktrace::force_capture(&self.caller)
        .frames()
        .iter()
        .map(|frame| (frame.func_index(), frame.func_name().map(str::to_owned)))
        .collect(),
    )
  }
  fn realm_promise_snapshot(
    &mut self,
    handle: f64,
  ) -> Result<Option<(i32, f64)>, String> {
    graph_calls::promise_snapshot(&mut self.caller, self.realm_instance, handle)
  }
  fn realm_try_graph_get_prototype(
    &mut self,
    handles: [f64; 1],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::get_prototype(&mut self.caller, self.realm_instance, handles)
  }
  fn realm_try_graph_set_prototype(
    &mut self,
    handles: [f64; 2],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::set_prototype(&mut self.caller, self.realm_instance, handles)
  }
  fn realm_try_graph_set(
    &mut self,
    handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::set(&mut self.caller, self.realm_instance, handles)
  }
  fn realm_try_graph_get(
    &mut self,
    handles: [f64; 2],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::get(&mut self.caller, self.realm_instance, handles)
  }
  fn realm_try_graph_call(
    &mut self,
    handles: [f64; 3],
  ) -> Result<Option<(bool, f64)>, String> {
    graph_calls::call(&mut self.caller, self.realm_instance, handles)
  }
  fn realm_bulk_utf16(
    &mut self,
    handle: f64,
  ) -> Result<Option<Vec<u16>>, String> {
    use wasmtime::AsContextMut;
    shared_strings::read(
      self.caller.as_context_mut(),
      self.realm_instance,
      handle,
    )
  }
  fn realm_string_cache(&mut self) -> &mut HashMap<Vec<u16>, f64> {
    &mut self.caller.data_mut().string_handles
  }
  fn realm_packet(&mut self, bytes: &[u8]) -> Result<f64, String> {
    use wasmtime::AsContextMut;
    shared_buffers::packet(
      self.caller.as_context_mut(),
      self.realm_instance,
      bytes,
    )
  }
  fn realm_has_export(&mut self, name: &str) -> bool {
    self
      .realm_instance
      .get_func(&mut self.caller, name)
      .is_some()
  }
  fn realm_adopt_buffer(
    &mut self,
    host: crate::js2wasm::RetainedHostBuffer,
  ) -> Result<RealmValue, String> {
    use wasmtime::AsContextMut;
    let handle = shared_buffers::adopt(
      self.caller.as_context_mut(),
      self.realm_instance,
      host,
    )?;
    self.realm_from_handle(handle)
  }
  fn realm_id(&self) -> usize {
    self.realm_id
  }
  fn realm_raw(
    &mut self,
    name: &str,
    args: &[f64],
    returns: bool,
  ) -> Result<f64, String> {
    let start = self.caller.data().realm_profile.start();
    let function = self
      .realm_instance
      .get_func(&mut self.caller, name)
      .ok_or_else(|| {
        format!("core artifact lacks realm bridge export {name}")
      })?;
    let args: Vec<_> = args
      .iter()
      .map(|v| wasmtime::Val::F64(v.to_bits()))
      .collect();
    let mut result = if returns {
      vec![wasmtime::Val::F64(0)]
    } else {
      vec![]
    };
    let outcome = function.call(&mut self.caller, &args, &mut result);
    self.caller.data_mut().realm_profile.finish(name, start);
    outcome.map_err(|error| format!("call realm bridge {name}: {error:#}"))?;
    if !returns {
      return Ok(0.0);
    }
    match result[0] {
      wasmtime::Val::F64(bits) => Ok(f64::from_bits(bits)),
      _ => Err(format!("realm bridge {name} returned a non-number")),
    }
  }
}

#[cfg(feature = "js2wasm_runtime_compile")]
pub(crate) fn bootstrap_context_for_test(
  path: &Path,
  publish: impl FnOnce(Rc<RefCell<DenoRuntime>>) -> Result<(), String>,
) -> Result<Rc<RefCell<DenoRuntime>>, String> {
  let shared = SharedDenoRuntime::new()?;
  let bytes = fs::read(path).map_err(|e| e.to_string())?;
  let module = Module::new(&shared.engine, bytes).map_err(|e| e.to_string())?;
  let prepared = shared.prepare_module(&module)?;
  DenoRuntime::instantiate_published(
    &shared,
    &prepared,
    PathBuf::from("."),
    0,
    publish,
  )
}
