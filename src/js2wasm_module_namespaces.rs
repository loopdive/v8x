use super::*;

pub(super) fn bind(
  linker: &mut Linker<DenoHostState>,
  realm: Instance,
  module: &Module,
  bindings: Option<&NativeModuleGraph>,
) -> Result<(), String> {
  for import in module.imports().filter(|import| {
    import.module() == DENO_IMPORT_MODULE
      && import.name().starts_with("__v8x_module_namespace_")
  }) {
    let wasmtime::ExternType::Func(ty) = import.ty() else {
      return Err("module namespace capability must be a function".into());
    };
    if ty.params().len() != 0
      || ty.results().len() != 1
      || !ty.results().all(|ty| ty.is_externref())
    {
      return Err(
        "module namespace capability must have () -> externref ABI".into(),
      );
    }
    let bindings = bindings
      .ok_or("module namespace capability requires native graph bindings")?;
    let matching: Vec<_> = bindings
      .modules
      .iter()
      .filter(|(specifier, _)| {
        let suffix: String = specifier
          .as_bytes()
          .iter()
          .map(|byte| format!("{byte:02x}"))
          .collect();
        let suffix = if suffix.is_empty() { "00" } else { &suffix };
        import.name() == format!("__v8x_module_namespace_{suffix}")
      })
      .collect();
    if matching.len() != 1 {
      return Err(
        "module namespace capability has missing or ambiguous native identity"
          .into(),
      );
    }
    let target = matching[0].1;
    let context = bindings.context;
    linker
      .func_new(
        DENO_IMPORT_MODULE,
        import.name(),
        ty,
        move |mut caller, _, results| {
          let value = crate::js2wasm::realm_objects::existing_module_namespace(
            context as *const crate::Context,
            target as *const crate::Module,
            caller.data().realm_owner_identity,
          )
          .map_err(wasmtime::Error::msg)?;
          let Some(value) = value else {
            results[0] = wasmtime::Val::ExternRef(None);
            return Ok(());
          };
          let handle = value
            .checked_handle(caller.data().realm_id)
            .map_err(wasmtime::Error::msg)?;
          let unwrap = realm
            .get_func(&mut caller, "__v8x_value_unwrap")
            .ok_or_else(|| {
              wasmtime::Error::msg("Context lacks namespace unwrap")
            })?;
          unwrap.call(
            &mut caller,
            &[wasmtime::Val::F64(handle.to_bits())],
            results,
          )
        },
      )
      .map_err(|error| format!("bind native module namespace: {error:#}"))?;
  }
  Ok(())
}
