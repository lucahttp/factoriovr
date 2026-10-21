// mod.rs — Mapeo de los mandos de la Pico a acciones de Factorio.
//
// NOTA DE ARQUITECTURA (ver ADR-010): la sesion OpenXR real la abre el
// motor en C++ (android/src/openxr_engine.cpp), que es quien tiene el
// contexto GL y los swapchains. Aqui no hay sesion: solo el mapeo de mandos
// a acciones y la decision de que hace cada mano, que es logica pura y se
// puede probar sin dispositivo.
//
// La sesion de Rust que hubo aqui (OpenXrSession, EyePose, el passthrough)
// era un esqueleto que nadie construia: el motor la dejo obsoleta.
pub mod actions;
