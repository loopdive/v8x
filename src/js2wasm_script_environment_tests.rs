use super::*;

// Trusted local fixtures are built/precompiled in a separate packaging process.
// This function uses the deployment deserializer and retained-Context linker;
// it neither invokes a compiler nor supplies a runtime-eval provider.
pub fn precompiled_scripts_share_context_lexicals(
  path: &Path,
) -> Result<(), String> {
  let shared = SharedDenoRuntime::new()?;
  let context = shared.precompiled_file(&path.join("context.cwasm"))?;
  let scripts = (0..11)
    .map(|index| {
      shared.precompiled_file(&path.join(format!("script-{index}.cwasm")))
    })
    .collect::<Result<Vec<_>, _>>()?;
  let mut first =
    DenoRuntime::instantiate(&shared, &context, PathBuf::from("."), 0)?;
  let mut second =
    DenoRuntime::instantiate(&shared, &context, PathBuf::from("."), 0)?;
  let read_number =
    |runtime: &mut DenoRuntime, name: &str| -> Result<f64, String> {
      runtime
        .instance
        .get_typed_func::<(), f64>(&mut runtime.store, name)
        .map_err(|error| error.to_string())?
        .call(&mut runtime.store, ())
        .map_err(|error| error.to_string())
    };
  let read_bool =
    |runtime: &mut DenoRuntime, name: &str| -> Result<i32, String> {
      runtime
        .instance
        .get_typed_func::<(), i32>(&mut runtime.store, name)
        .map_err(|error| error.to_string())?
        .call(&mut runtime.store, ())
        .map_err(|error| error.to_string())
    };
  assert_eq!(read_number(&mut first, "__v8x_probe_script_score")?, 0.0);
  assert_eq!(read_bool(&mut first, "__v8x_probe_script_has_retained")?, 0);
  for script in &scripts[..3] {
    first.instantiate_graph(&shared, script)?;
  }
  assert_eq!(read_number(&mut first, "__v8x_probe_script_score")?, 1.0);
  assert_eq!(
    read_number(&mut first, "__v8x_probe_script_observed")?,
    42.0
  );
  assert_eq!(read_bool(&mut first, "__v8x_probe_script_has_retained")?, 1);
  assert_eq!(read_number(&mut second, "__v8x_probe_script_score")?, 0.0);
  assert_eq!(
    read_bool(&mut second, "__v8x_probe_script_has_retained")?,
    0
  );
  let error = first.instantiate_graph(&shared, &scripts[3]).unwrap_err();
  assert!(error.contains("__module_init"), "{error}");
  assert_eq!(read_bool(&mut first, "__v8x_probe_script_has_first")?, 0);
  assert_eq!(
    read_number(&mut first, "__v8x_probe_script_observed")?,
    42.0
  );
  first.instantiate_graph(&shared, &scripts[4])?;
  first.instantiate_graph(&shared, &scripts[5])?;
  assert_eq!(read_number(&mut first, "__v8x_probe_script_fixed")?, 41.0);
  assert_eq!(read_number(&mut first, "__v8x_probe_script_caught")?, 42.0);
  first.instantiate_graph(&shared, &scripts[6])?;
  first.instantiate_graph(&shared, &scripts[7])?;
  assert_eq!(
    read_number(&mut first, "__v8x_probe_script_observed")?,
    43.0
  );
  let number_handle = first.instantiate_script(&shared, &scripts[8])?;
  assert!(number_handle.0 && number_handle.1 > 0.0);
  assert_eq!(
    read_number(&mut first, "__v8x_probe_completion_number")?,
    42.0
  );
  let retained_before = first.store.data().aot_call_graphs.len();
  for index in [9, 10, 3] {
    let nested = DenoRuntime::instantiate_script_in_context(
      &mut first.store,
      first.realm_instance,
      &shared,
      &scripts[index],
      None,
      true,
    )?;
    assert_eq!(nested.0, index != 3);
    if index == 9 {
      assert_eq!(nested.1, 0.0);
    } else {
      assert!(nested.1 > 0.0);
    }
    assert_eq!(
      read_number(&mut first, "__v8x_probe_completion_number")?,
      42.0
    );
  }
  // Native ownership dispatch still finds nested graphs after completion is
  // restored, including an initializer that threw after publishing values.
  assert_eq!(
    first.store.data().aot_call_graphs.len(),
    retained_before + 3
  );
  assert_eq!(first.instantiate_script(&shared, &scripts[9])?, (true, 0.0));
  let object_handle = first.instantiate_script(&shared, &scripts[10])?;
  assert!(object_handle.0 && object_handle.1 > 0.0);
  assert_eq!(read_bool(&mut first, "__v8x_probe_completion_identity")?, 1);
  assert_eq!(
    read_number(&mut second, "__v8x_script_completion_handle")?,
    0.0
  );
  for script in &scripts[..3] {
    second.instantiate_graph(&shared, script)?;
  }
  assert_eq!(read_number(&mut second, "__v8x_probe_script_score")?, 1.0);
  assert_eq!(
    read_number(&mut second, "__v8x_probe_script_observed")?,
    42.0
  );
  let stats = shared.stats()?;
  assert_eq!(stats.runtime_eval_instantiations, 0);
  assert_eq!(stats.compilations, 0);
  Ok(())
}
