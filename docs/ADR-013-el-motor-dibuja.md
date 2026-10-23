# ADR-013 — El motor dibuja: el mod 3D como render y el inyector como puente (RCON fuera)

> **Estado**: propuesto (2026-10-22). **Revisa** `ADR-012` §3.1 y el alcance de
> F2/F3 de `PLAN-PICO-4-ULTRA.md`.
>
> Origen: el usuario pidió quitar RCON, quitar la predicción y **no reconstruir el
> juego a mano**, usando el mod 3D de GDani31 como base.
>
> Conclusión corta: **sí se puede, y el repo ya está a medio camino**. El cambio
> de arquitectura real no es "reconstruir el juego en el visor", es **convertir el
> inyector en el puente** (escena + entrada + frames) y dejar que el motor siga
> siendo el motor.

---

## 1. Lo que el repo ya tiene (evidencia, no suposición)

| Pieza | Estado real | Dónde |
|---|---|---|
| **Port del mod 3D a ARM64** | **Hecho**: 16 hooks de `X::draw(DrawQueue&)` (ensambladora, horno, taladro, contenedor logístico, acumulador, tanque, estación de carga, silo, laboratorio, caldera, poste eléctrico, roboport, radar, insertador, torreta, torreta de munición, contenedor) | `injector-linux/src/modelo3d.rs:957+` |
| **Modelos GLB dibujados en el contexto del motor** | **Hecho**: `render3d.rs` resuelve GL por `dlsym` (GLAD no expone `glUniformMatrix4fv`) y dibuja los GLB en el framebuffer del juego | `injector-linux/src/render3d.rs` |
| **Prototipo de cada entidad (qué modelo le toca)** | **Hecho**: como `Entity::getPrototype()` no existe en la build 2.1.22 ARM64, se localiza **en caliente** escaneando el objeto de la entidad | `injector-linux/src/proto.rs` |
| **Estéreo real del lado del motor** | **Hecho**: `XFACTORVR_3D_CAMERA=D` desplaza la cámara y **alterna el signo en cada frame**; el visor reparte las vistas por ojo | `hooks.rs:99`, `modelo3d.rs:177` |
| **Captura del frame completo** | **Hecho**: se captura en `GameRenderer::render` *antes* de que el motor vacíe el framebuffer del mundo | `hooks.rs:92` |
| **Avance de la carga de la partida** | **Hecho**: hook de `MainLoop::processEvents` (se llama en menú, carga y juego) | `hooks.rs:72` |
| **El motor exporta la entrada** | `SDL_PushEvent`, `SDL_SendMouseMotion`, `SDL_SendMouseWheel`, `SDL_PumpEvents` en el `.symtab` | symtab del binario ARM64 |
| **El motor exporta su interfaz de comandos** | 93 símbolos de `CommandProcessor` (`executeCommand`, `executeSilentCommand`, `executeMeasuredCommand`) — **la misma que usa RCON por dentro** | symtab del binario ARM64 |
| **El inyector ya abre el RCON desde dentro del motor** | `autostart::tick_rcon()` | `hooks.rs` (final) |

**Consecuencia**: el inyector ya lee el estado **en el proceso**, ya dibuja en el
contexto del motor y ya sabe qué entidad es cuál. RCON y la predicción son, hoy,
capas que se pueden desmontar.

---

## 2. La arquitectura de tres capas, sin RCON

```
   MOTOR (Factorio 2.1.22 ARM64)                = el juego: reglas, GUI, física
        │  el inyector vive DENTRO del motor (LD_PRELOAD)
        │  ├─ lee la escena en el mismo frame (posiciones, prototipos, dirección)
        │  ├─ dibuja los modelos 3D en el contexto GL del motor
        │  └─ captura el frame (con la GUI del juego dentro)
        ▼
   INYECTOR  ──► socket XFV1 (ya existe) ──► VISOR (APK OpenXR)
        ▲                                      ├─ stereo alterno por ojo
        │                                      ├─ passthrough MR + ancla en la mesa
   entrada VR ──┘  SDL_PushEvent               └─ cámara del ojo, 90 Hz
   (rayo → tile → clic)  (el motor aplica sus reglas: ghost, rotación, coste)
```

- **No hay RCON** para el estado: la escena viaja en el frame en que se generó.
- **No hay predicción**: no hay snapshot que extrapolar; el visor muestra lo último
  recibido. La interpolación entre frames del motor es una Repaint de textura, no
  una predicción de estado.
