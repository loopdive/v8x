use super::*;
use wasmtime::Val;

// A Script import must read foreign Script values through their owning getter,
// not through the Context's unrelated closed-object layouts. Bind the normal
// Context provider as fallback only when no retained Script owns the receiver.
pub(super) fn bind(
  linker: &mut Linker<DenoHostState>,
  realm: Instance,
  module: &Module,
) -> Result<(), String> {
  bind_foreign_ownership(linker, realm, module)?;
  bind_foreign_call(linker, realm, module)?;
  let Some(import) = module.imports().find(|import| {
    import.module() == CONTEXT_IMPORT_MODULE
      && import.name() == "__v8x_context_get"
  }) else {
    return Ok(());
  };
  let wasmtime::ExternType::Func(ty) = import.ty() else {
    return Err("Context getter import must be a function".into());
  };
  if ty.params().len() != 3
    || !ty.params().all(|ty| ty.is_externref())
    || ty.results().len() != 1
    || !ty.results().all(|ty| ty.is_externref())
  {
    return Err("Context getter import must have three externref parameters and one externref result".into());
  }
  linker
    .func_new(
      CONTEXT_IMPORT_MODULE,
      "__v8x_context_get",
      ty,
      move |caller, args, results| {
        let graphs = caller.data().aot_call_graphs.clone();
        // Results must remain rooted until Wasmtime consumes the host return.
        // An inner RootScope would unroot them before the trampoline sees them.
        let mut scope = caller;
        for graph in graphs.iter().rev() {
          let Some(owns) = graph.get_func(&mut scope, "localOwns") else {
            continue;
          };
          let mut matched = [Val::I32(0)];
          owns.call(&mut scope, &args[..1], &mut matched)?;
          match matched[0].i32() {
            Some(0) => continue,
            Some(1) => {}
            _ => {
              return Err(wasmtime::Error::msg(
                "Script owner must return zero or one",
              ));
            }
          }
          let get = graph
            .get_func(&mut scope, "__v8x_script_get_export")
            .ok_or_else(|| {
              wasmtime::Error::msg("owning Script lacks native getter")
            })?;
          let keep =
            realm.get_func(&mut scope, "__v8x_value_keep").ok_or_else(
              || wasmtime::Error::msg("Context lacks native keeper"),
            )?;
          let mut object = [Val::F64(0)];
          let mut receiver = [Val::F64(0)];
          keep.call(&mut scope, &args[..1], &mut object)?;
          keep.call(&mut scope, &args[2..3], &mut receiver)?;
          if object[0].f64() != receiver[0].f64() {
            return Err(wasmtime::Error::msg(
              "foreign Script getter requires an explicit Reflect receiver ABI",
            ));
          }
          // Forward the pending Wasm exception unchanged. Taking it here would
          // prevent the caller's JS catch from observing the original payload.
          return get.call(&mut scope, &args[..2], results);
        }
        let fallback = realm
          .get_func(&mut scope, "__v8x_context_get")
          .ok_or_else(|| {
            wasmtime::Error::msg("Context lacks getter fallback")
          })?;
        fallback.call(&mut scope, args, results)
      },
    )
    .map_err(|error| format!("bind owning Script getter: {error:#}"))?;
  Ok(())
}

