use super::*;
use crate::js2wasm::RetainedHostBuffer;
use wasmtime::{
  AnyRef, ArrayRef, AsContextMut, CallHook, OwnedRooted, RootScope,
  StoreContextMut, Val,
};

pub(super) struct HostBufferBinding {
  host: RetainedHostBuffer,
  bytes: OwnedRooted<ArrayRef>,
  handle: f64,
}

/// One-shot packet, deliberately not registered with the persistent buffer
/// synchronization hook. The consuming export releases its numeric root.
pub(super) fn packet(
  store: StoreContextMut<'_, DenoHostState>,
  instance: Instance,
  input: &[u8],
) -> Result<f64, String> {
  if input.len() > i32::MAX as usize {
    return Err("transfer packet exceeds buffer ABI".into());
  }
  let mut roots = RootScope::new(store);
  let mut store = roots.as_context_mut();
  (|| -> wasmtime::Result<f64> {
    let create = instance
      .get_typed_func::<f64, f64>(&mut store, "__v8x_value_buffer_create")?;
    let handle = create.call(&mut store, input.len() as f64)?;
    let storage = instance
      .get_func(&mut store, "__v8x_value_buffer_storage")
      .ok_or_else(|| wasmtime::Error::msg("missing packet storage export"))?;
    let mut raw = [Val::ExternRef(None)];
    storage.call(&mut store, &[Val::F64(handle.to_bits())], &mut raw)?;
    let external = raw[0]
      .externref()
      .and_then(|v| v.copied())
      .ok_or_else(|| wasmtime::Error::msg("packet storage is not externref"))?;
    let value = AnyRef::convert_extern(&mut store, external)?;
    let vector = value
      .as_struct(&store)?
      .ok_or_else(|| wasmtime::Error::msg("packet storage is not a struct"))?;
    if vector.field(&mut store, 0)?.i32() != Some(input.len() as i32) {
      return Err(wasmtime::Error::msg("invalid packet length ABI"));
    }
    let data = vector.field(&mut store, 1)?;
    let array = data
      .anyref()
      .and_then(|v| v.copied())
      .ok_or_else(|| wasmtime::Error::msg("missing packet byte array"))?
      .as_array(&store)?
      .ok_or_else(|| wasmtime::Error::msg("invalid packet byte array"))?;
    if array.len(&store)? as usize != input.len()
      || !matches!(array.ty(&store)?.element_type(), wasmtime::StorageType::I8)
    {
      return Err(wasmtime::Error::msg("incompatible packet byte ABI"));
    }
    for (index, byte) in input.iter().enumerate() {
      array.set(&mut store, index as u32, Val::I32(*byte as i32))?;
    }
    Ok(handle)
  })()
  .map_err(|error| format!("write transfer packet: {error:#}"))
}

/// No Wasm calls here: a return hook may run with a pending Wasm exception.
pub(super) fn synchronize(
  store: StoreContextMut<'_, DenoHostState>,
  event: CallHook,
) -> wasmtime::Result<()> {
  if store.data().host_buffers.is_empty() {
    return Ok(());
  }
  let mut roots = RootScope::new(store);
  let mut store = roots.as_context_mut();
  let mut bindings = std::mem::take(&mut store.data_mut().host_buffers);
  let result = (|| -> wasmtime::Result<()> {
    for binding in &mut bindings {
      let bytes = binding.bytes.to_rooted(&mut store);
      if bytes.len(&store)? as usize != binding.host.len() {
        return Err(wasmtime::Error::msg("host buffer storage length changed"));
      }
      for index in 0..binding.host.len() {
        if event.exiting_host() {
          bytes.set(
            &mut store,
            index as u32,
            Val::I32(binding.host.read(index) as i32),
          )?;
        } else {
          let value =
            bytes.get(&mut store, index as u32)?.i32().ok_or_else(|| {
              wasmtime::Error::msg("invalid host buffer byte storage")
            })?;
          binding.host.write(index, value as u8);
        }
      }
    }
    Ok(())
  })();
  store.data_mut().host_buffers = bindings;
  result
}

pub(super) fn adopt(
  store: StoreContextMut<'_, DenoHostState>,
  instance: Instance,
  host: RetainedHostBuffer,
) -> Result<f64, String> {
  let mut roots = RootScope::new(store);
  let mut store = roots.as_context_mut();
  if let Some(binding) = store
    .data()
    .host_buffers
    .iter()
    .find(|b| b.host.identity() == host.identity())
  {
    return Ok(binding.handle);
  }
  let result = (|| -> wasmtime::Result<f64> {
    let create = instance
      .get_typed_func::<f64, f64>(&mut store, "__v8x_value_buffer_create")?;
    let handle = create.call(&mut store, host.len() as f64)?;
    let storage = instance
      .get_func(&mut store, "__v8x_value_buffer_storage")
      .ok_or_else(|| {
        wasmtime::Error::msg("missing host buffer storage export")
      })?;
    let mut raw = [Val::ExternRef(None)];
    storage.call(&mut store, &[Val::F64(handle.to_bits())], &mut raw)?;
    let external =
      raw[0].externref().and_then(|v| v.copied()).ok_or_else(|| {
        wasmtime::Error::msg("host buffer storage is not an externref")
      })?;
    let value = AnyRef::convert_extern(&mut store, external)?;
    let vector = value.as_struct(&store)?.ok_or_else(|| {
      wasmtime::Error::msg("host buffer storage is not a struct")
    })?;
    if vector.field(&mut store, 0)?.i32() != Some(host.len() as i32) {
      return Err(wasmtime::Error::msg("incompatible host buffer length ABI"));
    }
    let data = vector.field(&mut store, 1)?;
    let array = data
      .anyref()
      .and_then(|v| v.copied())
      .ok_or_else(|| {
        wasmtime::Error::msg("host buffer bytes are not a GC reference")
      })?
      .as_array(&store)?
      .ok_or_else(|| {
        wasmtime::Error::msg("host buffer bytes are not an array")
      })?;
    if array.len(&store)? as usize != host.len()
      || !matches!(array.ty(&store)?.element_type(), wasmtime::StorageType::I8)
    {
      return Err(wasmtime::Error::msg("incompatible host buffer byte ABI"));
    }
    for index in 0..host.len() {
      array.set(&mut store, index as u32, Val::I32(host.read(index) as i32))?;
    }
    let bytes = array.to_owned_rooted(&mut store)?;
    store.data_mut().host_buffers.push(HostBufferBinding {
      host,
      bytes,
      handle,
    });
    Ok(handle)
  })();
  result.map_err(|error| format!("adopt host ArrayBuffer: {error:#}"))
}
