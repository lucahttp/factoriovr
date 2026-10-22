// Arnés TEMPORAL de pruebas del módulo `ui`.
//
// `src/lib.rs` es de otro agente, así que todavía NO declara `pub mod ui;`.
// Este fichero incluye el módulo directamente para poder ejecutar sus tests
// sin tocar `lib.rs`.
//
// Cuando el agente principal añada `pub mod ui;` a `src/lib.rs`, este arnés
// se puede BORRAR: los tests pasan a correr como tests de la librería.

// Aquí el módulo es privado, así que sus `pub use` cuentan como no usados.
#![allow(unused_imports, dead_code)]

#[path = "../src/ui/mod.rs"]
mod ui;
