//! Define el cfg `elf_target` para los destinos con formato ELF (Linux y
//! Android). El codigo del inyector (mmap, mprotect, dl_iterate_phdr,
//! /proc/self/maps) solo tiene sentido en esos sistemas; en el host macOS se
//! compilan stubs para que `cargo build` siga sirviendo de comprobacion de
//! sintaxis.

fn main() {
    println!("cargo:rustc-check-cfg=cfg(elf_target)");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "linux" || target_os == "android" {
        println!("cargo:rustc-cfg=elf_target");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
