# shaders/ — El shader PBR de referencia (GLSL de escritorio)

Aquí queda **una sola cosa**: el shader PBR completo de
**Generals-Zero-Hour-XR** (`model.vert` / `model.frag`), con instancing, IK,
sombras y materiales. Es la referencia de lo que se puede hacer.

## Lo que usa el visor de verdad

Los shaders que el visor ejecuta están en
**`android/src/main/assets/shaders/`**:

| Fichero | Qué es |
|---|---|
| `modelo_visor.vert` / `.frag` | PBR con el color horneado en el vértice (la versión en **GLES 3.2**, la que entiende el Adreno de la Pico). Sin UBO ni esqueleto: el modelo se pinta con el color del vértice y una matriz por instancia. |
| `fullscreen.vert` | El quad de pantalla completa (SGSR). |
| `sgsr/sgsr1.frag` | El SGSR de Qualcomm, adaptado a GLES 3.2 (el original usa `textureGather`, que en ES 3.0 no existe). |

El motor los carga desde ahí con `src/renderer/shader.rs` (si no están en el
APK, usa una copia embebida).

## Lo que se ha descartado (y por qué)

- **`warp.vert` / `warp.frag`** (deformación del render 2D sobre el plano 3D):
  **descartado**. Son D3D11 (GLSL de escritorio con `StructuredBuffer` y
  `layout(set=…)`) y el motor en Android va con OpenGL ES 3.2: no hay
  equivalente. Además el warp solo tiene sentido si el motor dibuja en 3D, y
  aquí el 3D lo pone el **visor** (terreno real + modelos GLB de las
  entidades reales), no el motor.
- **`sky.vert` / `sky.frag`**: el cielo no se usa.
- **`sgsr/`** (la copia de escritorio): la version que funciona es la de
  `assets/shaders/sgsr/`.

## El siguiente paso natural

`model.vert` / `model.frag` de aquí es **más completo** que
`modelo_visor.vert` (trae UBO con todos los parametros del material, IK y
sombras). Adaptarlo a GLES 3.2 (UBO → uniforms, esqueleto → quaterniones por
instancia) es lo que haria que las maquinas se vean con material de verdad en
vez de con color plano.
