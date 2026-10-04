use super::*;

const INPUT_DIR: &str = "V8X_JS2WASM_AOT_SCRIPT_DIR";

pub(crate) fn configured() -> bool {
  std::env::var_os(INPUT_DIR).is_some()
}

// Inspect before instantiation: no Script effects or interpreter provider can
// run while proving that this artifact implements the native completion ABI.
pub(super) fn validate(prepared: &PreparedModule) -> Result<(), String> {
  let module = match prepared {
    PreparedModule::Prelinked(instance) => instance.module(),
    PreparedModule::RuntimeEval(module) => module,
  };
  let Some(wasmtime::ExternType::Func(init)) =
    module.get_export("__module_init")
  else {
    return Err("AOT Script lacks native __module_init".into());
  };
  if init.params().len() != 0 || init.results().len() != 0 {
    return Err("AOT Script initializer must have () -> () ABI".into());
  }
  let mut sinks = 0;
  for import in module.imports() {
    if import.module() != CONTEXT_IMPORT_MODULE {
      return Err(format!(
        "AOT Script forbids import {}::{}",
        import.module(),
        import.name()
      ));
    }
    if import.name() == "__v8x_context_script_completion" {
      let wasmtime::ExternType::Func(sink) = import.ty() else {
        return Err("AOT Script completion sink must be a function".into());
      };
      let params = sink.params().collect::<Vec<_>>();
      if params.len() != 1
        || !params[0].is_externref()
        || sink.results().len() != 0
      {
        return Err(
          "AOT Script completion sink must have (externref) -> () ABI".into(),
        );
      }
      sinks += 1;
    }
  }
  if sinks != 1 {
    return Err(
      "AOT Script requires exactly one native completion sink".into(),
    );
  }
  if let Some(get) = module.get_export("__v8x_script_get_export") {
    let wasmtime::ExternType::Func(get) = get else {
      return Err("AOT Script getter must be a function".into());
    };
    if get.params().len() != 2
      || !get.params().all(|ty| ty.is_externref())
      || get.results().len() != 1
      || !get.results().all(|ty| ty.is_externref())
    {
      return Err(
        "AOT Script getter must have (externref, externref) -> externref ABI"
          .into(),
      );
    }
    let Some(wasmtime::ExternType::Func(owns)) = module.get_export("localOwns")
    else {
      return Err("AOT Script getter requires allocation ownership".into());
    };
    if owns.params().len() != 1
      || !owns.params().all(|ty| ty.is_externref())
      || owns.results().len() != 1
      || !owns
        .results()
        .all(|ty| matches!(ty, wasmtime::ValType::I32))
    {
      return Err(
        "AOT Script ownership must have (externref) -> i32 ABI".into(),
      );
    }
  }
  if let Some(call) = module.get_export("__v8x_script_call_export") {
    let wasmtime::ExternType::Func(call) = call else {
      return Err("AOT Script call must be a function".into());
    };
    if call.params().len() != 3
      || !call.params().all(|ty| ty.is_externref())
      || call.results().len() != 1
      || !call.results().all(|ty| ty.is_externref())
    {
      return Err("AOT Script call must have three externref parameters and one externref result".into());
    }
    if module.get_export("__v8x_script_get_export").is_none() {
      return Err("AOT Script call requires validated getter/ownership".into());
    }
  }
  Ok(())
}

// Script goal and completion ABI are distinct from Module graph identity.
pub(super) fn digest(specifier: &str, source: &str) -> String {
  let mut hasher = Sha256::new();
  hasher.update(b"v8x/js2wasm Script completion ABI v1\0");
  update_graph_digest(&mut hasher, specifier.as_bytes());
  update_graph_digest(&mut hasher, source.as_bytes());
  format!("{:x}", hasher.finalize())
}

pub(super) fn configured_input(
  specifier: &str,
  source: &str,
) -> Option<PathBuf> {
  std::env::var_os(INPUT_DIR).map(|directory| {
    PathBuf::from(directory)
      .join(format!("{}.cwasm", digest(specifier, source)))
  })
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn script_binding_matches_build_side_utf8_digest() {
    assert_eq!(
      digest("é", "42;"),
      "5a142fa31a2cc229f816531945f3e10a387ed44e968dcf7e80eff85e36a964b5"
    );
    assert_ne!(digest("a", "bc"), digest("ab", "c"));
    assert_ne!(digest("a", "bc"), digest("b", "bc"));
    assert_ne!(digest("a", "bc"), digest("a", "bd"));
    assert_ne!(
      digest("a", "bc"),
      graph_digest(
        "a",
        &[SourceModule {
          specifier: "a".into(),
          source: "bc".into(),
        }]
      )
    );
  }

  #[cfg(feature = "js2wasm_runtime_compile")]
  #[test]
  fn native_script_abi_rejects_wrong_signatures_and_interpreter_imports() {
    let shared = SharedDenoRuntime::new().unwrap();
    let module =
      |sink_type, namespace: &str, init_params, extra: Option<&str>| {
        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        let mut section = |id, payload: Vec<u8>| {
          bytes.extend([id, payload.len() as u8]);
          bytes.extend(payload);
        };
        let mut types = vec![2, 0x60, 1, sink_type, 0, 0x60, init_params];
        if init_params == 1 {
          types.push(0x7c);
        }
        types.push(0);
        section(1, types);
        let mut imports = vec![1, namespace.len() as u8];
        imports.extend(namespace.as_bytes());
        let name = "__v8x_context_script_completion";
        imports.push(name.len() as u8);
        imports.extend(name.as_bytes());
        imports.extend([0, 0]);
        section(2, imports);
        section(3, vec![1, 1]);
        let name = "__module_init";
        let mut exports = vec![1, name.len() as u8];
        exports.extend(name.as_bytes());
        exports.extend([0, 1]);
        if let Some(extra) = extra {
          exports[0] = 2;
          exports.push(extra.len() as u8);
          exports.extend(extra.as_bytes());
          exports.extend([0, 1]);
        }
        section(7, exports);
        section(10, vec![1, 2, 0, 0x0b]);
        PreparedModule::RuntimeEval(Module::new(&shared.engine, bytes).unwrap())
      };
    validate(&module(0x6f, CONTEXT_IMPORT_MODULE, 0, None)).unwrap();
    assert!(
      validate(&module(0x7c, CONTEXT_IMPORT_MODULE, 0, None))
        .unwrap_err()
        .contains("(externref)")
    );
    assert!(
      validate(&module(0x6f, CONTEXT_IMPORT_MODULE, 1, None))
        .unwrap_err()
        .contains("initializer")
    );
    assert!(
      validate(&module(0x6f, RUNTIME_EVAL_IMPORT_MODULE, 0, None))
        .unwrap_err()
        .contains("forbids import")
    );
    for export in ["__v8x_script_get_export", "__v8x_script_call_export"] {
      assert!(
        validate(&module(0x6f, CONTEXT_IMPORT_MODULE, 0, Some(export)))
          .unwrap_err()
          .contains("Script")
      );
    }
  }
}
