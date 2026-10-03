//! The engine (page scripts, parsing, layout) runs on the main thread. Windows gives the
//! main thread 1 MiB of stack where Linux and macOS give 8 MiB, and real pages need more
//! than 1 MiB; link Windows binaries with the same 8 MiB.

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

    let icon =
        std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("axiom.ico");
    write_axiom_icon(&icon).expect("write Axiom application icon");
    winres::WindowsResource::new()
        .set_icon(icon.to_str().expect("UTF-8 icon path"))
        .compile()
        .expect("compile Axiom Windows resources");
}

/// A small, original orbital-A mark. Keeping it generated makes the EXE icon reproducible
/// without checking opaque binary artwork into the source tree.
fn write_axiom_icon(path: &std::path::Path) -> std::io::Result<()> {
    const SIDE: usize = 32;
    let mut rgba = vec![0_u8; SIDE * SIDE * 4];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let dx = x as i32 - 15;
            let dy = y as i32 - 15;
            let d2 = dx * dx + dy * dy;
            let i = (y * SIDE + x) * 4;
            let ring = (72..=185).contains(&d2);
            let diagonal = (x as i32 - (15 - dy / 2)).abs() <= 2 && (7..=25).contains(&y);
            let crossbar = (14..=18).contains(&y) && (10..=21).contains(&x);
            if ring || diagonal || crossbar {
                rgba[i..i + 4].copy_from_slice(&[68, 122, 255, 255]);
            }
        }
    }
    let mut image = Vec::new();
    image.extend_from_slice(&40_u32.to_le_bytes());
    image.extend_from_slice(&(SIDE as i32).to_le_bytes());
    image.extend_from_slice(&((SIDE * 2) as i32).to_le_bytes());
    image.extend_from_slice(&1_u16.to_le_bytes());
    image.extend_from_slice(&32_u16.to_le_bytes());
    image.extend_from_slice(&0_u32.to_le_bytes());
    image.extend_from_slice(&((SIDE * SIDE * 4) as u32).to_le_bytes());
    image.extend_from_slice(&[0; 16]);
    for y in (0..SIDE).rev() {
        for x in 0..SIDE {
            let p = &rgba[(y * SIDE + x) * 4..][..4];
            image.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    image.extend(std::iter::repeat_n(0_u8, SIDE * 4));
    let mut ico = Vec::new();
    ico.extend_from_slice(&0_u16.to_le_bytes());
    ico.extend_from_slice(&1_u16.to_le_bytes());
    ico.extend_from_slice(&1_u16.to_le_bytes());
    ico.extend_from_slice(&[SIDE as u8, SIDE as u8, 0, 0]);
    ico.extend_from_slice(&1_u16.to_le_bytes());
    ico.extend_from_slice(&32_u16.to_le_bytes());
    ico.extend_from_slice(&(image.len() as u32).to_le_bytes());
    ico.extend_from_slice(&22_u32.to_le_bytes());
    ico.extend_from_slice(&image);
    std::fs::write(path, ico)
}
