# ADR-010 — Auditoría de la arquitectura: qué dibuja cada capa

## Por qué

El objetivo del proyecto pide "revisar bien el juego para hacerlo de la mejor
manera posible, porque parece que solo estás corriendo el backend y
renderizando tú todo". Esa frase era una crítica **justa** al principio del
proyecto: el visor dibujaba una reconstrucción del mundo con sprites.

Ahora el juego **sí** dibuja (su framebuffer viaja al visor por el protocolo
XFV1, ver ADR-009), pero al revisar el repo con `codebase-memory-mcp` (índice
de 6.493 nodos) salió otra cosa: **el repo describía una arquitectura que ya no
era la de verdad**, y tenía ~1.000 líneas de código muerto.

## Lo que se encontró

El grafo de llamadas showed que:

| Capa | Estado real |
|---|---|
| `src/renderer/` (465 líneas: `warp.rs`, `model.rs`, `sgsr.rs`, `shader.rs`, `mod.rs`) | **Código muerto**: su único punto de entrada público (`TabletopRenderer::render_eye`) no lo llama nadie. El módulo estaba declarado en `lib.rs` y nada más. |
| `src/openxr/mod.rs` — `OpenXrSession`, `EyePose`, `passthrough.rs` | **Código muerto**: un esqueleto de sesión OpenXR en Rust que solo se construía en su propio test. La sesión real la abre el motor en C++. |
| `shaders/*.vert|frag` (380 líneas) | **No se usan**: el motor lleva sus shaders en línea dentro del C++ y el APK no los empaqueta. Son la referencia de Generals-Zero-Hour-XR. |
| `glow` (dependencia de crates.io) | Sin usar: solo la usaba el renderer muerto. |
| `src/assets.rs`, `src/terreno.rs`, `src/game_bridge.rs`, `src/tabletop.rs`, `src/openxr/actions.rs`, `src/lib.rs` | **Vivos**: son los que el motor C++ llama por el FFI (`xfactorvr_*`). |

O sea: **todo el dibujado ocurre en `android/src/openxr_engine.cpp`** (sesión
OpenXR, contexto GLES, shaders, sprites, terreno, panel del render del juego y
las acciones de las manos). Rust hace de capa de datos y lógica: atlas,
terreno, puente RCON, matemáticas de la mesa, geometría de los `.glb` y la
decisión de qué hace cada mano.

## Qué se hizo

1. **Borrado el código muerto**: `src/renderer/` y `src/openxr/passthrough.rs`,
   y `pub mod renderer;` de `lib.rs`. El test que probaba el mapeo de mandos
   (que vivía en `openxr/mod.rs`) se trasladó a `openxr/actions.rs`, que es
   donde ahora está ese código. Se elimina la dependencia `glow`.
2. **README corregido** (los dos idiomas): la sección "Estructura del
   Proyecto" describía el pipeline gráfico en Rust y anunciaba el reescalado
   SGSR como una característica activa. Ahora dice la verdad: quién dibuja
   (C++), qué hace Rust (datos y lógica), que los shaders de `shaders/` son
   referencia y que **el SGSR está pendiente de portar**.
3. **ADR-009 y ADR-008** actualizados con lo que ha pasado después.

## Por qué no se porta el SGSR ahora

Los shaders de SGSR (super-resolución de Snapdragon) son de GZH-XR y están en
`shaders/sgsr/`. Portarlos al motor C++ (que dibuja en GLES con shaders en
línea) es trabajo de verdad, y **sin el visor delante no se puede comprobar
que mejora algo**: el reescalado se juzga en la imagen. Se deja el material
listo y documentado, para hacerlo cuando haya hardware delante.

## Cómo se verifica

- `cargo test`: 35/35 (eran 36: el que se va es el de las matrices de
  proyección de la sesión OpenXR muerta; el motor C++ hace la misma
  comprobación en `selfCheckMath()` al arrancar).
- `bash scripts/build_apk.sh`: el APK compila (el FFI no ha cambiado).
- Los bancos (`test_jugador_virtual.sh`, `test_inventario_vr.sh`,
  `test_stream_render.sh`, `test_modelo3d.sh`) no cambian: no tocaban el
  renderer.

## Consecuencia para el objetivo

El juego **corre y dibuja**; el visor lo muestra. Lo que el visor sigue
dibujando por su cuenta es el **diorama** (la vista de mesa con sprites y
modelos 3D), que es una capa propia y deliberada: es lo que permite colocar
máquinas y ver la fábrica en 3D mientras el panel del fondo muestra la
imagen real del juego.
