//! Herramienta de diagnostico: parsea el ELF del motor y resuelve simbolos
//! por nombre, igual que hara el inyector en tiempo de ejecucion.
//!
//!     cargo run --example parse_elf -- <ruta al ELF> [nombre a resolver]
//!
//! Sirve para comprobar el des-mangler y el lector de .symtab sin necesidad
//! de arrancar el juego.

use std::path::PathBuf;

use xfactorvr_injector::demangle::demangle;
use xfactorvr_injector::elf::ElfImage;

fn main() {
    let mut args = std::env::args().skip(1);
    let ruta = match args.next() {
        Some(r) => PathBuf::from(r),
        None => {
            eprintln!("uso: parse_elf <ruta al ELF> [nombre a resolver]");
            std::process::exit(1);
        }
    };
    println!("leyendo {}", ruta.display());
    let imagen = match ElfImage::parse(&ruta) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("ERROR parseando el ELF: {e}");
            std::process::exit(1);
        }
    };
    println!("simbolos de funcion: {}", imagen.symbols.len());
    println!("primer PT_LOAD en vaddr 0x{:x}", imagen.first_load_vaddr());

    let consultas: Vec<String> = args.collect();

    for nombre in &consultas {
        println!("resolviendo '{nombre}'...");
        let partes: Vec<&str> = nombre.as_str().split("::").filter(|p| !p.is_empty()).collect();
        let mut encontrados = 0;
        for s in &imagen.symbols {
            if !partes.iter().all(|p| s.name.contains(p)) {
                continue;
            }
            match demangle(&s.name) {
                Some(d) => {
                    if d.qualified == *nombre {
                        println!("  0x{:x}  {}", s.value, d.full);
                        encontrados += 1;
                    }
                }
                None => {}
            }
        }
        if encontrados == 0 {
            println!("  (no encontrado)");
            // Listar los candidatos con el mismo prefijo.
            let pre = partes.first().copied().unwrap_or("");
            let mut cands: Vec<String> = Vec::new();
            for s in &imagen.symbols {
                if !s.name.contains(pre) {
                    continue;
                }
                if let Some(d) = demangle(&s.name) {
                    if d.qualified.starts_with(pre) && !cands.contains(&d.qualified) {
                        cands.push(d.qualified);
                    }
                }
            }
            cands.sort();
            for c in cands.iter().take(15) {
                println!("  candidato: {c}");
            }
        }
    }
}
