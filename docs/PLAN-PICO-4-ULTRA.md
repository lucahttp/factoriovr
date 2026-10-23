# PLAN — Factorio en realidad mixta para **Pico 4 Ultra** (standalone)

> **Estado**: plan propuesto (2026-10-22). El destino está decidido por el usuario:
> **Pico 4 Ultra standalone (Android)**, no PC-VR.
>
> Este documento **no sustituye** a los ADR: los ejecuta y los ordena. Cada fase
> apunta al ADR que la respalda y a los bancos (`scripts/test_*.sh`) que la validan.
>
> Reglas de la casa (ver `ADR-012` §4): comentarios y documentación en español;
> nada de `rm`; lo pesado en `/Volumes/X10Pro/`; los bancos se validan **contra el
> motor real**, nunca con datos de ejemplo.
>
> ⚠️ **Revisado por `ADR-013` (2026-10-22)**: el usuario pidió quitar RCON,
> quitar la predicción y no reconstruir el juego a mano, usando el mod 3D como
> base. La conclusión del ADR es que **el repo ya está a medio camino** (la Pista
> B está portada: 16 hooks `draw(DrawQueue&)`, GLB en el contexto del motor,
> estéreo alterno engine-side), y que el trabajo de verdad es **convertir el
> inyector en el puente** (escena + entrada + frames). Consecuencias:
> **F0 ahora mide FPS**, **F2 y F3 se vacían en la variante V1** (la dibuja el
> motor), y aparece **R0** (protocolo de escena in-process + inyección de
> entrada), que sustituye a RCON.

---

## 0. La decisión, en un párrafo

El motor de Factorio **no va a dibujar la realidad mixta**. Se queda como
**servidor de estado** (headless, RCON + mod puente) y **todo el 3D lo dibuja el
visor (APK) en la GPU de la Pico**: terreno, máquinas con su modelo 3D real, HUD
espacial, estéreo de verdad. Esa decisión ya está tomada en `ADR-012` §3.1 y la
evidencia externa la confirma: el mejor caso conocido de Factorio ARM64 corriendo
en un móvil son **3,5–10 FPS** (r/termux, 813 upvotes) y un visor estéreo a 90 Hz
no perdona eso. Lo que falta no es render: es **fidelidad de juego** (cintas,
fluidos, energía, trenes), **interfaz** (investigación, texto, mapa), **anclaje MR
a la mesa real** y **empaquetado** (que la app arranque el motor sola).

Existe además un hallazgo que **cambia el diagnóstico histórico**: el bloqueo
"en Android no hay GPU" era cierto para el ICD de Adreno (es bionic y no se puede
`dlopen` en un proceso glibc), pero **el payload de Mesa del proyecto ya incluye
Turnip** (`libvulkan_freedreno.so`), el driver abierto de Adreno que habla con el
kernel KGSL y **no depende de la libc de Android**. Esa vía nunca se ha probado
en la Pico: es el spike F0.

---

## 1. Diagnóstico: hechos verificados

### 1.1 El motor

| Hecho | Cómo se comprobó | Implicación |
|---|---|---|
| Factorio 2.1.22 tiene build oficial **linux-arm64** (la hizo Wube para el Steam Frame) | `FFF-446` (18-09-2026) | No hay que compilar nada |
| El binario ARM64 es ELF64 aarch64 PIE de 261 MB, `NEEDED = libresolv.so.2, libm.so.6, libc.so.6` | Lectura del ELF (cabecera + `.dynamic`) | SDL2 y libstdc++ van dentro; **el motor solo pide glibc** |
| **No está stripped**: 291 301 símbolos en `.symtab` | Lectura de `.symtab`/`.dynsym` | Los hooks del inyector por **nombre** son viables. El mod 3D de GDani31 no vale tal cual (usa PDB de MSVC) |
| El motor pide **OpenGL 3.3 core** y `dlopen`ea el driver en runtime (`libGL.so`, `libEGL`, `libvulkan`, `SDL_VIDEODRIVER` presentes como strings) | Búsqueda de strings en el binario | El problema nunca fue de enlazado: es **qué driver hay disponible** |
| El RCON es **función de servidor**, no de single-player | `--help` del cliente; `NOTAS-carga-y-campana.md` §6 | El headless es el que habla con la app; el cliente gráfico no sirve para eso |
| El cliente gráfico se atasca cargando una partida (el `InLoadingGameBox::process()` nunca se invoca) y muere con 4 GB de RAM | `NOTAS-carga-y-campana.md` §1–§3 | El **headless + jugador virtual** (ADR-007) es el camino fiable |

