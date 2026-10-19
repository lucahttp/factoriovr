# XFactorVR — Factorio Tabletop Mixed Reality for Pico 4 Ultra

A port of **Factorio** to Mixed Reality (Tabletop / 3D Diorama) designed to run **standalone** on **Pico 4 Ultra** (Qualcomm Snapdragon XR2 Gen 2, PICO OS / Android 14).

The factory is projected onto your real table as an interactive 3D diorama with color stereo passthrough, support for `.glb` 3D models and hardware rescaling via **Snapdragon Ga[...]

---

## Main Features

* **Tabletop / Diorama Mode (Generals Zero Hour XR style)**: Projects the Factorio factory fixed to the ground in `XR_REFERENCE_SPACE_TYPE_STAGE` space at the exact table height [...]
* **Geological Plinth and Edge Clipping (`u_board_bounds`)**: The terrain sits on a plinth with soil/rock strata and a beveled frame, neatly clipping geometry so it does not overflow [...]
* **High-Resolution Factorio Textures**: Industrial concrete tiles with expansion joints and anchor bolts, 45° hazard stripes, conveyor belts a[...]
* **Full 3D Machinery (`factorio-3d-mod`)**: Assembling machines with rotating gears and a steam chimney, miners with rotating shafts and deposits, steel furnaces with fire a[...]
* **Holographic Space Command Console (`XrCommandUI`)**: Floating glass control panel decoupled from the table with 5 interactive buttons (`[OBSERVADOR 1:1]`, `[RE-ANCLAR]`, `[ENERGY]`, `[ITEMS]`, [...])
* **6DoF Table Manipulation (`XrSurfaceGrab`)**: Grab and move the diorama around the room by holding the Grip button; adjust millimeter height and orientation with the joystick.
* **Human Observer 1:1 Scale Mode (`XrWorld::observer`)**: Switch between a tactical diorama perspective and the engineer’s view walking directly on the factory concrete at a 1:1 scale with n[...]

---

## Project Structure

* [`src/symbols.rs`](src/symbols.rs): ELF in-memory symbol scanner and resolver (uses the `.symtab` table from an unstripped Linux binary without requiring `.pdb` files).
* [`src/offsets.rs`](src/offsets.rs): Definitions of Factorio symbols and offsets under the Itanium ABI for GCC/Clang (`GameRenderer::render`, `AssemblingMachine::draw`, `RenderParameters::centerOn`, [...])
* [`src/tabletop.rs`](src/tabletop.rs): Mathematics for the table volume, spatial anchoring in meters, `.glb` model loader for `factorio-3d-mod` with texture sampling and 6DoF controller raycast [...]
* [`src/lib.rs`](src/lib.rs): Entry points `_init`, `xfactorvr_init`, `xfactorvr_load_glb` and `xfactorvr_free_glb` (C FFI).
* [`src/renderer/`](src/renderer/): OpenGL ES 3.2 graphics pipeline:
  * [`warp.rs`](src/renderer/warp.rs): Original 2D ground warping over the 3D plane with *sharp-bilinear* filtering.
  * [`model.rs`](src/renderer/model.rs): GLTF model rendering with massive GPU instancing.
  * [`sgsr.rs`](src/renderer/sgsr.rs): Qualcomm Snapdragon super-resolution module for Adreno 740.
* [`src/openxr/`](src/openxr/): OpenXR stereoscopic session, dual-eye matrices, color passthrough control and Pico controller action mapping.
* [`shaders/`](shaders/): GLSL ES 3.20 shaders (`warp.vert/frag`, `model.vert/frag`, `sky.vert/frag`, and `sgsr/`).
* [`android/src/openxr_engine.cpp`](android/src/openxr_engine.cpp): Native C++ OpenXR engine based on Generals Zero Hour XR with STAGE space world-locking support, GLB model loading, console[...]
* [`android/`](android/): Native app container (`AndroidManifest.xml` with Pico VR/MR intents and `launcher.cpp`).
* [`scripts/`](scripts/):
  * [`build_apk.sh`](scripts/build_apk.sh): Builds the Rust library for `aarch64-linux-android` with the NDK and creates the signed APK.
  * [`deploy_pico.sh`](scripts/deploy_pico.sh): Uploads game assets and transfers the APK to the headset via ADB.

---

## References

This project uses and is inspired by the following repositories as references:

- [GDani31/factorio-3d-mod](https://github.com/GDani31/factorio-3d-mod/tree/main) — 3D models and assets used to represent machinery and game elements.
- [Cesarus85/Generals-Zero-Hour-XR](https://github.com/Cesarus85/Generals-Zero-Hour-XR) — reference for the OpenXR engine and app architecture (action mapping, STAGE support, holographic UI ideas).

---

## Controls on Pico 4 Ultra

| Action | Pico Controller | Function in Factorio XR |
|---|---|---|
| **Laser Pointer & Reticle** | Right controller (6DoF Aim) | Point to factory tiles or holographic console buttons |
| **Index Trigger** | Press / Click | Select / Interact with spatial UI buttons |
| **Side Trigger (Grip)** | Hold (`XrSurfaceGrab`) | Grab and drag the table to reposition it anywhere in your room |
| **Joystick (Diorama Mode)** | Up / Down | Fine height adjustment of the table (±4 mm per step) |
| **Joystick (Diorama Mode)** | Left / Right | Rotate the table orientation around its vertical axis (Yaw) |
| **Joystick (Observer Mode)** | 2D Stick | Walk movement at 1:1 scale across the factory floor |
| **Holographic Console** | [OBSERVADOR 1:1] Button | Toggle between tactical diorama view and 1:1 floor view |

---

## Build and Deploy

### 1. Build the Standalone APK
Generate the signed package ready to install:
```bash
bash scripts/build_apk.sh
```
The final file is produced at:
`build/XFactorVR_Pico4Ultra.apk`

### 2. Deploy to Pico 4 Ultra via ADB
Connect the headset via USB-C or Wi‑Fi (make sure Developer Mode and USB Debugging are enabled):
```bash
bash scripts/deploy_pico.sh
```
The script will install the APK, set full storage permissions (`MANAGE_EXTERNAL_STORAGE`), transfer the folders `data/base`, `data/core` and `models` to internal storage (`/sdc[...]`).

To view logs in real time:
```bash
bash scripts/deploy_pico.sh --stream
```

### 3. Download the native Linux ARM64 binary (FFF #446)
Wube officially distributes the native Linux ARM64 binary through the Steam Experimental branch (`v2.1+`). To download it directly from the Steam console:
```text
download_depot 427520 427523 1410136920089685874
```
This downloads the natively compiled ARM64 (`aarch64`) binary, allowing the Factorio simulation to run directly on the Snapdragon XR2 Gen 2 with optimal performance.

---

## Testing and Validation

To run unit tests for table math, stereoscopic projection and symbol scanning of the real binary:
```bash
cargo test
```
