use super::*;

// Canonical build/runtime factory identity. The hex header binds each parameter
// separately (including empty strings) without introducing source text into a
// comment. Function bodies remain byte-for-byte intact inside the factory.
fn factory_source(body: &str, parameters: &[String]) -> String {
  let encoded = parameters
    .iter()
    .map(|name| {
      name
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
    })
    .collect::<Vec<_>>();
  format!(
    "/*v8x CompileFunction v1 {}*/\n(function({}) {{\n{}\n}})",
    serde_json::to_string(&encoded).unwrap(),
    parameters.join(","),
    body
  )
}

#[cfg(test)]
mod tests {
  use super::factory_source;
  #[test]
  fn factory_binding_matches_build_side_bytes() {
    assert_eq!(
      factory_source("return value + 1;", &["value".into()]),
      "/*v8x CompileFunction v1 [\"76616c7565\"]*/\n(function(value) {\nreturn value + 1;\n})"
    );
    assert_ne!(factory_source("", &[]), factory_source("", &["".into()]));
    assert_ne!(
      factory_source("", &["a,b".into()]),
      factory_source("", &["a".into(), "b".into()])
    );
    assert!(
      factory_source("", &["é".into()])
        .starts_with("/*v8x CompileFunction v1 [\"c3a9\"]*/")
    );
  }
}

#[unsafe(no_mangle)]
pub extern "C" fn v8__ScriptCompiler__CompileFunction(
  context: *const Context,
  source: *mut Source,
  arguments_count: usize,
  arguments: *const *const V8String,
  context_extensions_count: usize,
  _context_extensions: *const *const Object,
  options: CompileOptions,
  _no_cache_reason: NoCacheReason,
) -> *const crate::Function {
  let result = (|| {
    if context_extensions_count != 0 {
      return Err(
        "AOT CompileFunction context extensions are not implemented".into(),
      );
    }
    if !options.difference(CompileOptions::EagerCompile).is_empty() {
      return Err(
        "AOT CompileFunction does not consume V8 code caches or compile hints"
          .into(),
      );
    }
    if source.is_null() || (arguments_count != 0 && arguments.is_null()) {
      return Err(
        "AOT CompileFunction received invalid source or parameter storage"
          .into(),
      );
    }
    let words = source.cast::<usize>();
    let body = unsafe { string_value(words.read() as *const V8String) }
      .ok_or("AOT CompileFunction source is not a string")?
      .to_owned();
    let origin = unsafe { words.add(1).read() as *const Value };
    let specifier = unsafe { string_value(origin) }
      .unwrap_or("<anonymous>")
      .to_owned();
    let mut parameters = Vec::with_capacity(arguments_count);
    for index in 0..arguments_count {
      parameters.push(
        unsafe { string_value(arguments.add(index).read()) }
          .ok_or("AOT CompileFunction parameter is not a string")?
          .to_owned(),
      );
    }
    let owner = match unsafe { heap_value(context) } {
      Some(HeapValue::Context(state)) => state
        .deno_core_bootstrap
        .as_ref()
        .or(state.module_runtime.as_ref())
        .cloned(),
      _ => None,
    }
    .ok_or("AOT CompileFunction has no live Context runtime")?;
    let factory = factory_source(&body, &parameters);
    let resource = format!("v8x:CompileFunction:{specifier}");
    realm_objects::callback_access::with_owner(&owner, |runtime| {
      let (normal, value) = runtime
        .realm_run_aot_script(&resource, &factory, true)?
        .ok_or("AOT CompileFunction requires trusted function packages")?;
      let value = realm_objects::from_realm(runtime, &owner, value)?;
      if !normal {
        record_exception(current_isolate(), value);
        return Ok(ptr::null());
      }
      if !matches!(unsafe { heap_value(value) }, Some(HeapValue::Function(_))) {
        return Err(
          "AOT CompileFunction factory did not return a callable".into(),
        );
      }
      Ok(value.cast())
    })
  })();
  match result {
    Ok(function) => function,
    Err(error) => {
      realm_objects::report(error);
      ptr::null()
    }
  }
}