- **No hay reconstrucción**: las cintas, los fluidos, la energía, los trenes, los
  biters, la investigación y la GUI **son los del juego**, porque los dibuja el
  juego.
- **Las órdenes dejan de ser "comandos"**: la mano del VR traza un rayo, el
  inyector lo traduce a la posición del mapa (el truco de `GetMapPosition`/`ComputeWalkDir`
  del mod 3D) y **sintetiza un clic** con `SDL_PushEvent`. El motor aplica sus
  propias reglas: coste, prerequisites, rotación, modo fantasma, deshacer.

---

## 3. La frontera honesta: el motor dibuja en **ortográfica**

> "El motor dibuja el mundo con una proyección **ortográfica en X e Y** (el suelo
> es un plano). Para que un modelo 3D se vea con volumen se le aplica un inclinado
> y se proyecta al plano de la pantalla." — `render3d.rs`

Esto significa que **el frame del motor es un póster**: no hay geometría por
píxel, hay un dibujo 2D con modelos inclinados que fingen volumen. De ahí dos
variantes, y la diferencia es real:

| | **V1 — El juego dibuja** (póster estéreo) | **V2 — El visor dibuja** (geometría real) |
|---|---|---|
| Qué se ve en la mesa | El frame del motor, en dos vistas alternas | Malla real (modelos GLB) con la matriz del ojo |
| Fidelidad del juego | **Máxima**: la GUI real, las animaciones, los efectos | La que el visor sabe dibujar |
| Se puede rodear la fábrica | **No** (es un plano) | **Sí** (geometría) |
| Anchura | **Estéreo por paralaje**: con la cámara del motor desplazada ±D px, el ojo izquierdo y el derecho ven la escena desde puntos distintos → el diorama tiene profundidad | Estéreo por geometría |
| Coste de render del motor | **Alto**: el motor dibuja la escena **dos veces por frame** | Ninguno (el motor puede ir headless) |
| Reconstrucción | Cero | La que ya existe (T1–T4) |

**El punto clave**: V1 y V2 **no son alternativas excluyentes**, sino dos salidas
del mismo puente. El inyector ya sabe, en cada `draw()`, la posición de la
entidad en tiles, su prototipo y su dirección → puede mandar esa escena al visor
(V2) **y** mandar los frames (V1), con el mismo código y sin RCON.

---

## 4. El coste oculto del estéreo alterno: el motor tiene que ir rápido

El estéreo engine-side alterna el ojo **en cada frame**, así que para 90 Hz con
dos ojos hacen falta **90 frames por segundo del motor** (45 FPS por ojo y por
segundo). Un móvil con GPU de Adreno 740 vía Zink puede acercarse, pero hay que
medirlo, y si no llega hay red de seguridad: **`XR_EXT_frame_synthesis`**, que la
Pico 4 Ultra soporta, reproyecta el último frame y salva el judder.

Por eso **F0 (el spike de GPU) tiene que medir FPS, no solo "arranca"**.

---

## 5. Qué cambia en el plan

| Fase del PLAN | Antes | Ahora |
|---|---|---|
| **F0** | "¿Hay GPU?" (arranca o no) | **"¿Cuántos FPS da el cliente gráfico con Turnip?"** (el número decide V1/V2) |
| **F2** (reconstruir cintas, fluidos, energía, trenes, biters) | 2 semanas de trabajo | **Cero para V1**: los dibuja el motor. Para V2, el inyector los transmite (el mod 3D ya tiene `hooks/items.rs` para los ítems en las cintas) |
| **F3** (árbol de investigación, texto, mapa) | 1 semana | **Cero para V1** (la GUI del juego va en el frame, clicable con el input inyectado). Para V2, se queda, pero se puede simplificar a "lista de tecnologías disponibles" |
| **F6** (el juego real dibujando) | opcional, 2–3 semanas | **deja de ser opcional**: es el camino principal |
| **R0** (nuevo) | — | **Protocolo de escena in-process + inyección de entrada**: sustituye a RCON |
| **F7** (banco de hooks en el Mac) | 3–5 días | Sube a prioridad: es donde se valida R0 sin la Pico |

### R0 — El puente sin RCON (nuevo, y es el trabajo de verdad)

