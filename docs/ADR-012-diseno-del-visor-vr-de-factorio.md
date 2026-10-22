# ADR-012 — El visor VR de Factorio: diseño objetivo (fidelidad al juego + UX)

> **Estado**: en curso. Este documento es la referencia que reparten los
> trabajos: define QUÉ se quiere y QUÉ ficheros toca cada uno.
>
> Origen: el usuario pidió que el visor se parezca lo máximo posible al juego
> original y que la jugabilidad no sea "mortal", tomando como referencia de
> diseño el proyecto **Generals-Zero-Hour-XR** (Command & Conquer: Zero Hour
> XR) y usando los **modelos 3D del mod factorio-3d-mod** de GDani31.
>
> **Progreso**: T1 (modelos 3D) y T2 (HUD) entregados y validados; T3 (el
> modelo 3D tiene prioridad) y T4 (gesto diferido) implementados. Falta la
> validación visual en la Pico (bloqueada por el cable USB).

---

## 1. Qué hace GZH-XR y qué lessons nos llevas

`Generals-Zero-Hour-XR/` es un juego 3D (C&C) running en MR. Sus ficheros XR
revelan un diseño muy consciente de la **UX en VR**, no solo del render:

| Fichero | Qué resuelve |
|---|---|
| `XrScene.h` | Colocación del tablero sobre la **mesa real** (Meta Scene API), con auto-ajuste de escala por bisección. |
| `XrSurface.h` / `XrSurfaceGrab` | La superficie (tablero) se **agarra con una o dos manos**: moverla y escalarla con la distancia entre las manos. |
| `XrReferenceSpace.h` | Cuando OpenXR re-localiza, **compensa las poses** de tablero, ancla y menús (nunca usa la altura de la cabeza). |
| `XrWorld.h` | El juego se renderiza con la **cámara del ojo** (2 ojos, 2 FOV) y se proyecta sobre el tablero; incluye un **modo observador a escala humana** (10 unidades/m, ojos a 1,65 m). |
| `XrPanelLayout.h` | **Una sola tabla de controles** define a la vez los píxeles, el hit-test y el estado: "Java renders primitives from this table only; it owns no geometry, so pixels and hit regions cannot drift apart". |
| `XrTactics.h` | **Gesto diferido**: al pulsar el gatillo NO se emite la orden; se arma y se emite al soltar o al arrastrar. Con **pista textual** del modo actual. |
| `XrSceneUI.h` | Colocación en **3 pasos** (modo → superficie → listo) con **vista previa**, confirmar/cancelar, y estado "armed" para no aceptar input por error. |
| `XrMetaKeyboard.h` | Teclado nativo del sistema para escribir texto. |

**Las cuatro ideas que nos vamos a llevar** (las que cambian la sensación de
jugar):

1. **El tablero es un objeto manipulable**, no un panel fijo: se agarra, se
   escala y se mueve con las manos.
2. **Todo el HUD son ventanas en el espacio**, cada una con su plano, y cada
   píxel y cada zona de pulsación salen de la **misma tabla**.
3. **Ninguna acción es inmediata**: el gatillo *arma*, y la orden sale al soltar.
   Cada modo dice en texto qué va a pasar.
4. **La vista 3D se renderiza con la cámara del ojo**, así que el estéreo es
   de verdad (geometría), no un desplazamiento de textura.

---

## 2. La brecha medida (con el código, no de memoria)

Hoy (`android/src/openxr_engine.cpp`, `src/tabletop.rs`):

- Las entidades reales del juego se dibujan como **sprites 2D planos** del
  atlas (`ent.sprite_id != 0` → quad con la textura del juego). El resto del
  diorama son **primitivas dibujadas a mano** en C++ (cajas, cilindros,
  catenarias para los postes).
- El **cargador GLB ya existe** (`load_glb_as_vertices` en `src/tabletop.rs`,
  con la FFI `xfactorvr_load_model`) pero **solo lo usa un test**: es código
  muerto en el visor.
- La **tabla entidad→modelo** de los GLB de GDani31 solo está en ese test.
- El **shader PBR completo** (instancing, IK, sombras, PBR) ya está en
  `shaders/model.vert` / `model.frag`, pero **no se usa**.
- El panel con la pantalla del juego es un **quad plano vertical** al fondo de
  la mesa, con el ojo resuelto desplazando la **UV** (efecto "TV 3D").
