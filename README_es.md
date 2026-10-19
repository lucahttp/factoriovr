# XFactorVR — Factorio Tabletop Mixed Reality para Pico 4 Ultra

Port de **Factorio** en Realidad Mixta (Tabletop / Maqueta 3D) diseñado para ejecutarse de forma **Standalone** en **Pico 4 Ultra** (Qualcomm Snapdragon XR2 Gen 2, PICO OS / Android 14).

La fábrica se proyecta sobre tu mesa real como un diorama tridimensional interactivo con Passthrough estéreo a color, soporte para modelos 3D `.glb` y reescalado de hardware mediante **Snapdragon[...]

---

## Características Principales

* **Modo Maqueta / Tabletop Diorama (Estilo Generals Zero Hour XR)**: Proyecta la fábrica de Factorio anclada de forma fija al suelo en espacio `XR_REFERENCE_SPACE_TYPE_STAGE` a la altura exacta del escritorio físico (~0.74m), eliminando la flotación frente a la cara.
* **Persistencia del Anclaje de Mesa (`layout.json`)**: Guarda automáticamente la posición, orientación yaw y escala de la maqueta en `/sdcard/XFactorVR/layout.json` al soltar el agarre y la restaura exactamente en el siguiente inicio.
* **Interacción Bimanual (`XrInteraction`)**: Pellizcar para hacer zoom (Pinch-to-zoom de 0.25x a 4.0x) y rotación libre sobre el eje Y sujetando ambos gatillos Grip de los mandos Pico simultáneamente.
* **Yacimientos Reales de Recursos (Hierro, Cobre, Carbón, Piedra)**: Vetás minerales tridimensionales con cristales emergentes sobre el terreno; perforadoras extrayendo mineral directamente del suelo.
* **Simulación en Vivo y Puente Bidireccional (`xfactorvr_bridge`)**: Conexión RCON/IPC ultrarrápida con el servidor nativo Linux ARM64 y motor de automatización integrado en Rust (perforadoras, hornos fundiendo mineral con combustible, ensambladoras fabricando piezas).
* **Consola Holográfica Espacial Multi-Pestaña (`XrCommandUI`)**:
  * **Pestaña Construir**: Quickbar de 9 ranuras para edificios industriales.
  * **Pestaña Inventario**: Inspección del inventario real del jugador con barras medidoras de stock.
  * **Pestaña Fabricar**: Banco de trabajo interactivo para craftear Engranajes, Circuitos Electrónicos, Cintas Transportadoras y Perforadoras con un clic de gatillo.
* **Tooltips Holográficos 3D en Mandos (`XrControllerHelp`)**: Badge HUD flotante sobre la muñeca del mando derecho con indicaciones codificadas por color (Trigger: Acción, Grip: Mover/Demoler, Stick: Rotar/Altura, 2-Grips: Zoom/Giro).
* **Audio Espacial 3D Posicional (`XrAudio`)**: Síntesis estéreo nativa a 48000 Hz mediante AAudio con zumbido industrial continuo de fábrica, atenuación por distancia y paneo espacial dependiente de la pose de la cabeza, además de efectos acústicos para colocación, minado, crafteo y pulsaciones de interfaz.
* **Feedback Háptico en Mandos (`XrHapticVibration`)**: Vibraciones táctiles sutiles en pulsaciones de menú (25ms), impactos al asentar maquinaria (45ms) y sacudidas firmes al desmantelar instalaciones (65ms).
* **Construcción y Deconstrucción 6DoF**: Rejilla magnética, previsualización fantasma holográfica translúcida (verde para libre, rojo para ocupado) y consumo/devolución real de ítems al inventario.
* **Animaciones de Maquinaria y Partículas 3D (`factorio-3d-mod`)**:
  * Antena de radar con rotación continua de 360°.
  * Torreta defensiva con rotación y seguimiento independiente del cabezal superior.
  * Perforadora minera con pistón/broca vertical reciprocante.
  * Robopuerto con drones de patrulla aérea orbitando en 3D.
  * Máquina de ensamblaje con baliza titilante en la chimenea y bocanadas de vapor.
  * Postes de tendido eléctrico con cables catenarios tridimensionales autogenerados entre postes contiguos.
  * Cintas transportadoras con desplazamiento UV animado e ítems en tránsito.
  * Sistema de partículas 3D para humo, vapor, chispas de colocación y escombros de minado.
* **Modo Observador Humano a Escala 1:1 (`XrWorld::observer`)**: Cambia entre la perspectiva de diorama táctico y la vista del ingeniero caminando directamente sobre el hormigón de la fábrica con navegación fluida mediante el stick analógico.

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
| **Puntero Láser & Retículo** | Mando derecho (6DoF Aim) | Apunta a casillas de la fábrica, Quickbar o botones de la consola |
| **Gatillo Índice (Trigger)** | Clic sobre ranura Quickbar | Selecciona la herramienta/entidad activa a construir |
| **Gatillo Índice (Trigger)** | Clic sobre casilla vacía | Construir entidad seleccionada (Ghost verde) |
| **Gatillo Lateral (Grip)** | Clic sobre entidad existente | Minar / Deconstruir entidad apuntada |
| **Gatillo Índice (Trigger)** | Clic con herramienta Demoler | Minar / Deconstruir entidad apuntada |
| **Joystick X (Thumbstick)** | Izquierda / Derecha | Rotar orientación de colocación de la entidad en 90° |
| **Gatillo Lateral (Grip)** | Mantener sobre borde mesa | Agarrar y arrastrar la mesa (`XrSurfaceGrab`) para reubicarla |
| **Joystick Y (Modo Mesa)** | Arriba / Abajo | Ajuste fino de altura de la mesa ($\pm4\text{mm}$ por paso) |
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