1. **Escena**: en el hook de `draw()`, acumular por frame la lista
   `(tipo, prototipo, x, y, dirección, altura)` y mandarla por el socket XFV1
   junto al frame. Coste: unos pocos kB por frame.
2. **Entrada**: traducir el rayo de la mano a **posición del mapa** y sintetizar el
   clic con `SDL_PushEvent` (y el teclado con `SDL_PushEvent` de teclado para
   escribir). El motor se encarga de todo lo demás.
3. **Comandos que no son clic** (investigar, cambiar de máquina, abrir la GUI):
   o bien la GUI real del juego (V1, y entonces es un clic más), o bien
   `CommandProcessor::executeSilentCommand` desde dentro (con el cuidado de
   construir un `CommandSource` válido).
4. **Cerrar RCON**: `autostart::tick_rcon()` deja de abrir el puerto; el puente
   Lua (`xfactorvr_bridge`) pasa a ser opcional (útil para lo que el inyector no
   ve, como el estado del terreno o la lista de recetas).
5. **Bancos**: `test_stream_render.sh` (ya existe) verifica el estéreo;
   falta uno nuevo: *la entrada inyectada llega al motor y construye*.

---

## 6. Riesgos nuevos que aparecen (y los viejos que se van)

| Riesgo | Mitigación |
|---|---|
| El motor no llega a 90 FPS con Turnip → judder en el estéreo alterno | `XR_EXT_frame_synthesis`; y si no llega, V2 (el motor headless no dibuja nada y el visor va a 90 Hz con geometría propia) |
| El cliente gráfico no termina de cargar la partida (`NOTAS` §3, abierto) | El hook de `processEvents` ya fuerza el avance de la carga; si no basta, `SDL_PushEvent` de un evento de ventana (la pista #2 de las notas) |
| Capturar el framebuffer del motor en la Pico consume ancho de banda | XFV1 va por TCP local; 1280×720 RGB son ~2,7 MB/frame → a 60 FPS, 165 MB/s: **hay que bajar a RGBA con compresión o bajar resolución**; o enviar solo el frame y que el visor lo repita |
| La GUI del juego es un póster plano (no espacial) | Aceptable: un panel plano es un panel plano (es lo que hace GZH-XR con sus paneles) |

**Se van**: el snapshot por RCON, la predicción/interpolación de estado, la
reconstrucción manual de la lógica del juego (cintas, fluidos, GUI), y el
"cliente gráfico en la Pico" como opción de segunda.

---

## 7. Cómo se valida

- `scripts/test_stream_render.sh` — el estéreo alterno (ya existe).
- **Nuevo**: `scripts/test_entrada_inyectada.sh` — el rayo de VR produce un clic y
  el motor construye (esto es lo que prueba que el juego sigue siendo el juego).
- **Nuevo**: `scripts/test_escena_inprocess.sh` — la escena que llega al visor
  coincide con lo que el motor dibuja (se compara contra una captura del frame).
- `scripts/test_modelo3d.sh` — los modelos 3D en el contexto del motor.
- `cargo test` (raíz 18 + inyector 26) — la suite, con los 10 doctests arreglados.
- **F0**: el número de FPS del cliente gráfico con Turnip en la Pico.

---

## 9. Lo que ya está implementado (V1, parte de entrada)

### 9.1 El puente de entrada: `injector-linux/src/entrada.rs`

El motor **no necesita RCON para las órdenes**: recibe un ratón y decide. Eso
exige tres cosas, y las tres están resueltas con los símbolos del propio motor
(`.symtab` de la build 2.1.22 ARM64, **no está stripeado**):

| Necesidad | Símbolo en el motor | Uso |
|---|---|---|
| Mover el cursor | `SDL_SendMouseMotion` | El ratón del motor va al pixel del clic |
| Saber la ventana | `SDL_GetMouseFocus` → `SDL_GetWindowID` | El `windowID` que SDL exige en el evento |
| Mandar el botón | `SDL_PushEvent` (y `_REAL` en builds como la de Termux) | `SDL_MouseButtonEvent` con el reparto de bytes de SDL2 |
| Que el motor convierta el pixel en casilla | `GameView::getMapPosition(PixelPosition)` | **Ya lo hace el motor**: es la función que hookea el mod 3D en Windows |

El último punto es el que hace que esto sea tan poco trabajo: **no hay que
saber la cámara**. El motor ya sabe pasar de pixel a casilla.

**El flujo completo**:

```text
   mano del VR -> rayo -> golpea el plano de la mesa (el frame del motor)
                -> (u,v) de la textura -> pixel (x,y) de la imagen
                -> socket "XFCE" -> inyector
                -> SDL_SendMouseMotion + SDL_PushEvent(boton)
                -> el motor llama a GameView::getMapPosition
                -> el motor decide: selecciona, construye, investiga, deshace
```

- **Protocolo "XFCE"** (TCP, el inyector es el servidor): `magic u32` + `tipo u8`
  + `tamano u16` + carga. El tipo 1 es el clic: `x u16`, `y u16`, `boton u8`,
  `accion u8` (abajo / arriba / clic).
- El inyector **escala** el pixel de la imagen transmitida (640×360 por defecto)
  al framebuffer real (1280×720): el tamaño real lo publica la captura del frame
  (`modelo3d.rs` → `entrada::fijar_viewport`).
- **Variable de entorno**: `XFACTORVR_ENTRADA_PUERTO` (si no está, no hace nada).
- Tests: 4 en el crate del inyector (protocolo, escala y formato del evento SDL).

### 9.2 El spike de GPU: `scripts/spike_gpu_pico.sh`

Un comando que contesta la pregunta que decide V1: **¿hay GPU real y cuántos
FPS da?** Hace, en el dispositivo:

1. Root (`su`), `/dev/kgsl-3d0`, `gpu_model` (¿Adreno 740?), SELinux.
2. Que **Turnip** esté en el payload (`libvulkan_freedreno.so`, lo trae el
   paquete `mesa-vulkan-drivers`).
3. Lanza el cliente gráfico con `XFACTORVR_USE_TURNIP=1` y lee la línea
   `Initialised OpenGL` del motor: si dice **Adreno**, Turnip arrancó dentro
   del proceso glibc (y no hay problema de dos libc, porque Turnip habla con el
   kernel por KGSL, no con la libc de Android).
4. Mide los **FPS del motor** con el contador de frames del inyector.

`scripts/start_factorio_client.sh` tiene el modo nuevo `XFACTORVR_USE_TURNIP=1`,
que escribe el `.json` del ICD de Turnip con la ruta del dispositivo (los `.json`
del payload apuntan a `/usr/lib/...`, que en la Pico no existe) y lo usa en vez
de lavapipe.

### 9.3 El banco: `scripts/test_entrada_inyectada.sh` y `scripts/enviar_clic.py`

El banco levanta el cliente real en el contenedor con el puente de entrada
activo, manda un clic por el protocolo XFCE y comprueba que **el clic entra en
la cola de eventos del motor**. `enviar_clic.py` es el mismo cliente, para
mandar clics a mano o desde el banco.

**Estado: validado en el contenedor** (motor 2.1.22 ARM64 real, sin RCON):

```text
=== 3. Lanzando el cliente con el puente de entrada ===
    OK: [xfv-injector] info: entrada: escuchando en 127.0.0.1:27021
=== 4. Mandando un clic por el protocolo XFCE ===
    OK: [xfv-injector] info: entrada: clic #1 en (320,180) boton=1 accion=3
=== 5. Resultado ===
OK: el puente de entrada mete el clic en la cola de eventos del motor.
```

Esto demuestra la cadena completa de la entrada: el inyector encuentra las
funciones de SDL **dentro del motor real** (por su `.symtab`), monta el
listener, recibe el clic, lo escala al tamaño de la ventana y lo mete en la
cola de eventos con el `windowID` correcto.

Falta un nivel (el 3): comprobar que el motor **actúa** sobre el clic (el
jugador se mueve, aparece el fantasma de construcción). Eso necesita el motor
en juego y **con ventana con foco**, que en el contenedor no hay (SDL va
`offscreen`); se valida en la Pico con el X de Termux.

### 9.4 Lo que falta en el VISOR para cerrar V1

El visor **todavía no habla con este puente** (sigue usando el puente RCON
para las acciones). Falta:

1. **Enviar el clic**: el rayo de la mano golpea el plano de la mesa (el frame
   del motor), sale un (u,v) de la textura → un pixel de la imagen → un mensaje
   XFCE al puerto del puente. El visor ya tiene el hilillo que recibe los
   frames (`game_receiver_thread`); el de entrada es simétrico y va en la
   dirección contraria.
2. **El panel como diorama horizontal**: hoy el frame del motor se pinta como
   un **quad vertical** al fondo de la mesa (`mat4_translate(..., JUEGO_ALTURA,
   JUEGO_PROFUNDIDAD)`). Para "la fábrica sobre mi mesa" tiene que ir **tumbado**
   en la mesa, con el passthrough alrededor, y con la escala de la tabla
   (1 casilla = 32 px en el juego = 0,04 m en la mesa).
3. **El anclaje a la mesa real** (`XR_BD_spatial_anchor`, ver `PLAN-PICO-4-ULTRA.md` F1).
4. Ojo por ojo: el visor **ya tiene** las dos ranuras (`s_game_frame[2]`) y el
   flag `s_juego_estereo` para repartir las vistas alternas del motor, así que
   el estéreo engine-side está del lado del visor ya resuelto.

#### 9.4.1 Reparto de trabajo (2026-10-22, en curso)

Los tres puntos anteriores se hacen en paralelo, cada uno en ficheros **nuevos
y disjuntos**, y después se integran en `openxr_engine.cpp`:

| Módulo | Ficheros | Qué resuelve |
|---|---|---|
| `puente_entrada` | `android/src/puente_entrada.{h,cpp}` + banco de escritorio | Cliente XFCE: conectar, mandar clic y reconectar si el motor se reinicia |
| `panel_mesa` | `android/src/panel_mesa.{h,cpp}` + banco de escritorio | Geometría del quad **tumbado** y el cálculo **rayo → pixel** (puro, testeable) |
| `ancla_espacial` | `android/src/ancla_espacial.{h,cpp}` + `docs/ADR-014-anclaje-a-la-mesa.md` | Anclaje a la mesa real con `XR_BD_spatial_anchor` (y persistencia), con fallback a STAGE |

Después, una cuarta pasada integra los tres en `android/src/openxr_engine.cpp`
(hilo de entrada, quad tumbado, matriz del ancla) y en `scripts/build_apk.sh`
(que lista las fuentes de forma explícita).

**Convenciones de píxel (fijas, para que la integración no se equivoque)**:
el motor dibuja con la Y invertida, así que en el framebuffer la **fila 0 es la
de ARRIBA**; el quad tiene la **V invertida** (v=0 = arriba de la imagen); y el
inyector escala el pixel de la imagen transmitida al tamaño de la ventana
contando **desde arriba**, que es como cuenta el ratón de SDL.

**Hallazgo del worker del anclaje**: los headers OpenXR del repo
(`android/include/openxr`, Khronos 1.1.45) **no traen ninguna de las
extensiones BD** (`XR_BD_spatial_anchor`, `spatial_plane`, `spatial_sensing`,
`spatial_scene`), ni el SDK de PICO 3.0.0 las incluye. Sí están en el registro
oficial de Khronos (`xr.xml`), del que se transcribió la ABI mínima con sus
valores numéricos exactos dentro de `ancla_espacial.cpp`. Consecuencia
práctica: **hay que PEDIR esas extensiones al crear la instancia OpenXR**, si no
el runtime devuelve `XR_ERROR_FEATURE_UNSUPPORTED` y el módulo se queda inactivo
(en fallback a STAGE, sin romper nada).

**Estéreo engine-side: verificado**. El inyector manda el ojo al que pertenece
cada frame en el **bit 0 de `flags`** de la cabecera XFV1 (`empaquetar(...,
ojo & 1, ...)`) y el visor lo usa como ranura (`ranura = (flags & 1) ? 1 : 0`),
poniendo `s_juego_estereo = true` cuando han llegado las dos vistas. El
desplazamiento de UV "TV 3D" queda solo como respaldo cuando el motor no puede
desplazar la cámara.

## 10. Referencias internas

- `ADR-002` — cliente gráfico Zink y mod 3D.
- `ADR-003` — el inyector Linux ARM64 (trampolines por símbolo).
- `ADR-009` — "render real al visor".
- `ADR-012` — diseño del visor (T1–T4) y la tabla de hooks 10/17.
- `PLAN-PICO-4-ULTRA.md` — el plan por fases (F0–F7), revisado por este ADR.
- `NOTAS-carga-y-campana.md` — el bloqueo de carga del cliente gráfico.