### 1.2 La cadena de gráficos en Android

| Hecho | Cómo se comprobó | Implicación |
|---|---|---|
| El motor arranca hoy con **Zink → lavapipe (software)**: `Initialised OpenGL:[1] zink Vulkan 1.4(...); driver: 4.6 (Core Profile)` | Banco en el contenedor ARM64 | Funciona, pero **sin GPU**: cualquier 3D dentro del motor va por CPU |
| La causa es que el ICD de Adreno de Android (`/vendor/lib64/hw/vulkan.adreno.so`) es **bionic** y no se puede `dlopen` en un proceso glibc | `PISTA-TERMUX.md`, `PISTA-PICO.md` §3 | Era el bloqueo "no hay GPU en Android" |
| **El payload de Mesa del proyecto ya trae Turnip**: `build/mesa/usr/lib/aarch64-linux-gnu/libvulkan_freedreno.so` + `freedreno_icd.aarch64.json` (Mesa 23.9.239) | Listado del árbol `build/mesa` | Turnip habla con `/dev/kgsl-3d0` (kernel) → **sin conflicto de libc**. Nunca se ha probado en la Pico |
| La comunidad usa **exactamente** esa pila: Termux + Termux:X11 + PRoot + **Zink/Turnip** (Adreno) o VirGL/ANGLE (Mali) | r/termux (813 upvotes), guía de GPU para contenedores Android | En **Adreno** el camino es Turnip; VirGL es para Mali |
| El mejor resultado de Factorio ARM64 en un móvil es **3,5 FPS** (Redmi Note 10T, Mali) y ~10 FPS con VirGL en Android 17 | r/termux, r/factorio | **El motor no puede ser el que renderice la MR** |
| El Steam Frame (el Linux ARM64 de referencia de Wube) usa **Zink**, y aun así FFF-446 reporta que bajar ajustes no mejora | Phoronix, `FFF-446` | Es un límite estructural de OpenGL→Vulkan, no de ajustes |

### 1.3 El visor y la Pico

| Hecho | Cómo se comprobó | Implicación |
|---|---|---|
| El visor ya usa **`XR_FB_passthrough`** (reconstrucción) y espacio **STAGE** a 0,74 m | `android/src/openxr_engine.cpp` | La MR está montada; falta **anclarla a la mesa real** |
| Pico 4 Ultra = **Snapdragon XR2 gen 2**, Android 14, OpenGL ES + Vulkan, conformante a **OpenXR 1.1** (PICO OS ≥ 5.12.0U) | Khronos OpenXR Runtime Extension Support Report | Se puede usar el loader estándar de Khronos |
| Extensiones disponibles: `XR_BD_spatial_anchor`, `XR_BD_spatial_anchor_persistence`, `XR_BD_spatial_mesh`, `XR_BD_spatial_plane`, `XR_BD_spatial_scene`, `XR_FB_passthrough`, `XR_EXT_frame_synthesis`, `XR_PICO_adaptive_resolution`, `XR_PICO_secure_mixed_reality`, `XR_PICO_hand_tracking` | Khronos (Pico 4 Ultra) | **Detectar la mesa y anclar el tablero a ella** es una extensión estándar, no privativa |
| `XR_PICO_secure_mixed_reality` es exclusiva de la serie 4 Ultra | PICO Native SDK 3.0.0 | Ventaja del dispositivo elegido |

