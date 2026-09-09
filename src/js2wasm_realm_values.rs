use super::*;

/// Store-local typed functions. Validation happens once, before caching.
#[derive(Clone)]
pub(super) enum RealmFunction {
  N0(wasmtime::TypedFunc<(), f64>),
  V0(wasmtime::TypedFunc<(), ()>),
  N1(wasmtime::TypedFunc<f64, f64>),
  V1(wasmtime::TypedFunc<f64, ()>),
  N2(wasmtime::TypedFunc<(f64, f64), f64>),
  V2(wasmtime::TypedFunc<(f64, f64), ()>),
  N3(wasmtime::TypedFunc<(f64, f64, f64), f64>),
  V3(wasmtime::TypedFunc<(f64, f64, f64), ()>),
  N4(wasmtime::TypedFunc<(f64, f64, f64, f64), f64>),
  V4(wasmtime::TypedFunc<(f64, f64, f64, f64), ()>),
}

fn realm_call(
  mut store: wasmtime::StoreContextMut<'_, DenoHostState>,
  instance: Instance,
  name: &str,
  args: &[f64],
  returns: bool,
) -> wasmtime::Result<f64> {
  let function = if let Some(function) = store.data().realm_functions.get(name)
  {
    function.clone()
  } else {
    let function = match (args.len(), returns) {
      (0, true) => {
        RealmFunction::N0(instance.get_typed_func(&mut store, name)?)
      }
      (0, false) => {
        RealmFunction::V0(instance.get_typed_func(&mut store, name)?)
      }
      (1, true) => {
        RealmFunction::N1(instance.get_typed_func(&mut store, name)?)
      }
      (1, false) => {
        RealmFunction::V1(instance.get_typed_func(&mut store, name)?)
      }
      (2, true) => {
        RealmFunction::N2(instance.get_typed_func(&mut store, name)?)
      }
      (2, false) => {
        RealmFunction::V2(instance.get_typed_func(&mut store, name)?)
      }
      (3, true) => {
        RealmFunction::N3(instance.get_typed_func(&mut store, name)?)
      }
      (3, false) => {
        RealmFunction::V3(instance.get_typed_func(&mut store, name)?)
      }
      (4, true) => {
        RealmFunction::N4(instance.get_typed_func(&mut store, name)?)
      }
      (4, false) => {
        RealmFunction::V4(instance.get_typed_func(&mut store, name)?)
      }
      _ => {
        return Err(wasmtime::Error::msg("unsupported realm bridge signature"));
      }
    };
    store
      .data_mut()
      .realm_functions
      .insert(name.to_owned(), function.clone());
    function
  };
  match (function, args, returns) {
    (RealmFunction::N0(f), [], true) => f.call(&mut store, ()),
    (RealmFunction::V0(f), [], false) => f.call(&mut store, ()).map(|()| 0.0),
    (RealmFunction::N1(f), [a0], true) => f.call(&mut store, *a0),
    (RealmFunction::V1(f), [a0], false) => {
      f.call(&mut store, *a0).map(|()| 0.0)
    }
    (RealmFunction::N2(f), [a0, a1], true) => f.call(&mut store, (*a0, *a1)),
    (RealmFunction::V2(f), [a0, a1], false) => {
      f.call(&mut store, (*a0, *a1)).map(|()| 0.0)
    }
    (RealmFunction::N3(f), [a0, a1, a2], true) => {
      f.call(&mut store, (*a0, *a1, *a2))
    }
    (RealmFunction::V3(f), [a0, a1, a2], false) => {
      f.call(&mut store, (*a0, *a1, *a2)).map(|()| 0.0)
    }
    (RealmFunction::N4(f), [a0, a1, a2, a3], true) => {
      f.call(&mut store, (*a0, *a1, *a2, *a3))
    }
    (RealmFunction::V4(f), [a0, a1, a2, a3], false) => {
      f.call(&mut store, (*a0, *a1, *a2, *a3)).map(|()| 0.0)
    }
    _ => Err(wasmtime::Error::msg("realm bridge signature changed")),
  }
}

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

impl RealmAccess for DenoRuntime {
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
    use wasmtime::AsContextMut;
    let start = self.store.data().realm_profile.start();
    let outcome = realm_call(
      self.store.as_context_mut(),
      self.realm_instance,
      name,
      args,
      returns,
    );
    self.store.data_mut().realm_profile.finish(name, start);
    outcome.map_err(|error| {
      let payload =
        render_pending_wasm_exception(&mut self.store, self.realm_instance);
      format!("call realm bridge {name}: {error:#}; {payload}")
    })
  }
}

pub(crate) trait RealmAccess {
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
    if value.owner != self.realm_id() {
      return Err("value belongs to a different Wasmtime realm".to_string());
    }
    Ok(value.handle)
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
  let prepared = shared.prepare_module(&module)?;
  DenoRuntime::instantiate(&shared, &prepared, PathBuf::from("."), 0)
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
  // Cached signatures must reject mismatches without poisoning later calls.
  assert!(
    runtime
      .realm_raw("__v8x_value_global", &[0.0], true)
      .is_err()
  );
  assert!(runtime.realm_raw("__v8x_value_global", &[], false).is_err());
  assert_eq!(runtime.realm_global()?, global);
  assert!(
    runtime
      .realm_raw("__v8x_missing_export", &[], true)
      .is_err()
  );
  assert!(runtime.realm_raw("__v8x_value_number", &[], true).is_err());
  assert!(runtime.realm_number(7.0).is_ok());
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
impl<'a> CallerRealm<'a> {
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
    use wasmtime::AsContextMut;
    let start = self.caller.data().realm_profile.start();
    let outcome = realm_call(
      self.caller.as_context_mut(),
      self.realm_instance,
      name,
      args,
      returns,
    );
    self.caller.data_mut().realm_profile.finish(name, start);
    outcome.map_err(|error| format!("call realm bridge {name}: {error:#}"))
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