- El HUD son **tres sub-paneles** dibujados a mano (Construcción, Inventario,
  Fabricar) sin zonas de pulsación unificadas.
- Los controles son **directos**: el gatillo hace la acción en el momento.

Es decir: **el shader, el cargador de modelos y la tabla de modelos ya están
escritos**; lo que falta es conectarlos. Por eso el aspecto es el mismo que
en el post de GitHub.

### 2.1 ¿Es viable el port del mod 3D al motor? (medido, no supuesto)

`scripts/verificar_simbolos.py` lee la tabla de símbolos del ELF ARM64 del
motor (`factorio_linux_arm64/factorio/bin/arm64/factorio`, **no está stripped**
y tiene `debug_info`) y dice qué hooks existen con el mangling **Itanium**
(el motor de Windows usa MSVC, así que los nombres del mod no valen tal cual).

Resultado: **10 de los 17 hooks existen tal cual** (incluidos los cinco
`draw` de entidades: `AssemblingMachine::draw`, `Furnace::draw`,
`MiningDrill::draw`, `Turret::draw`, `AmmoTurret::draw`) y **7 cambiaron de
nombre o de sitio** en la build 2.1.22:

| Hook del mod | En la build 2.1.22 (ARM64) |
|---|---|
| `GameRenderer::acknowledgeResize` | `GameView::acknowledgeResize` |
| `WorkingVisualisations::drawCraftingMachine` | `WorkingVisualisations::drawMainAnimation` |
| `WorkingVisualisations::drawMiningDrill` | `WorkingVisualisations::drawMainAnimation` |
| `GameView::createRenderParameters` | no existe con ese nombre (nosotros ya loThemeamos por offset) |
| `Entity::getPrototype` / `Entity::getSurface` | no están en `Entity` |
| `DayTime::getDarkness` | no existe (`DayTime` solo tiene `setPosition`) |

Conclusión: **el port al motor es viable**, pero **no es la vía para la Pico**
(ver 3.1).

---

## 3. Arquitectura objetivo

### 3.1 Decisión principal: el 3D se dibuja en el VISOR, no en el motor

El objetivo es la **Pico 4 Ultra**, donde el motor gráfico iría por
**Zink → lavapipe (software)**: meter el renderer 3D de GDani31 dentro del
motor significaría renderizar PBR por software, y el motor no podría ir en
headless.

En cambio, el visor corre **nativo** en la GPU de la Pico. Y además el motor
puede ir **headless**: ya lo hacemos (la campaña se juega sin cliente gráfico).

De ahí sale la arquitectura:

```
   motor headless (ARM64, en Termux o en /data/local/tmp)
        │  RCON: snapshot real (entidades, terreno, items)   ← ya funciona
        ▼
   visor (APK, GPU nativa de la Pico)
        ├─ terreno 3D del mapa real            (src/terreno.rs, ya existe)
        ├─ MÁQUINAS CON MODELO 3D REAL        (T1: GLB de GDani31)
        ├─ tablero manipulable con las manos  (T3)
        └─ HUD en ventanas espaciales          (T2)
```

Ventajas: estéreo de verdad (geometría con la matriz del ojo), rendimiento
real, el motor puede ir headless (sin atlas de 1,3 GB), y **el juego original
sigue visible**: la vista 2D del motor se pinta como **textura del terreno**
(si el motor graphical corre) o se sustituye por el terreno 3D pelado.

El mod 3D de GDani31 se usa como **fuente de los modelos y de la tabla
entidad→modelo**, que es justo lo que falta.

### 3.2 El HUD: ventanas, no un panel

Una **tabla de controles única** (como `XrPanelLayout.h`): cada control tiene
id, rol, etiqueta, estado y su rectángulo en píxeles; de ahí sale **a la vez**
el dibujo y el hit-test, para que no se desincronicen. Las ventanas son
planos en el espacio, cada una en su sitio, y se pueden agarrar.

### 3.3 Los controles: armar, luego actuar

El gatillo **arma**; al soltar se emite la orden. Cada modo actual escribe su
**pista** en pantalla. Nada de acciones instantáneas que se disparen por
error.

---

## 4. Los trabajos (reparto por ficheros, sin solapes)