### 1.4 El estado del repo

- 12 ADR, ~22 500 líneas de Rust (inyector, visor, UI, assets) + ~4 200 de C++ (motor OpenXR) + 40 bancos.
- **Salud**: `cargo test` en la raíz → 18 tests OK; `cargo test --lib` en el inyector → **26 OK**.
- **Rojo**: `cargo test` completo en el inyector → **10 doctests FALLAN** (pseudocódigo en bloques ```rust de `modelo3d.rs`). Falso rojo que esconde el resultado real.
- **Deriva de documentación**: `ADR-012` §5 dice "41 tests"; hoy son 18 + 26.
- **Sin commitear**: `injector-linux/src/{render3d,modelos,glb,json,proto}.rs` (Pista B: dibujar los modelos 3D *dentro* del motor), `hooks.rs`, `lib.rs`, `modelo3d.rs` (+238 líneas) y `runtime/` (módulo para arrancar el motor desde la app).

---

## 2. Arquitectura objetivo

```
┌────────────────────────── PICO 4 ULTRA ──────────────────────────┐
│                                                                  │
│   ┌── visor (APK, OpenXR 1.1, GPU Adreno nativa) ────────────┐   │
│   │  terrain 3D + entidades con MODELO 3D real (GLB)         │   │
│   │  HUD en ventanas espaciales  ·  manos  ·  audio/háptica   │   │
│   │  ancla MR sobre la MESA REAL (XR_BD_spatial_anchor)      │   │
│   └───────────────▲──────────────────────────────────────────┘   │
│                   │ RCON (localhost)  ▲ estado + órdenes         │
│   ┌───────────────┴───────────────────┴──────────────────────┐  │
│   │  motor Factorio 2.1.22 ARM64 — HEADLESS (sin GPU)       │  │
│   │  simulación real · campaña · mod xfactorvr_bridge        │  │
│   └──────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
```

**Reparto de papeles (definitivo)**

| Pieza | Quién | Por qué |
|---|---|---|
| Simulación, estado, órdenes | Motor headless | Es el juego real; el RCON ya funciona (ADR-011: RESUELTO) |
| Geometría, cámara, estéreo | Visor | GPU nativa, 90 Hz, estéreo de verdad (geometría, no UV) |
| Modelos 3D | Los GLB de GDani31 (fuente) + `src/modelos3d.rs` (tabla entidad→modelo) | Ya está: 63,7 MB → 7,2 MB optimizados (ADR-012 T1) |
| Transporte | RCON sobre localhost + snapshot | Probado contra el motor real |

**Lo que ya no se va a hacer** (y por qué, para que no se vuelva a abrir)

| Descartado | Motivo |
|---|---|
| VirGL / ANGLE para el motor | Es la vía de **Mali**; en Adreno es Turnip; y además el motor no debe renderizar la MR |
| El ICD de Adreno de `/vendor` | Bionic: no se puede `dlopen` en un proceso glibc |
| El cliente gráfico como camino principal | 3,5–10 FPS en móvil; y tiene un bloqueo de carga sin resolver |
| El mod 3D de GDani31 tal cual | Es un DLL de Windows que hookea por PDB; aquí hay que portar los hooks a `.symtab` de ARM64. Su **valor real son los assets y la tabla entidad→modelo**, que el visor ya consume |
| Steam Link / ALVR / PC-VR | El objetivo es **standalone** |
| Reimplementar el motor | El motor real ya corre ARM64 |

---

## 3. Plan por fases

Cada fase tiene **criterio de aceptación** verificable. Ninguna avanza a la
siguiente sin el criterio verde (salvo F6/F7, que son explícitamente opcionales).

### F0 — Spike: GPU real en la Pico **y cuántos FPS da** *(bloqueante, ~1 día)*

> **Revisado por ADR-013**: no basta con "arranca". El estéreo engine-side
> alterna el ojo en cada frame, así que para 90 Hz hacen falta **90 frames/s del
> motor** (45 FPS por ojo). El número de FPS decide entre V1 (el motor dibuja) y
> V2 (dibuja el visor). Si hay red de seguridad: `XR_EXT_frame_synthesis`.

**Objetivo**: medir **cuántos FPS da el cliente gráfico con GPU real en la Pico**,
y de paso el techo del dispositivo para el visor.

**Por qué es la primera**: si sale verde, la Pista B (F6) se abre y el visor
puede pedirle al motor cosas que hoy no puede. Si sale roja, el resto del plan no
cambia (el motor va headless igual), pero nos ahorra meses de_track equivocado.

**Tareas**

1. Conectar la Pico por USB y verificar root:
   ```bash
   ADB=~/Library/Android/sdk/platform-tools/adb
   $ADB devices -l
   $ADB shell 'su -c id'                       # uid=0(root)
   $ADB shell 'su -c "ls -l /dev/kgsl-3d0"'
   $ADB shell 'su -c "cat /sys/class/kgsl/kgsl-3d0/gpu_model"'   # Adreno 740?
   $ADB shell 'su -c getenforce'
   ```
   *Si `su` no existe*: hay root público para Pico 4 Ultra con PICO OS ≤ 5.13.3
   (`picohaxx`/`cheese`). Es el mismo root que ya se supone en `PISTA-PICO.md` §3.
2. **Activar Turnip** en el runtime (hoy solo se usa `lvp_icd`):
   ```bash
   # en el runtime del dispositivo, con el ICD reescrito al prefijo real:
   export VK_ICD_FILENAMES=$RUNTIME/usr/share/vulkan/icd.d/freedreno_icd.aarch64.json
   export VK_DRIVER_FILES=$VK_ICD_FILENAMES     # Vulkan 1.3
   export LIBGL_DRIVERS_PATH=$RUNTIME/usr/lib/aarch64-linux-gnu/dri
   export MESA_LOADER_DRIVER_OVERRIDE=zink
   export MESA_GL_VERSION_OVERRIDE=3.3
   ```
   *Tarea oculta*: los `.json` de los ICD llevan rutas absolutas (`/usr/lib/aarch64-linux-gnu/...`);
   hay que reescribirlas al prefijo del runtime (patrón ya existente en `scripts/prepare_mesa.py`).
3. Probar con el **propio motor**, que es el mejor探测ador que hay: arranca el
   cliente gráfico y mira la línea de log
   `Initialised OpenGL:[1] zink Vulkan 1.4(...)`.
   - Debe decir **Adreno**, no `llvmpipe`.
   - Comparar FPS con el banco de lavapipe actual (320×180).
4. Medir el techo del dispositivo con un banco de la app (no del motor):
   `scripts/test_visor_pico.sh` → FPS y RAM con el diorama real.

**Criterio de aceptación**
- El motor (o cualquier proceso glibc del runtime) anuncia **Adreno** en el renderer string, **y**
- **FPS medidos** del cliente gráfico con Turnip a 1280×720 y a 640×360, o
- se documenta el error exacto por el que Turnip no arranca (permiso de `/dev/kgsl-3d0`, SELinux, ioctl) y **se decide con esa evidencia**.

**Riesgo**: medio. Que KGSL esté bloqueado por SELinux aunque haya root.
**Mitigación**: `setenforce 0` como root; si aun así falla, el plan sigue (el motor va headless).

---

### F1 — La fábrica, **sobre la mesa de verdad** *(~2–3 días)*

**Objetivo**: que el diorama se ancle a la mesa física, no a un plano imaginario
a 0,74 m. Es el salto de "un panel flotante" a "mi fábrica en la mesa".

**Tareas**
1. `XR_BD_spatial_plane` / `XR_BD_spatial_mesh`: detectar la mesa; el usuario la
   confirma con un rayo (o con la captura de escena de PICO).
2. `XR_BD_spatial_anchor` (+ `XR_BD_spatial_anchor_persistence`): crear el ancla
   sobre esa superficie y **guardarla**, para que la próxima sesión la recupere.
3. Escala 1:1 con las unidades del juego (definir el tamaño físico de un tile y
   la altura de un "nivel" de Factorio) y **reubicación** cuando OpenXR
   re-localiza: la idea de `XrReferenceSpace.h` de GZH-XR (compensar poses, nunca
   usar la altura de la cabeza).
4. Fallback honesto: si no hay ancla (o no se puede persistir), usar el STAGE de
   hoy con la altura manual.
5. Agarre con una y dos manos (mover/escalar el tablero), que es la idea #1 que
   el equipo ya decidió traer de GZH-XR.

**Criterio de aceptación**: al abrir la app, aparece la fábrica **encima de la mesa
real**, sobrevive a reiniciar la app y se puede agarrar y escalar con las manos.

**Riesgo**: medio. La persistencia de anclas en PICO es—inestable entre versiones de PICO OS.
**Mitigación**: la persistencia es un extra; el ancla en runtime ya da el 90 % del efecto.

---

### R0 — El puente sin RCON *(nuevo, ~4–5 días, paralelo, se valida en el Mac)*

> Añadido por `ADR-013`. Es el trabajo que **sustituye a RCON** y a la
> predicción, y **es común a V1 y V2**, así que no se pierde nada si F0 sale rojo.

**Objetivo**: que el inyector sea el puente (escena + entrada + frames) y que
RCON deje de ser el transporte del estado.

1. **Escena in-process**: en el hook de `draw()` acumular por frame
   `(tipo, prototipo, x, y, dirección)` y mandarlo por el socket XFV1 con el
   frame. Coste: unos pocos kB por frame. El inyector **ya tiene** estos datos
   (los usa para colocar los modelos 3D).
2. **Entrada inyectada**: rayo de la mano → posición del mapa (el truco
   `GetMapPosition`/`ComputeWalkDir` del mod 3D) → `SDL_PushEvent` con el clic.
   El motor aplica sus reglas: coste, prerequisites, rotación, fantasma, deshacer.
3. **Teclado** por `SDL_PushEvent` para escribir (chat, renombrar, buscar).
4. **Cerrar RCON**: `autostart::tick_rcon()` deja de abrir el puerto; el puente
   Lua queda como complemento para lo que el inyector no ve (terreno, recetas).
5. **Banco nuevo**: `test_entrada_inyectada.sh` — el clic del VR construye en el
   motor real. Es el banco que demuestra que **el juego sigue siendo el juego**.

**Criterio de aceptación**: con el puente, el visor ve la escena sin RCON y una
orden del VR produce una acción real del motor (construir/minar), verificado
contra el estado del juego.

---

### F2 — Que se sienta **Factorio** *(~2 semanas, pero **se vacía en V1**)*

> **Revisado por ADR-013**: en la variante **V1** (el motor dibuja), esta fase
> **no existe**: las cintas, los fluidos, la energía, los trenes y los biters los
> dibuja el juego. Solo queda work si se va por **V2** (el visor dibuja la
> geometría) — y entonces el inyector los transmite en vez de reconstruirlos a mano.

**Objetivo**: cerrar la distancia entre "un diorama bonito" y "estoy jugando".
El visor hoy dibuja sprites/modelos sueltos; un juego de fábrica es sobre todo
**flujo de materiales en movimiento**.

**Orden de trabajo** (por impacto en la sensación de juego):

| # | Qué | Qué hace falta |
|---|---|---|
| 1 | **Cintas transportadoras con ítems circulando** | El mod puente debe enviar, por cinta, la posición de cada ítem (no solo la cinta). Animación en el visor |
| 2 | **Ítems en el suelo / chutes / máquinas** | Posición de los ítems del área del jugador |
| 3 | **Fluidos** (agua, vapor) | Superficie del fluido + paredes animadas |
| 4 | **Red eléctrica** | Postes con catenaria, subestaciones, satisfacción, apagones |
| 5 | **Trenes y vías** | Locomotor, vagones, raíl, control de locomotoras (el icono del juego) |
| 6 | **Enemigos (biters)** | Nidos, oleadas, alerta: sin ellos no hay tensión de Factorio |
| 7 | **Terreno con relieve** | Cliff/rampas con altura, lago/mar, árboles (base de `src/terreno.rs`) |
| 8 | **Efectos** | Humo, chispas, robots, animación de trabajo, robots de construcción |
| 9 | **Previsualización de construcción** | Fantasma verde/rojo + rotación: lo que hace Factorio legible |
| 10 | **Sonido posicional** | Ya hay audio/háptica: validar que el motor suene y que se localize |

**Criterio de aceptación**: una partida de campaña se puede **jugar entera** desde
el visor (extraer, fabricar, investigar, automatizar) sin tocar el PC.

**Riesgo**: alto en cantidad, bajo en dificultad (cada punto es "mover datos por
RCON y dibujar"). El riesgo real es **no acabar**: por eso la tabla está ordenada
y F2 puede cortarse por la mitad (1–4 ya dan la sensación de fábrica).

---

### F3 — La interfaz que falta *(~1 semana, pero **se vacía en V1**)*

> **Revisado por ADR-013**: en la variante **V1**, la GUI del juego (árbol de
> investigación, mapa, inventario, chat) va **dentro del frame capturado** y es
> clicable con la entrada inyectada → esta fase desaparece. En **V2** se queda,
> y se puede simplificar a "lista de tecnologías disponibles" en vez de un árbol
> completo.

**Objetivo**: sin esto no se puede *jugar* Factorio, solo mirar un diorama.

| # | Qué | Decisión |
|---|---|---|
| 1 | **Árbol de investigación** (obligatorio) | Ventana espacial con scroll porcategories, selección, confirmación **armar→soltar** (el gesto diferido ya implementado) |
| 2 | **Entrada de texto** | Teclado del sistema de PICO (`InputMethodManager`) + el `XrMetaKeyboard.h` de GZH-XR como referencia. Sirve para chat, renombrar, buscar recetas |
| 3 | **Mapa / minimapa** | La superficie completa como textura en un quad (el terreno ya se conoce) |
| 4 | **HUD de estado** | Producción por ítem, energía, alertas, cola de facturas |
| 5 | **Inventario del jugador** | Mismas ventanas que ya existen (`src/ui/`) |
| 6 | **Fuente de verdad** | La fuente bitmap 5×7 es provisional: migrar a ImGui (el SDK de PICO trae `ImGuiRenderer`) cuando se pueda validar en el dispositivo |

**Criterio de aceptación**: se puede desbloquear una tecnología, escribir un chat y
abrir el mapa **sin manos en un teclado físico**.

---

### F4 — Rendimiento y estabilidad en el dispositivo *(~3–5 días)*

**Objetivo**: 90 Hz estables y sesiones largas, sin que el móvil se caliente ni
mate el proceso.

- **Rendimiento**: la app a 90 Hz con el diorama completo (F2) — `XR_EXT_frame_synthesis`
  y `XR_PICO_adaptive_resolution` están en el dispositivo y sirven de red de seguridad.
- **Memoria**: el motor headless pide ~1,5–2,5 GB; el visor, el atlas de sprites y los GLB
  (~7 MB) el resto. Medir con `logcat` + `dumpsys meminfo` y **bajar el atlas** si aprieta.
- **Térmica y batería**: sesión de 45 min con degradación (XR2 se pone al rojo).
- **Robustez**: reconexión del RCON, motor que muere y hay que relanzar, la app
  que se queda sin ancla.

**Criterio de aceptación**: 45 min de sesión estable a 90 Hz, sin OOM kill, sin
crash del visor ni del motor.

---

### F5 — Que sea **una sola app** *(~3–5 días)*

**Objetivo**: hoy el motor lo arranca un shell script (Termux o `/data/local/tmp`);
la experiencia de producto es "una app que abre el juego".

**Tareas**
1. Terminar el módulo `runtime/` (ya sin commitear): el APK ejecuta el motor
   con `execve` del loader glibc y `LD_LIBRARY_PATH` al runtime, motor en el
   directorio privado de la app (sin `noexec` de `/sdcard`).
2. El motor abre el RCON en localhost; la app lo encuentra (fichero de puerto +
   contraseña en el dir privado) y se conecta.
3. Ciclo de vida: abrir la app → arranca el motor → carga/crea partida → conecta →
   ancla MR → **se puede seguir donde se dejó** (autosave).
4. Icono en el menú de PICO, orientation, splash, y un "Factorio VR" reconocible.
5. Dejar Termux como **vía de desarrollo**, no como requisito del usuario.

**Criterio de aceptación**: desde el menú de la Pico, una toque y la fábrica
aparece en la mesa.

---

### F6 — *(Opcional)* El juego real dibujando en 3D *(~2–3 semanas, condicional)*

**Solo si F0 salió verde.** Es la Pista B que ya está a medio hacer (el
`modelo3d.rs` sin commitear dibuja los modelos GLB **dentro** del contexto GL del
motor, portando los hooks de GDani31 a los símbolos del ELF ARM64).

**Qué aportaría**: fidelidad imposible de replicar a mano (cintas con animación
real, humos, GUI del juego, la cámara deformada de GDani31) y el "modo panel"
real del juego en vez de una reimplementación.

**Qué cuesta**: (a) resolver el **bloqueo de carga** del cliente gráfico
(`NOTAS-carga-y-campana.md` §3, sin resolver), (b) llevar el runtime a la Pico
con GPU, (c) laProjection: un framebuffer 2D proyectado es **un póster**, no
geometría — el 3D real seguiría siendo el del visor.

**Regla**: F6 **nunca** bloquea F1–F5. Si el tiempo no da, se aparca y el
proyecto sigue con el visor.

---

### F7 — *(Paralelo, en el Mac)* Pista B como banco de hooks *(~3–5 días, en el Mac)*

En el Mac hay binario **x64** local (`factorio/bin/x64/factorio`), así que el
inyector se puede validar **natively y rápido** (sin contenedor ARM64).

**Tareas**: portar los hooks de GDani31 a los símbolos de la build 2.1.22 (el
ADR-012 §2.1 ya mide **10 de 17** hooks tal cual y los 7 que cambiaron), y usar
el banco para (a) validar la inyección, (b) generar frames de referencia, (c)
extraer assets.

**Valor**: deja de ser "el camino principal" y pasa a ser **laboratorio**. Es
también el único sitio donde el render del motor es barato, porque el Mac tiene
GPU de verdad.

---

## 4. Riesgos

| Riesgo | Prob. | Impacto | Mitigación |
|---|---|---|---|
| **Turnip no arranca en la Pico** (KGSL bloqueado por SELinux aunque haya root) | media | medio (cierra F6) | El plan principal (F1–F5) no depende de él. Escríbelo como evidencia y sigue |
| El cargador de partida del cliente gráfico nunca se resuelve | alta | medio | El headless + jugador virtual ya resuelve la campaña (ADR-007) |
| **F2 no se termina** → el diorama sigue siendo un maqueta | alta | **alto** | F2 está ordenada por impacto; cortar en el punto 4 ya se siente Factorio |
| Rendimiento insuficiente con el diorama completo | media | alto | Instancing (ya hay `draw_instances`), atlas comprimido, `XR_PICO_adaptive_resolution` |
| OOM: motor + visor + atlas en 12 GB | media | alto | Bajar el atlas, medir pronto, `MEMORY_LIMIT` del headless |
| Root no disponible en tu PICO OS | media | **alto** (rompe F0/F1) | Comprobarlo **el primer día**; si no hay root, F1 se limita a STAGE + altura manual y F6 queda fuera |
| Deriva entre el visor y el mod puente (formatos que no cuadran) | media | medio | Un banco que valide el snapshot contra el motor real (regla de la casa) |
| Calor/batería | alta | medio | Sesiones cortas por defecto, pausa al bajar el visor |

---

## 5. Definición de "hecho" (DoD) del proyecto

1. Desde el menú de la Pico, **una app** arranca el motor y muestra la fábrica
   **anclada a la mesa real**.
2. 90 Hz estables durante 45 min.
3. Se puede **extraer, fabricar, investigar y automatizar** entero desde el visor.
4. El estado que se ve es **el del juego real** (verificado contra el motor, no simulado).
5. Se puede **guardar y continuar** una partida desde el visor.

---

## 6. Las primeras 48 horas (acciones concretas, en orden)

1. `adb` + `su` + `/dev/kgsl-3d0` + `gpu_model` en la Pico → **F0, paso 1**.
   (Es lo único que puede cambiar el plan; el resto se puede hacer mientras.)
2. Con root, `getenforce` y, si hace falta, `setenforce 0`.
3. Reescribir los ICD de Mesa con el prefijo del runtime y arrancar el cliente
   gráfico **con Turnip** en vez de lavapipe → mirar la línea `Initialised OpenGL`.
4. En paralelo (Mac): arreglar los **10 doctests** del inyector (cambiar ```rust
   por ```text en los bloques de pseudocódigo) para dejar `cargo test` en verde,
   y commitear el trabajo sin commitear (que hoy es riesgo de perderlo todo).
5. En paralelo: medir en la Pico el FPS real del visor con el diorama actual
   (`scripts/test_visor_pico.sh`) → es la línea base de F4.
6. Escribir el ADR-013 con el resultado de F0 (verde → se abre F6; rojo → se
   documenta y se sigue con F1).

---

## 7. Referencias

- **FFF-446 "An ARM and a Frame"** — el build linux-arm64 oficial de Wube:
  <https://factorio.com/blog/post/fff-446>
- **r/termux** — "Factorio ARM64 running natively on Android hardware through Termux"
  (813 upvotes; 3,5 FPS con VirGL/ANGLE sobre Mali):
  <https://www.reddit.com/r/termux/comments/1wl9p8a/>
- **r/factorio** — mismo hilo, con la recipe de Termux + PRoot + traducción GL→Vulkan:
  <https://www.reddit.com/r/factorio/comments/1wl9236/>
- **Mesa / Zink** (OpenGL sobre Vulkan):
  <https://docs.mesa3d.org/drivers/zink.html>
- **Turnip** (driver abierto de Adreno, vía kernel KGSL) — el que falta activar:
  <https://docs.mesa3d.org/drivers/turnip.html>
- **Generals-Zero-Hour-XR** — la referencia de UX (tablero manipulable, ventanas
  espaciales, gesto diferido, cámara del ojo):
  <https://github.com/Cesarus85/Generals-Zero-Hour-XR>
- **factorio-3d-mod** (GDani31) — la fuente de los modelos 3D y de la tabla
  entidad→modelo:
  <https://github.com/GDani31/factorio-3d-mod/tree/main>
- **Khronos OpenXR Runtime Extension Support Report** — extensiones de Pico 4 Ultra:
  <https://github.khronos.org/OpenXR-Inventory/runtime_extension_support.html>
- **PICO Native SDK 3.0.0** (MR, frame synthesis, adaptive resolution):
  <https://developer.picoxr.com/blog/native-sdk-3/>
