fn main() {
  println!("cargo:rerun-if-changed=diagnostic-symbols.txt");
  println!(
    "cargo:rerun-if-changed=../../../src/js2wasm/diagnostic_abi_symbols.txt"
  );
  if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
    for symbol in
      include_str!("../../../src/js2wasm/diagnostic_abi_symbols.txt")
        .lines()
        .chain(include_str!("diagnostic-symbols.txt").lines())
    {
      if symbol.is_empty() || symbol.starts_with('#') {
        continue;
      }
      println!("cargo:rustc-link-arg=-Wl,-u,_{symbol}");
      println!("cargo:rustc-link-arg=-Wl,-exported_symbol,_{symbol}");
    }
  }
  let mut source = String::from("#include <stdio.h>\n#include <stdlib.h>\n");
  for symbol in include_str!("diagnostic-symbols.txt").lines() {
    assert!(
      symbol
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'_')
    );
    source.push_str(&format!(
            "__attribute__((weak,noreturn)) void {symbol}(void) {{ fprintf(stderr, \"v8x shared diagnostic: unresolved {symbol}\\n\"); abort(); }}\n"
        ));
  }
  let path = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap())
    .join("diagnostic.c");
  std::fs::write(&path, source).unwrap();
  cc::Build::new().file(path).compile("v8x_shared_diagnostic");
}