fn bind_foreign_ownership(
  linker: &mut Linker<DenoHostState>,
  realm: Instance,
  module: &Module,
) -> Result<(), String> {
  let Some(import) = module.imports().find(|import| {
    import.module() == CONTEXT_IMPORT_MODULE
      && import.name() == "__v8x_context_owns"
  }) else {
    return Ok(());
  };
  let wasmtime::ExternType::Func(ty) = import.ty() else {
    return Err("Context ownership import must be a function".into());
  };
  if ty.params().len() != 1
    || !ty.params().all(|ty| ty.is_externref())
    || ty.results().len() != 1
    || !ty.results().all(|ty| matches!(ty, wasmtime::ValType::I32))
  {
    return Err(
      "Context ownership import must have (externref) -> i32 ABI".into(),
    );
  }
  linker
    .func_new(
      CONTEXT_IMPORT_MODULE,
      "__v8x_context_owns",
      ty,
      move |mut caller, args, results| {
        let original = realm
          .get_func(&mut caller, "__v8x_context_owns")
          .ok_or_else(|| {
            wasmtime::Error::msg("Context lacks ownership predicate")
          })?;
        original.call(&mut caller, args, results)?;
        match results[0].i32() {
          Some(1) => return Ok(()),
          Some(0) => {}
          _ => {
            return Err(wasmtime::Error::msg(
              "Context owner must return zero or one",
            ));
          }
        }
        let Some(wasmtime::Extern::Func(local)) =
          caller.get_export("localOwns")
        else {
          return Err(wasmtime::Error::msg(
            "linked caller lacks allocation ownership",
          ));
        };
        let mut own = [Val::I32(0)];
        local.call(&mut caller, args, &mut own)?;
        match own[0].i32() {
          Some(1) => {
            results[0] = Val::I32(0);
            return Ok(());
          }
          Some(0) => {}
          _ => {
            return Err(wasmtime::Error::msg(
              "caller owner must return zero or one",
            ));
          }
        }
        let graphs = caller.data().aot_call_graphs.clone();
        for graph in graphs.iter().rev() {
          let Some(owns) = graph.get_func(&mut caller, "localOwns") else {
            continue;
          };
          owns.call(&mut caller, args, &mut own)?;
          match own[0].i32() {
            Some(1) => {
              results[0] = Val::I32(1);
              return Ok(());
            }
            Some(0) => {}
            _ => {
              return Err(wasmtime::Error::msg(
                "foreign owner must return zero or one",
              ));
            }
          }
        }
        results[0] = Val::I32(0);
        Ok(())
      },
    )
    .map_err(|error| format!("bind foreign allocation ownership: {error:#}"))?;
  Ok(())
}

fn bind_foreign_call(
  linker: &mut Linker<DenoHostState>,
  realm: Instance,
  module: &Module,
) -> Result<(), String> {
  let Some(import) = module.imports().find(|import| {
    import.module() == CONTEXT_IMPORT_MODULE
      && import.name() == "__v8x_context_call"
  }) else {
    return Ok(());
  };
  let wasmtime::ExternType::Func(ty) = import.ty() else {
    return Err("Context call import must be a function".into());
  };
  if ty.params().len() != 3
    || !ty.params().all(|ty| ty.is_externref())
    || ty.results().len() != 1
    || !ty.results().all(|ty| ty.is_externref())
  {
    return Err("Context call import must have three externref parameters and one externref result".into());
  }
  linker
    .func_new(
      CONTEXT_IMPORT_MODULE,
      "__v8x_context_call",
      ty,
      move |mut caller, args, results| {
        let graphs = caller.data().aot_call_graphs.clone();
        for graph in graphs.iter().rev() {
          let Some(owns) = graph.get_func(&mut caller, "localOwns") else {
            continue;
          };
          let mut own = [Val::I32(0)];
          owns.call(&mut caller, &args[..1], &mut own)?;
          if own[0].i32() == Some(0) {
            continue;
          }
          if own[0].i32() != Some(1) {
            return Err(wasmtime::Error::msg(
              "callee owner must return zero or one",
            ));
          }
          let call = graph
            .get_func(&mut caller, "__v8x_script_call_export")
            .ok_or_else(|| {
              wasmtime::Error::msg("owning Script lacks native call")
            })?;
          return call.call(&mut caller, args, results);
        }
        let fallback = realm
          .get_func(&mut caller, "__v8x_context_call")
          .ok_or_else(|| wasmtime::Error::msg("Context lacks call fallback"))?;
        fallback.call(&mut caller, args, results)
      },
    )
    .map_err(|error| format!("bind owning Script call: {error:#}"))?;
  Ok(())
}
