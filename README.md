# XFactorVR — Factorio Tabletop Mixed Reality para Pico 4 Ultra

Port de **Factorio** en Realidad Mixta (Tabletop / Maqueta 3D) diseñado para ejecutarse de forma **Standalone** en **Pico 4 Ultra** (Qualcomm Snapdragon XR2 Gen 2, PICO OS / Android 14).

La fábrica se proyecta sobre tu mesa real como un diorama tridimensional interactivo con Passthrough estéreo a color, soporte para modelos 3D `.glb` y reescalado de hardware mediante **Snapdragon[...]

---

## Características Principales

* **Modo Maqueta / Tabletop Diorama (Estilo Generals Zero Hour XR)**: Proyecta la fábrica de Factorio anclada de forma fija al suelo en espacio `XR_REFERENCE_SPACE_TYPE_STAGE` a la altura exacta [...]
* **Plinto Geológico y Recorte de Borde (`u_board_bounds`)**: El terreno se asienta sobre un zócalo con estratos de tierra/roca y marco biselado, recortando limpiamente la geometría para no des[...]
* **Texturas de Alta Resolución Factorio**: Losas de concreto industrial con juntas de dilatación y pernos de anclaje, franjas diagonales de peligro (hazard stripes a 45°), cintas transportadoras a[...]
* **Maquinaria 3D Completa (`factorio-3d-mod`)**: Máquinas de ensamblaje 2 con engranajes giratorios y chimenea de vapor, perforadoras con vástago rotatorio y yacimiento, hornos de acero con fue[...]
* **Consola de Mando Espacial Holográfica (`XrCommandUI`)**: Panel flotante de cristal desacoplado de la mesa con 5 botones interactivos (`[OBSERVADOR 1:1]`, `[RE-ANCLAR]`, `[ENERGÍA]`, `[ÍTEMS[...]
* **Manipulación de Mesa 6DoF (`XrSurfaceGrab`)**: Agarra y traslada la maqueta por la habitación manteniendo presionado el botón Grip; ajusta la altura milimétrica y orientación con el joyst[...]
* **Modo Observador Humano a Escala 1:1 (`XrWorld::observer`)**: Cambia entre la perspectiva de diorama táctico y la vista del ingeniero caminando directamente sobre el hormigón de la fábrica c[...]

---

## Estructura del Proyecto

* [`src/symbols.rs`](src/symbols.rs): Escáner y resolvedor de símbolos ELF en memoria (utiliza la tabla `.symtab` del binario no-strippeado de Linux sin requerir archivos `.pdb`).
* [`src/offsets.rs`](src/offsets.rs): Definiciones de símbolos y offsets de Factorio bajo la ABI Itanium de GCC/Clang (`GameRenderer::render`, `AssemblingMachine::draw`, `RenderParameters::center[...]
* [`src/tabletop.rs`](src/tabletop.rs): Matemáticas del volumen de mesa, anclaje espacial en metros, cargador de modelos `.glb` de `factorio-3d-mod` con muestreo de texturas y raycast de mando 6D[...]
* [`src/lib.rs`](src/lib.rs): Puntos de entrada `_init`, `xfactorvr_init`, `xfactorvr_load_glb` y `xfactorvr_free_glb` (C FFI).
* [`src/renderer/`](src/renderer/): Pipeline gráfico OpenGL ES 3.2:
  * [`warp.rs`](src/renderer/warp.rs): Deformación del suelo 2D original sobre el plano 3D con filtrado *sharp-bilinear*.
  * [`model.rs`](src/renderer/model.rs): Renderizado de modelos 3D GLTF con instanciación masiva en GPU.
  * [`sgsr.rs`](src/renderer/sgsr.rs): Módulo de super-resolución de Qualcomm Snapdragon para Adreno 740.
* [`src/openxr/`](src/openxr/): Sesión estereoscópica OpenXR, matrices oculares duales, control de Passthrough a color y mapeo de acciones de mandos Pico.
* [`shaders/`](shaders/): Shaders GLSL ES 3.20 (`warp.vert/frag`, `model.vert/frag`, `sky.vert/frag`, y `sgsr/`).
* [`android/src/openxr_engine.cpp`](android/src/openxr_engine.cpp): Motor OpenXR nativo en C++ basado en Generals Zero Hour XR con soporte para STAGE space world-locking, carga de modelos GLB, con[...]
* [`android/`](android/): Contenedor de app nativa (`AndroidManifest.xml` con intents VR/MR de PICO y lanzador `launcher.cpp`).
* [`scripts/`](scripts/):
  * [`build_apk.sh`](scripts/build_apk.sh): Compila la librería Rust para `aarch64-linux-android` con el NDK y genera el APK firmado.
  * [`deploy_pico.sh`](scripts/deploy_pico.sh): Sube los assets de juego y transfiere el APK al visor mediante ADB.

---

## Referencias

Este proyecto utiliza y se inspira en los siguientes repositorios como referencia:

- [GDani31/factorio-3d-mod](https://github.com/GDani31/factorio-3d-mod/tree/main) — modelos 3D y assets utilizados para representar la maquinaria y elementos del juego.
- [Cesarus85/Generals-Zero-Hour-XR](https://github.com/Cesarus85/Generals-Zero-Hour-XR) — referencia para el motor OpenXR y la arquitectura de aplicación (mapeo de acciones, soporte STAGE, ideas de UI holográfica).

---

## Controles en Pico 4 Ultra

| Acción | Mando Pico | Función en Factorio XR |
|---|---|---|
| **Puntero Láser & Retículo** | Mando derecho (6DoF Aim) | Apunta a casillas de la fábrica o botones de la consola holográfica |
| **Gatillo Índice (Trigger)** | Presionar / Clic | Seleccionar / Interactuar con botones de la interfaz espacial |
| **Gatillo Lateral (Grip)** | Mantener presionado (`XrSurfaceGrab`) | Agarrar y arrastrar la mesa para reubicarla libremente en tu habitación |
| **Joystick (Modo Diorama)** | Arriba / Abajo | Ajuste fino de altura de la mesa ($\pm4\text{mm}$ por paso) |
| **Joystick (Modo Diorama)** | Izquierda / Derecha | Rotar la orientación de la mesa sobre su eje vertical (Yaw) |
| **Joystick (Modo Observador)** | 2D Stick | Desplazamiento a pie en escala 1:1 por el suelo de la fábrica |
| **Consola Holográfica** | Botón [OBSERVADOR 1:1] | Alternar entre vista de maqueta táctica y vista de suelo en 1:1 |

---

## Compilación y Despliegue

### 1. Compilar el APK Standalone
Genera el paquete firmado listo para instalar:
```bash
bash scripts/build_apk.sh
```
El archivo final se genera en:
`build/XFactorVR_Pico4Ultra.apk`

### 2. Desplegar en la Pico 4 Ultra vía ADB
Conecta el visor mediante cable USB-C o Wi-Fi (asegúrate de tener el Modo Desarrollador y Depuración USB activados):
```bash
bash scripts/deploy_pico.sh
```
El script instalará el APK, configurará los permisos de almacenamiento completo (`MANAGE_EXTERNAL_STORAGE`), transferirá las carpetas `data/base`, `data/core` y `models` a la memoria interna (`[...]

Para ver los logs en tiempo real:
```bash
bash scripts/deploy_pico.sh --stream
```

### 3. Descarga del binario nativo Linux ARM64 (FFF #446)
Wube distribuye oficialmente el binario nativo Linux ARM64 a través de la rama Experimental de Steam (`v2.1+`). Para descargarlo directamente desde la consola de Steam:
```text
download_depot 427520 427523 1410136920089685874
```
Esto descarga el binario compilado nativamente para arquitectura ARM64 (`aarch64`), permitiendo ejecutar la simulación de Factorio directamente en el Snapdragon XR2 Gen 2 con rendimiento óptimo.

---

## Pruebas y Validación

Para ejecutar los tests unitarios de matemáticas de mesa, proyección estereoscópica y escaneo de símbolos del binario real:
```bash
cargo test
```
