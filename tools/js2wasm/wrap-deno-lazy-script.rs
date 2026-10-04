// Compile build-side against the pinned, unchanged deno_core library. Use
// Deno's actual public helper instead of duplicating its wrapping semantics.
fn main() {
  let path = std::env::args()
    .nth(1)
    .expect("usage: wrap-deno-lazy-script SOURCE_FILE");
  let source =
    std::fs::read_to_string(path).expect("read original lazy Script");
  print!("{}", deno_core::wrap_lazy_ext_script(&source));
}
