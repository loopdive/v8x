use super::*;
use wasmtime::{AnyRef, AsContextMut, RootScope, StoreContextMut, Val};

/// Compiler-private, read-only UTF-16 storage ABI. All references remain rooted
/// through Wasmtime; no raw addresses survive a call or collection.
pub(super) fn read(
  store: StoreContextMut<'_, DenoHostState>,
  instance: Instance,
  handle: f64,
) -> Result<Option<Vec<u16>>, String> {
  let mut roots = RootScope::new(store);
  let mut store = roots.as_context_mut();
  let Some(export) =
    instance.get_func(&mut store, "__v8x_value_string_storage")
  else {
    return Ok(None);
  };
  (|| -> wasmtime::Result<Vec<u16>> {
    let mut raw = [Val::ExternRef(None)];
    export.call(&mut store, &[Val::F64(handle.to_bits())], &mut raw)?;
    let external = raw[0]
      .externref()
      .and_then(|v| v.copied())
      .ok_or_else(|| wasmtime::Error::msg("string storage is not externref"))?;
    let value = AnyRef::convert_extern(&mut store, external)?;
    let root = value
      .as_struct(&store)?
      .ok_or_else(|| wasmtime::Error::msg("string storage is not a struct"))?;
    let length = root
      .field(&mut store, 0)?
      .i32()
      .filter(|n| *n >= 0)
      .ok_or_else(|| wasmtime::Error::msg("invalid string length ABI"))?
      as usize;
    let mut output = Vec::new();
    output.try_reserve_exact(length)?;
    let mut pending = vec![(root, length)];
    while let Some((node, expected)) = pending.pop() {
      if node.field(&mut store, 0)?.i32() != Some(expected as i32) {
        return Err(wasmtime::Error::msg("inconsistent string node length"));
      }
      let second = node.field(&mut store, 1)?;
      let third = node.field(&mut store, 2)?;
      if let Some(offset) = second.i32() {
        // NativeString (including its hashed subtype): len, offset, i16 array.
        let data = third
          .anyref()
          .and_then(|v| v.copied())
          .ok_or_else(|| {
            wasmtime::Error::msg("missing string code-unit array")
          })?
          .as_array(&store)?
          .ok_or_else(|| {
            wasmtime::Error::msg("invalid string code-unit array")
          })?;
        let capacity = data.len(&store)? as usize;
        if offset < 0
          || !matches!(
            data.ty(&store)?.element_type(),
            wasmtime::StorageType::I16
          )
          || (offset as usize)
            .checked_add(expected)
            .is_none_or(|end| end > capacity)
        {
          return Err(wasmtime::Error::msg("incompatible flat string ABI"));
        }
        for index in offset as usize..offset as usize + expected {
          let unit = data
            .get(&mut store, index as u32)?
            .i32()
            .ok_or_else(|| wasmtime::Error::msg("invalid UTF-16 storage"))?;
          output.push(unit as u16);
        }
      } else {
        // ConsString: len, left, right. Walk iteratively to avoid Rust stack
        // growth on deep concatenation trees, including memoized flat ropes.
        let left = second
          .anyref()
          .and_then(|v| v.copied())
          .ok_or_else(|| wasmtime::Error::msg("missing left string"))?
          .as_struct(&store)?
          .ok_or_else(|| wasmtime::Error::msg("invalid left string"))?;
        let right = third
          .anyref()
          .and_then(|v| v.copied())
          .ok_or_else(|| wasmtime::Error::msg("missing right string"))?
          .as_struct(&store)?
          .ok_or_else(|| wasmtime::Error::msg("invalid right string"))?;
        let left_len = left
          .field(&mut store, 0)?
          .i32()
          .filter(|n| *n >= 0)
          .ok_or_else(|| wasmtime::Error::msg("invalid left string length"))?
          as usize;
        let right_len = right
          .field(&mut store, 0)?
          .i32()
          .filter(|n| *n >= 0)
          .ok_or_else(|| wasmtime::Error::msg("invalid right string length"))?
          as usize;
        if left_len.checked_add(right_len) != Some(expected) {
          return Err(wasmtime::Error::msg("incompatible rope string ABI"));
        }
        if right_len != 0 {
          pending.push((right, right_len));
        }
        if left_len != 0 {
          pending.push((left, left_len));
        }
      }
      if output.len() > length {
        return Err(wasmtime::Error::msg(
          "string storage exceeded declared length",
        ));
      }
    }
    if output.len() != length {
      return Err(wasmtime::Error::msg("string storage length mismatch"));
    }
    Ok(output)
  })()
  .map(Some)
  .map_err(|error| format!("read realm UTF-16 storage: {error:#}"))
}
