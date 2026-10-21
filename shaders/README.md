# shaders/ — Shaders de referencia (NO los usa el motor actual)

Estos ficheros GLSL ES 3.20 vienen de **Generals-Zero-Hour-XR** y son la
referencia para lo que le falta al motor:

- `warp.vert/frag`: deformación del suelo 2D sobre el plano 3D con filtrado
  *sharp-bilinear*.
- `model.vert/frag`: renderizado de los modelos 3D GLTF.
- `sky.vert/frag`: cielo.
- `sgsr/`: super-resolución de Qualcomm Snapdragon (SGSR) para el Adreno 740.

## El motor actual no los usa

El motor de VR es `android/src/openxr_engine.cpp` y lleva **sus propios
shaders en línea** (OpenXR, sprites, terreno, panel del render del juego). El
APK ni siquiera los empaqueta: `scripts/build_apk.sh` no los copia.

Hubo un pipeline gráfico en Rust (`src/renderer/`, con `warp.rs`, `model.rs` y
`sgsr.rs`) que usaba estos shaders, pero **nadie lo llamaba**: el motor en C++
lo dejó obsoleto y aquel código se borró en la auditoría de ADR-010.

## Para qué se conservan

El **SGSR está pendiente de portar** al motor C++: es la vía de reescalado por
software de la imagen del juego que se ve en el panel, y con ella se podría
subir la resolución de lo que se manda al visor. El material está aquí para
que el port no haya que buscarlo otra vez.

**No funcionan tal cual**: son shaders de escritorio (GLSL 3.20) y habría que
adaptarlos a GLES 3.20 y al encadenado del motor (que dibuja a un FBOs
intermedio, no a pantalla).