| # | Trabajo | Ficheros | Quién |
|---|---|---|---|
| **T1** | Modelos 3D reales en el diorama: tabla entidad→modelo, carga en el APK, geometría por entidad con su transformación, y **optimizar los GLB** (hoy pesan 21 MB la ensambladora) | `src/modelos3d.rs` (nuevo), `src/tabletop.rs`, `src/assets.rs` | @MrMeseek |
| **T2** | HUD: tabla de controles única + ventanas espaciales (Construir / Inventario / Fabricar / Vista) con sus estados y hit-test | `src/ui/` (nuevo) | @MrMeseek |
| **T3** | El tablero de verdad: plano en el suelo con la perspectiva del ojo, **agarrable y escalable con una y dos manos**, y la vista del juego como textura | `android/src/openxr_engine.cpp`, `src/tabletop.rs` (solo la parte de pose/escala) | agente principal |
| **T4** | Controles: gesto diferido (armar→soltar), hover del rayo sobre el tablero, paneo con el stick, pista del modo | `src/openxr/actions.rs`, `android/src/openxr_engine.cpp` | agente principal |

Reglas de la casa: comentarios y documentación en **español**; nada de
`rm` (usar `mavis-trash` o `os.remove`); lo pesado en `/Volumes/X10Pro/`; los
bancos se validan contra el **motor real** del contenedor, nunca con datos de
ejemplo.

### 4.1 Lo que ya está hecho (2026-10-22)

| Trabajo | Estado | Detalle |
|---|---|---|
| T1 | **hecho** | `src/modelos3d.rs` (tabla, caché, geometría), `src/bin/optimizar_glb.rs`, 26 GLB en `android/src/main/assets/models/`. **63,7 MB → 7,2 MB.** |
| T2 | **hecho** | `src/ui/` (tabla de controles única, 4 ventanas, estados con pista, fuente 5x7). 11 tests. |
| T3 | **hecho** | El modelo 3D real tiene prioridad sobre el sprite plano; los GLB salen del APK (se extraen al directorio interno de la app); `LiveEntity::model_id`; geometría viva en un draw call. |
| T4 | **hecho** | Gesto diferido: `xfactorvr_vr_hand_action` **arma**, `xfactorvr_vr_hand_release` **ejecuta**, `xfactorvr_vr_hand_armed` para el feedback. El HUD se pinta con la geometría que genera `src/ui/`. |

El punto que explain por que se veia igual que en el post de GitHub: **si la
entidad tenia sprite del atlas, se dibujaba el sprite 2D tumbado y se hacia
`continue`, de modo que la malla 3D nunca se llegaba a dibujar**. Y los GLB se
buscaban en `/sdcard`, que no existe en el escritorio y es lento en la Pico.

Pendiente de validar en el dispositivo: el aspecto de los modelos 3D, las
ventanas del HUD y el gesto diferido en MR.

### 4.2 El SDK de PICO, revisado

`/Volumes/X10Pro/Downloads/PICO OpenXR SDK 3.0.0-20250422.zip` (848 entradas):

- **El loader que ya usamos es el de PICO**: `libopenxr_loader.so` de arm64-v8a
  mide 731.328 bytes, los mismos que el de `android/lib/arm64-v8a/`.  No hay
  que cambiar nada.
- `OpenXR/pico/` trae **extensiones experimentales y pre-release de PICO**
  (las que usa GZH-XR, como la de colocar el tablero sobre la mesa real).  Son
  experimentales: usarlas ataría el visor al runtime de PICO.  La colocación
  manual con rayo y el agarre con las manos ya funcionan en cualquier
  dispositivo, así que no hacen falta.
- `Samples/framework/src/gui/` trae un **renderer de GUI VR con ImGui**
  (`ImGuiRenderer`, `GuiWindow`).  Es la vía para tener texto con fuente de
  verdad y widgets de verdad en el HUD, en vez de la fuente bitmap 5x7 que
  hay ahora.  Es trabajo grande (ImGui + backend OpenXR) y solo tiene sentido
  cuando se pueda validar en el dispositivo, así que queda como mejora
  pendiente, no como parte de este trabajo.

---

## 5. Cómo se valida

- `scripts/test_modelo3d.sh`: el banco del 3D (hoy mide los píxeles del cubo;
  con T1 debe medir los de un modelo real).
- `scripts/test_visor_real.sh`: el diorama trae las entidades del juego real.
- `scripts/test_stream_render.sh`: el estéreo real (paralaje).
- `scripts/test_visor_pico.sh`: en el dispositivo, el visor se conecta al
  motor y trae el juego real (logcat).
- `cargo test` (41 tests) y `cargo test --lib` del inyector (7).

Nada de esto se da por bueno sin ejecutarlo contra el motor real.
