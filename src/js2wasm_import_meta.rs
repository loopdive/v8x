use super::*;

/// Captured by each instantiated graph's imports, not mutable Store-global URL
/// state. Later calls into an older graph must retain its original Module.
pub(crate) struct NativeModuleGraph {
  pub(crate) context: usize,
  pub(crate) modules: Vec<(String, usize)>,
}

pub(super) fn bind(
  linker: &mut Linker<DenoHostState>,
  realm: Instance,
  module: &Module,
  bindings: Option<&NativeModuleGraph>,
) -> Result<(), String> {
  if let Some(bindings) = bindings {
    let mut specifiers = std::collections::HashSet::new();
    if bindings
      .modules
      .iter()
      .any(|(specifier, _)| !specifiers.insert(specifier))
    {
      return Err(
        "native graph has ambiguous duplicate module specifiers".into(),
      );
    }
  }
  for import in module.imports().filter(|import| {
    import.module() == DENO_IMPORT_MODULE
      && import.name().starts_with("__v8x_import_meta_")
  }) {
    let wasmtime::ExternType::Func(ty) = import.ty() else {
      return Err("import-meta capability must be a function".into());
    };
    if import.name() == "__v8x_import_meta_unwrap" {
      if ty.params().len() != 1
        || !ty.params().all(|ty| matches!(ty, wasmtime::ValType::F64))
        || ty.results().len() != 1
        || !ty.results().all(|ty| ty.is_externref())
      {
        return Err(
          "import-meta unwrap must have (f64) -> externref ABI".into(),
        );
      }
      linker
        .func_new(
          DENO_IMPORT_MODULE,
          import.name(),
          ty,
          move |mut caller, args, results| {
            let unwrap = realm
              .get_func(&mut caller, "__v8x_value_unwrap")
              .ok_or_else(|| {
                wasmtime::Error::msg("Context lacks value unwrap")
              })?;
            unwrap.call(&mut caller, args, results)
          },
        )
        .map_err(|error| format!("bind import-meta unwrap: {error:#}"))?;
      continue;
    }
    if ty.params().len() != 0
      || ty.results().len() != 1
      || !ty.results().all(|ty| matches!(ty, wasmtime::ValType::F64))
    {
      return Err("import-meta capability must have () -> f64 ABI".into());
    }
    let bindings = bindings.ok_or_else(|| {
      "import-meta requires native graph bindings".to_string()
    })?;
    let target = bindings
      .modules
      .iter()
      .find_map(|(specifier, target)| {
        let suffix: String = specifier
          .as_bytes()
          .iter()
          .map(|byte| format!("{byte:02x}"))
          .collect();
        let suffix = if suffix.is_empty() { "00" } else { &suffix };
        (import.name() == format!("__v8x_import_meta_{suffix}"))
          .then_some(*target)
      })
      .ok_or_else(|| {
        format!(
          "import-meta capability {} is absent from the native graph",
          import.name()
        )
      })?;
    let context = bindings.context;
    linker
      .func_wrap(
        DENO_IMPORT_MODULE,
        import.name(),
        move |caller: Caller<'_, DenoHostState>| -> wasmtime::Result<f64> {
          let owner = crate::js2wasm::realm_objects::import_meta_owner(
            context as *const crate::Context,
            caller.data().realm_owner_identity,
          )
          .map_err(wasmtime::Error::msg)?;
          let mut access =
            CallerRealm::new(caller).map_err(wasmtime::Error::msg)?;
          crate::js2wasm::realm_objects::import_meta_handle(
            &mut access,
            &owner,
            target as *const crate::Module,
            context as *const crate::Context,
          )
          .map_err(wasmtime::Error::msg)
        },
      )
      .map_err(|error| {
        format!("bind native import-meta capability: {error:#}")
      })?;
  }
  Ok(())
}
