use super::*;

// Retain the actual native result in the same store. Script allocation owners
// take precedence over Context helpers, including for descriptor reads.
fn reflect(
  runtime: &mut dyn RealmAccess,
  entry: &RealmObjectBinding,
  operation: &str,
  key: Option<RealmValue>,
) -> Result<Option<RealmValue>, String> {
  let mut handles = vec![runtime.realm_check(entry.value)?];
  if let Some(key) = key {
    handles.push(runtime.realm_check(key)?);
  }
  if let Some((success, handle)) =
    runtime.realm_try_graph_reflection(&handles, operation)?
  {
    let value = runtime.realm_from_handle(handle)?;
    if !success {
      let exception = from_realm(runtime, &entry.runtime, value)?;
      record_exception(current_isolate(), exception);
      return Ok(None);
    }
    return Ok(Some(value));
  }
  let export = match operation {
    "names" => "__v8x_value_own_names",
    "symbols" => "__v8x_value_own_symbols",
    "descriptor" => "__v8x_value_descriptor",
    _ => return Err("unknown native property reflection".into()),
  };
  runtime.realm_handle(export, &handles).map(Some)
}

fn read(
  runtime: &mut dyn RealmAccess,
  object: RealmValue,
  key: &str,
) -> Result<RealmValue, String> {
  let key = runtime.realm_string(&key.encode_utf16().collect::<Vec<_>>())?;
  runtime.realm_get(object, key)
}

fn array_index(text: &str) -> Option<u32> {
  let index = text.parse::<u32>().ok()?;
  (index < u32::MAX && index.to_string() == text).then_some(index)
}

pub(crate) fn own_property_names(
  object: *const Object,
  filter: PropertyFilter,
  conversion: KeyConversionMode,
) -> Option<Result<*const Array, String>> {
  let entry = binding(object)?;
  Some(callback_access::with_owner(&entry.runtime, |runtime| {
    let mut elements = Vec::new();
    for (operation, skip) in [
      ("names", filter.is_skip_strings()),
      ("symbols", filter.is_skip_symbols()),
    ] {
      if skip {
        continue;
      }
      let Some(keys) = reflect(runtime, &entry, operation, None)? else {
        return Ok(ptr::null());
      };
      let length = read(runtime, keys, "length")?;
      let length = runtime.realm_as_number(length)?;
      if !length.is_finite()
        || length < 0.0
        || length.fract() != 0.0
        || length > u32::MAX as f64
      {
        return Err(
          "native reflection returned invalid key array length".into(),
        );
      }
      for index in 0..length as u32 {
        let key = read(runtime, keys, &index.to_string())?;
        if filter.is_only_writable()
          || filter.is_only_enumerable()
          || filter.is_only_configurable()
        {
          let Some(descriptor) =
            reflect(runtime, &entry, "descriptor", Some(key))?
          else {
            return Ok(ptr::null());
          };
          if runtime.realm_kind(descriptor)? == 0 {
            continue;
          }
          let mut accepted = true;
          for (required, attribute) in [
            (filter.is_only_writable(), "writable"),
            (filter.is_only_enumerable(), "enumerable"),
            (filter.is_only_configurable(), "configurable"),
          ] {
            if !required {
              continue;
            }
            let value = read(runtime, descriptor, attribute)?;
            if runtime.realm_kind(value)? != 2
              || !runtime.realm_as_boolean(value)?
            {
              accepted = false;
              break;
            }
          }
          if !accepted {
            continue;
          }
        }
        let key = if operation == "names" {
          if runtime.realm_kind(key)? != 4 {
            return Err("native own names contained a non-string key".into());
          }
          let text = String::from_utf16(&runtime.realm_as_utf16(key)?)
            .map_err(|_| "property key has unpaired UTF-16 surrogates")?;
          match (array_index(&text), conversion) {
            (Some(_), KeyConversionMode::NoNumbers) => continue,
            (Some(index), KeyConversionMode::KeepNumbers) => {
              allocate(current_isolate(), HeapValue::Number(index as f64))
            }
            _ => from_realm(runtime, &entry.runtime, key)?,
          }
        } else {
          if runtime.realm_kind(key)? != 8 {
            return Err("native own symbols contained a non-symbol key".into());
          }
          from_realm(runtime, &entry.runtime, key)?
        };
        elements.push(key);
      }
    }
    Ok(allocate(
      current_isolate(),
      HeapValue::Array(ArrayState {
        elements,
        properties: Vec::new(),
      }),
    ))
  }))
}

#[cfg(test)]
mod tests {
  use super::array_index;
  #[test]
  fn canonical_array_index_names_are_not_numeric_string_coercions() {
    assert_eq!(array_index("0"), Some(0));
    assert_eq!(array_index("4294967294"), Some(u32::MAX - 1));
    for text in ["01", "-0", "+1", "1.0", " 1", "4294967295", ""] {
      assert_eq!(array_index(text), None);
    }
  }
}
