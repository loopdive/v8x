//! Experimental compiler-free shared runtime for the Deno core probe.
//!
//! Rust dylibs require the consumer and library to use the same Rust toolchain
//! and compatible dependency graph. This is not a stable plugin ABI.
pub use runtime::*;
