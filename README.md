# FactorioVR — Factorio Tabletop Mixed Reality for Pico 4 Ultra

**Factorio** Mixed Reality port (Tabletop / 3D Diorama) designed to run **Standalone** on **Pico 4 Ultra** (Qualcomm Snapdragon XR2 Gen 2, PICO OS / Android 14).

The factory is projected onto your real table as an interactive three-dimensional diorama with full-color stereo passthrough, support for `.glb` 3D models, and hardware upscaling via **Snapdragon Game Super Resolution (Qualcomm SGSR)**.

---

## Main Features

* **Tabletop Diorama Mode (Generals Zero Hour XR Style)**: Projects the Factorio factory fixed-anchor to the floor in `XR_REFERENCE_SPACE_TYPE_STAGE` space at the exact height of the physical desk (~0.74m), eliminating floating in front of your face.
* **Geological Plinth and Edge Clipping (`u_board_bounds`)**: The terrain sits on a socket with earth/rock strata and a bevelled frame, cleanly clipping geometry so it doesn't spill over into the physical room.
* **High-Resolution Factorio Textures**: Industrial concrete slabs with expansion joints and anchor bolts, 45° hazard stripes, animated conveyor belts with rollers and chevron arrows, and industrial metallic sheen.
* **Complete 3D Machinery (`factorio-3d-mod`)**: Assembly machines 2 with rotating gears and steam chimney, mining drills with rotating stem and ore deposit, steel furnaces with blazing fire, radar station with 360° satellite dish, roboports with patrol drones, defense turrets, power poles with catenary cables, and yellow and blue robotic inserters with articulated claws.
* **Holographic Spatial Command Console (`XrCommandUI`)**: Floating glass panel decoupled from the table with 5 interactive buttons (`[1:1 OBSERVER]`, `[RE-ANCHOR]`, `[POWER]`, `[ITEMS]`, `[TECHNOLOGY]`), aimable with a laser pointer and trigger-activatable.
* **6DoF Table Manipulation (`XrSurfaceGrab`)**: Grab and move the diorama around the room by holding the Grip button; adjust millimeter height and orientation with the joystick.
* **1:1 Scale Human Observer Mode (`XrWorld::observer`)**: Switch between tactical diorama perspective and the engineer's view walking directly on the factory concrete with smooth analog stick navigation.

---

## Project Structure

* [`src/symbols.rs`](src/symbols.rs): In-memory ELF symbol scanner and resolver (uses the `.symtab` table from the unstripped Linux binary without requiring `.pdb` files).
* [`src/offsets.rs`](src/offsets.rs): Factorio symbols and offset definitions under GCC/Clang Itanium ABI (`GameRenderer::render`, `AssemblingMachine::draw`, `RenderParameters::centerOn`, etc.).
* [`src/tabletop.rs`](src/tabletop.rs): Table volume mathematics, spatial anchoring in meters, `factorio-3d-mod` `.glb` model loader with texture sampling and 6DoF command raycast to `(X, Y)` cells.
* [`src/lib.rs`](src/lib.rs): Entry points `_init`, `xfactorvr_init`, `xfactorvr_load_glb`, and `xfactorvr_free_glb` (C FFI).
* [`src/renderer/`](src/renderer/): OpenGL ES 3.2 graphics pipeline:
* [`warp.rs`](src/renderer/warp.rs): Original 2D ground deformation onto the 3D plane with sharp-bilinear filtering.
* [`model.rs`](src/renderer/model.rs): GLTF 3D model rendering with massive GPU instancing.
* [`sgsr.rs`](src/renderer/sgsr.rs): Qualcomm Snapdragon super-resolution module for Adreno 740.


* [`src/openxr/`](src/openxr/): OpenXR stereoscopic session, dual eye matrices, color passthrough control, and Pico controller action mapping.
* [`shaders/`](shaders/): GLSL ES 3.20 shaders (`warp.vert/frag`, `model.vert/frag`, `sky.vert/frag`, and `sgsr/`).
* [`android/src/openxr_engine.cpp`](android/src/openxr_engine.cpp): Native C++ OpenXR engine based on Generals Zero Hour XR with STAGE space world-locking support, GLB model loading, spatial UI console, and 90 FPS stereo rendering.
* [`android/`](android/): Native app container (`AndroidManifest.xml` with PICO VR/MR intents and `launcher.cpp` launcher).
* [`scripts/`](scripts/):
* [`build_apk.sh`](scripts/build_apk.sh): Compiles the Rust library for `aarch64-linux-android` using the NDK and generates the signed APK.
* [`deploy_pico.sh`](scripts/deploy_pico.sh): Uploads game assets and transfers the APK to the headset via ADB.



---

## References

This project uses and is inspired by the following repositories as reference:

* [GDani31/factorio-3d-mod](https://github.com/GDani31/factorio-3d-mod/tree/main) — 3D models and assets used to represent machinery and game elements.
* [Cesarus85/Generals-Zero-Hour-XR](https://github.com/Cesarus85/Generals-Zero-Hour-XR) — reference for the OpenXR engine and application architecture (action mapping, STAGE support, holographic UI ideas).

---

## Pico 4 Ultra Controls

| Action | Pico Controller | Factorio XR Function |
| --- | --- | --- |
| **Laser Pointer & Reticle** | Right Controller (6DoF Aim) | Aims at factory cells or holographic console buttons |
| **Index Trigger** | Press / Click | Select / Interact with spatial interface buttons |
| **Grip Button** | Hold Down (`XrSurfaceGrab`) | Grab and drag the table to reposition it freely in your room |
| **Joystick (Diorama Mode)** | Up / Down | Fine adjustment of table height ($\pm4\text{mm}$ per step) |
| **Joystick (Diorama Mode)** | Left / Right | Rotate table orientation on its vertical axis (Yaw) |
| **Joystick (Observer Mode)** | 2D Stick | Foot movement at 1:1 scale across the factory floor |
| **Holographic Console** | [1:1 OBSERVER] Button | Toggle between tactical diorama view and 1:1 floor view |

---

## Build and Deployment

### 1. Build the Standalone APK

Generates the ready-to-install signed package:

```bash
bash scripts/build_apk.sh

```

The final file is generated at:
`build/XFactorVR_Pico4Ultra.apk`

### 2. Deploy to Pico 4 Ultra via ADB

Connect the headset via USB-C cable or Wi-Fi (make sure Developer Mode and USB Debugging are enabled):

```bash
bash scripts/deploy_pico.sh

```

The script will install the APK, configure full storage permissions (`MANAGE_EXTERNAL_STORAGE`), transfer `data/base`, `data/core`, and `models` folders to internal memory (`/sdcard/XFactorVR`), and launch the application automatically.

To view real-time logs:

```bash
bash scripts/deploy_pico.sh --stream

```

### 3. Linux ARM64 Native Binary Download (FFF #446)

Wube officially distributes the Linux ARM64 native binary via the Steam Experimental branch (`v2.1+`). To download it directly from the Steam console:

```text
download_depot 427520 427523 1410136920089685874

```

This downloads the binary compiled natively for the ARM64 architecture (`aarch64`), allowing the Factorio simulation to run directly on the Snapdragon XR2 Gen 2 with optimal performance.

---

## Testing and Validation

To run unit tests for table mathematics, stereoscopic projection, and real binary symbol scanning:

```bash
cargo test

```
