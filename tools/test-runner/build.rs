//! The engine runs on the main thread. Windows gives the main thread 1 MiB of stack where
//! Linux and macOS give 8 MiB, and real pages need more than 1 MiB; link Windows binaries
//! with the same 8 MiB.

fn main() {
    const STACK: u32 = 8 * 1024 * 1024;
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:{STACK}"),
        Ok("gnu") => println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK}"),
        _ => {}
    }
}
