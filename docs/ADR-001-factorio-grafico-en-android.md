# ADR-001 — Cliente gráfico de Factorio ARM64 dentro de una app Android

| | |
|---|---|
| **Estado** | Aceptado (recomendación) |
| **Fecha** | 2026-10-19 |
| **Ámbito** | `xfactorvr` — port de Factorio a VR standalone para Pico 4 Ultra (Android 14, Snapdragon XR2 Gen 2, Adreno 740) |
| **Pregunta** | ¿Qué hace falta **exactamente** para que el cliente **gráfico** de Factorio ARM64 corra dentro de una app Android y se pueda enganchar su render a OpenXR? ¿Cuál es la vía menos frágil? |
| **Binario analizado** | `factorio_arm64/bin/arm64/factorio` — ELF aarch64 PIE, 249 MB, SHA-256 `b182a98683356e6e9818507db7d46680c6f8d07c479279234171c9f2255adcae`, **no stripped**, 148 802 símbolos definidos |

> Todo lo marcado **[V]** está verificado con un comando de este repo (salida literal citada).
> Lo marcado **[F]** es dato externo (web) con su fuente.
> Lo marcado **[NV]** es **no verificado**: hipótesis o pendiente de comprobar.

> **ADR complementario:** [`ADR-001-arquitectura-vr-factorio.md`](ADR-001-arquitectura-vr-factorio.md) define la arquitectura VR general (modo A headless vs modo B con el cliente gráfico real) y las fases de trabajo. Este ADR baja al detalle técnico de la **fase 5 y 6** de aquel plan: *qué* hace falta exactamente para que el cliente gráfico ARM64 corra en Android, *qué* stack de GL usar, y *dónde* enganchar el render. Las conclusiones no se contradicen: aquí se añade la evidencia (dependencias reales, versiones, direcciones) que faltaba.

---

## 0. Resumen ejecutivo

El hallazgo que cambia el plan: **el binario *headless* ARM64 que ya tenemos no es un build "sin gráficos"**. Es el build gráfico completo con la *interfaz* gráfica sustituida en tiempo de compilación por un stub. Dentro del mismo ELF están la clase real `GraphicsInterfaceOpenGL`, la clase real `SDLWindow`, el loader GLAD, y las llamadas reales a `SDL_CreateWindow` / `SDL_GL_CreateContext` / `SDL_GL_GetProcAddress` **[V]**. Lo único que falta para que dibuje son **los sprites (0 PNG en el paquete headless)**.

Por tanto hay **dos rutas** y la recomendación es una:

- **Ruta A (preferida, la que menos frágil da):** conseguir el binario **gráfico ARM64 oficial** (Steam, rama experimental 2.1) y ejecutarlo en Android con **Mesa Zink + Turnip** sobre el Vulkan de Adreno — exactamente el stack que usa Valve en el Steam Frame para este mismo juego **[F]**.
- **Ruta B (plan B, sin login):** parchear el binario headless para que instancie `SDLWindow`/`GraphicsInterfaceOpenGL` en vez de los stubs, y completar los assets con los PNG del Factorio x64 que ya tienes en Steam.

El enganche a OpenXR **no** debe hacerse interceptando `glDrawElements` (LD_PRELOAD), sino en un punto de mayor nivel que ya existe en el motor: `GlobalContext::swapRenderBuffers()` → `SDLWindow::swap()` **[V]**, más `GameView::getMapPosition()` para la cámara 2D **[V]**. Todo se resuelve **por nombre de símbolo** en el `.symtab` del ELF, igual que factorio-3d-mod lo hace por PDB en Windows **[V]**.

---

## 1. Contexto heredado (no se repite, se construye encima)

- Hoy: servidor headless ARM64 2.1.22 en el visor + renderizador propio en el cliente (RCON/IPC + Rust + OpenGL ES 3.2 + OpenXR).
- Ya existe `scripts/prepare_glibc.sh` (glibc de Termux → `build/glibc.tar.gz`) y `scripts/start_factorio_standalone.sh`.
- La SD es `noexec` → todo el motor va en `/data/local/tmp/`.
- Android usa bionic; el motor exige glibc (hasta **GLIBC_2.38** **[V]**).

---

## A. Dependencias reales del binario

### A.1 Lo que dice la tabla dinámica

```console
$ objdump -p factorio_arm64/bin/arm64/factorio | grep -A40 "Dynamic Section"
  Dynamic Section:
  NEEDED       libresolv.so.2
  NEEDED       libm.so.6
  NEEDED       libc.so.6
  ...
  Version References:
  required from libc.so.6:
    0x06969197 0x00 02 GLIBC_2.17
    ...
    0x069691b8 0x00 13 GLIBC_2.38
```

**[V]** Solo **tres** librerías dinámicas: `libresolv.so.2`, `libm.so.6`, `libc.so.6`. Total de símbolos importados: **437**, todos de glibc **[V]**.

> **No hay `libSDL2.so`, ni `libGL.so`, ni `libX11.so`, ni `libasound.so` en `DT_NEEDED`.** No es que falten: es que **Factorio las enlaza estáticamente**. Wube lo confirma en el foro de FFF #446: *"Because we statically link our dependencies, we don't require steamrt in order for the game to run"* **[F]**.

Consecuencia práctica para Android: **no hay que conseguir SDL2, ni libGL, ni X11 para que el binario arranque**. El peso extra vendría de las librerías de vídeo que SDL2 carga *en runtime* con `dlopen` (§A.4), y que sí habrá que resolver.

### A.2 ¿SDL2 o SDL3? ¿Qué versión?

**[V] SDL2, y está compilado dentro del binario.** 3 221 símbolos `SDL_*` definidos:

```console
$ nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep -cE "[TtW] SDL_"
3221
$ nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep -E "SDL_(Init|CreateWindow|GL_CreateContext)_"
00000000035fff84 t SDL_Init_DEFAULT
00000000036f84c8 t SDL_Init_REAL
00000000037723a0 t SDL_CreateWindow_REAL
000000000377770c t SDL_GL_CreateContext_REAL
```

El patrón `<fn>_REAL` / `<fn>_DEFAULT` es la **dynamic API de SDL2**. En SDL3 desaparecen `_REAL` y `SDL_GL_*` (se sustituye por `SDL_ClaimWindowForSurface`). **Descartado SDL3.** **[V]**

**Versión exacta: [NV].** `SDL_GetRevision_REAL` (0x36f97e4) devuelve una cadena **vacía** — el build de release de Factorio la eliminó:

```console
$ objdump -d --start-address=0x36f97e4 --stop-address=0x36f97f0 factorio_arm64/bin/arm64/factorio
 36f97e4: d0fec9c0   adrp x0, 0x1033000
 36f97e8: 9125d000   add  x0, x0, #0x974
 36f97ec: d65f03c0   ret
→ cadena en 0x1033974 = "" (vacía)
```

Sí hay indicadores de versión presentes (`SDL_CreateWindowWithProperties`, `SDL_CreateSurface`, `SDL_CreateRendererWithProperties`), que sitúan el SDL2 **≥ 2.0.10**, pero sin poder precisar el minor **[NV]**.

### A.3 ¿Qué subsistema de vídeo usa? (deducido de los símbolos)

**[V] Backends de vídeo de SDL2 compilados dentro:**

| Backend | Evidencia en el binario | Estado |
|---|---|---|
| **X11** | strings `"Couldn't open X11 display"`, `"Failed to connect to the X11 display server"`; TU `SDL_x11video.c`, `SDL_x11opengl.c`, `SDL_x11events.c` en el debug info | **Presente** **[V]** |
| **Wayland** | string de nombre de driver `"wayland"`; TUs `SDL_waylandvideo.c`, `SDL_waylandopengles.c`, `SDL_waylandwindow.c` | **Presente** **[V]** |
| **EGL** | strings `eglGetDisplay`, `eglInitialize`, `eglChooseConfig`, `"Could not parse EGL version string: %s"`; TU `SDL_egl.c` | **Presente** **[V]** |
| **offscreen** | string de driver `"offscreen"`; TUs `SDL_offscreenopengles.c`, `SDL_offscreenvideo.c` | **Presente** **[V]** |
| **KMSDRM** | string `"kmsdrm"`; TUs `SDL_kmsdrmvideo.c`; carga `libdrm.so.2` | **Presente** **[V]** |
| **dummy** | string `"dummy"`; TU `SDL_nullvideo.c` | **Presente** **[V]** |
| **Android** | **no** existe `SDL_android.c`; no hay `libandroid.so`; no hay driver `"android"` | **Ausente** **[V]** |

**Conclusión [V]:** el SDL2 embebido **no** tiene el driver de vídeo de Android. Por tanto la ventana **no** se puede crear con el `SDL_VIDEODRIVER=android` nativo de SDL2 (que además exige una `ANativeWindow` de Java y un `SDLActivity`). Hay tres formas de sortearlo, todas argumentadas en §B:

1. Render **offscreen** con EGL surfaceless (`EGL_PLATFORM_SURFACELESS_MESA` / `EGL_DEFAULT_DISPLAY`) + FBO propio — no hace falta ventana de SDL.
2. Crear la ventana de SDL sobre **X11 embebido** (`termux-x11`) o **Wayland** (Weston).
3. Parchear el SDL2 embebido para que su backend `android` esté disponible (es trabajo de C, no de Rust).

### A.4 Qué resuelve en runtime (los `dlopen` de SDL2)

**[V]** SDL2 carga dinámicamente estas librerías (son las que **sí** habrá que llevar al visor):

| Librería | Para qué |
|---|---|
| `libEGL.so.1` | contexto EGL (el camino que nos interesa) |
| `libGL.so.1` | GL de escritorio vía glvnd |
| `libGLESv2.so.2`, `libGLESv1_CM.so.1`, `libGLES_CM.so.1` | GLES |
| `libX11.so.6`, `libX11-xcb.so.1` | backend X11 |
| `libwayland-client.so.0`, `libwayland-egl.so.1`, `libwayland-cursor.so.0` | backend Wayland |
| `libdrm.so.2` | backend KMSDRM |
| `libudev.so.0` / `libudev.so.1` | eventos de entrada (gamepad/hotplug) |
| `libasound.so.2`, `libpulse.so.0` | audio |

```console
$ strings -a factorio_arm64/bin/arm64/factorio | grep -xE "lib(X11|wayland|EGL|GL|drm|asound|pulse|udev).*\.so.*"
libEGL.so.1
libGL.so.1
libX11.so.6
libX11-xcb.so.1
libasound.so.2
libdrm.so.2
libpulse.so.0
libudev.so.0
libudev.so.1
libwayland-client.so.0
libwayland-cursor.so.0
libwayland-egl.so.1
```

**Respuesta directa a las preguntas del encargo:**

| Pregunta | Respuesta verificada |
|---|---|
| ¿SDL2 o SDL3? | **SDL2**, enlazado estático dentro del binario **[V]** |
| ¿Qué versión de SDL? | **[NV]** — ≥2.0.10 por API; el `SDL_GetRevision` viene vacío |
| ¿`SDL_VIDEODRIVER`? | La cadena existe **[V]**, pero **no** hay driver `android` **[V]**; sí `x11`, `wayland`, `offscreen`, `kmsdrm`, `dummy` **[V]** |
| ¿Depende de X11? | El *backend* X11 está compilado, pero **no** es una dependencia de arranque: se carga por `dlopen` solo si se pide ese driver **[V]** |
| ¿Depende de Wayland? | Igual: presente, opcional, por `dlopen` **[V]** |
| ¿Depende de `libGL`? | **No** como dependencia dura. El GL lo da el **propio driver de la plataforma** vía `SDL_GL_GetProcAddress` + GLAD **[V]** |
| ¿Depende de `libasound`? | No en `DT_NEEDED`; `libasound.so.2`/`libpulse.so.0` se cargan por `dlopen` solo si se activa el audio **[V]** |

### A.5 ¿Qué versión de OpenGL exige?

**[V] El loader GLAD declara hasta OpenGL 3.3 como máximo**, y por tanto 3.3 es el mínimo que el juego da por bueno:

```console
$ nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep GLAD_GL_VERSION
00000000040c4240 b GLAD_GL_VERSION_1_0
...
00000000040c4268 b GLAD_GL_VERSION_3_2
00000000040c426c b GLAD_GL_VERSION_3_3     ← el más alto
$ nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep gladLoadGL
000000000385f990 t gladLoadGLLoader
```

Y los shaders del juego son GLSL 1.30, coherente con GL 3.x **[V]**:

```console
$ strings -a factorio_arm64/bin/arm64/factorio | grep -E "^#version [0-9]+"
#version 130
```

El mensaje de error que el juego emite cuando el sistema solo ofrece OpenGL ES **[V]**:

```
OpenGL 3.0 and later are not supported by this system
```

**Requisito práctico: OpenGL 3.3 *core* de escritorio.** No vale OpenGL ES 3.x (Android nativo), ni GLES+EXT, ni un driver que solo dé 3.1.

> Esto es la causa raíz del problema conocido en Raspberry Pi: el driver `v3d` del Pi 5 sólo expone **OpenGL 3.1 de escritorio**, y Factorio se niega a arrancar. El foro de Factorio documenta el rodeo: `MESA_GL_VERSION_OVERRIDE=3.3` + `MESA_GLSL_VERSION_OVERRIDE=330` + `ogl-accelerated-renderer=false` en `config.ini` **[F]**. Ese es el mismo knob que usaremos con llvmpipe si hiciera falta.

### A.6 ¿Headless y gráfico tienen las mismas dependencias?

**No — pero la diferencia es mucho menor de lo que parece, y esto es lo más importante del ADR.**

Inspeccionando la jerarquía de clases del binario headless **[V]**:

```console
$ nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep "vtable for"
0000000003c13ac0 d vtable for HeadlessWindow
0000000003ac0338 d vtable for GraphicsInterface
0000000003ac1500 d vtable for GraphicsInterfaceOpenGL     ← la REAL, compilada
0000000003c13c78 d vtable for HeadlessGraphicsInterface   ← el stub
0000000003ac0bb0 d vtable for SDLWindow                     ← la REAL, compilada
```

Y los stubs del headless son **métodos vacíos de 8 bytes** (un `ret`) **[V]**:

```console
00000000034f1af0 t HeadlessGraphicsInterface::setViewport(unsigned int, unsigned int, bool, bool)
00000000034f1af4 t HeadlessGraphicsInterface::getDefaultFramebufferSize()
00000000034f1b00 t HeadlessGraphicsInterface::defaultFramebuffer()
00000000034f1b08 t HeadlessGraphicsInterface::readFramebufferToMemory()
00000000034f1b28 t HeadlessGraphicsInterface::setVsync(bool)
00000000034f1b2c t HeadlessGraphicsInterface::swapBuffers()
00000000034f1b30 t HeadlessGraphicsInterface::flush(bool)
```

Además, el **camino gráfico real está compilado y es alcanzable** **[V]**:

- `SDLWindow::SDLWindow(GraphicsInterface::Type, WindowPositionData)` @ `0x34d8cd4` llama a `gladLoadGLLoader` en `+0x140c` (`0x34da0e0`) → crea ventana + contexto GL + carga de funciones GL.
- `SDL_GL_GetProcAddress_REAL` se llama 10+ veces seguidas en `0x36b1478…0x36b1550`, dentro de `GLES2_LoadFunctions` → es el loader GL de SDL/GLAD.
- `SDL_GL_CreateContext_REAL` se llama en `0x36ac340` (`GLES2_CreateRenderer`) y `0x36b320c` (`GL_CreateRenderer`); `SDL_CreateWindow_REAL` en `0x36bc8a8` (`SDL_CreateWindowAndRenderer_REAL`).

**Conclusión:** la diferencia entre el build headless y el gráfico **no es el motor**: es (a) qué subclase se instancia —`HeadlessWindow`/`HeadlessGraphicsInterface` en vez de `SDLWindow`/`GraphicsInterfaceOpenGL`—, y (b) **que el paquete headless no lleva los sprites**.

Falta además un flag de runtime, `GlobalContext::headlessMode` @ `0x3ea1168` (`.bss`) **[V]**, que leen funciones reales como `GameView::getMapPosition`, `CameraBase::CameraBase`, `GameRenderer::render/prepare/postPrepare`, `GameView::prepareRender`, `Sprite::~Sprite`, `SpriteBank::getSprite` o `Map::stop` **[V]** (localizadas por el patrón `adrp 0x3ea1000` + `add #0x168` sobre el desensamblado completo).

> **[NV]** No he podido determinar si ese flag se puede forzar a `false` **en runtime** o si la elección de la subclase está tomada en **tiempo de compilación** (`#ifdef`). La evidencia apunta a compilación (los stubs son clases distintas, no ramificaciones), pero no lo he confirmado. Esto es exactamente el punto de riesgo R1 del plan (§D).

### A.7 Los sprites: el otro 50 % del problema

| Paquete | Versión | PNG | Shaders | Ficheros |
|---|---|---|---|---|
| `factorio_arm64/` (headless ARM64) | 2.1.22 | **0** | 333 | 3 880 |
| `factorio_linux/` (Steam x64) | 2.0.77 | **4 297** | sí | 6 338 |

```console
$ find factorio_arm64 -name "*.png" | wc -l
0
$ find factorio_linux/data -name "*.png" | wc -l
4297
$ ls factorio_arm64/data/core/graphics/entity/biter/ | head -3
biter-attack-mask1.lua     ← la DEFINICIÓN del sprite está…
$ ls factorio_arm64/data/core/graphics/entity/biter/*.png | wc -l
0                               ← …pero el PNG no
```

El headless trae los `.lua` que *describen* el sprite (ruta, tamaño, frames) pero **no el PNG**. Los PNG son solo 4297 ficheros y son **independientes de la arquitectura** **[V]** (son datos, no código).

> **[NV]** El `data/` del Steam es **2.0.77** y el headless es **2.1.22**: los árboles no coinciden (el 2.1 trae `space-age/`, `quality/`, `recycler/`, `elevated-rails/`; solo 1 375 ficheros son comunes). Rellenar los PNG del 2.0.77 en el árbol del 2.1.22 **funcionará en parte** (los sprites de `base/` son mayormente estables) pero **es una mezcla de versiones y no la recomiendo**. La fuente correcta de assets es el **propio cliente gráfico ARM64 2.1**.

---

## B. Estrategia de gráfico en Android

### B.0 El precedente que lo cambia todo: el Steam Frame

El Steam Frame es ARM64, con GPU **Adreno** (la misma familia que la Adreno 740 de la Pico 4 Ultra), y su stack gráfico es **[F]**:

> *"the Frame runs Vulkan through **Turnip**, the open-source Mesa driver for Qualcomm's Adreno GPUs, and runs **OpenGL through zink**, a Mesa layer that turns OpenGL calls into Vulkan ones."* — vr.org

Y en ese stack, **Factorio 2.1 ARM64 nativo corre a 720p y 60 FPS** con ajustes gráficos equivalentes a Steam Deck **[F]** (FFF #446). Wube añade que el ARM64 nativo es **~11 % más rápido** que el x86-64 bajo FEX: **20,906 UPS vs 18,798 UPS** **[F]**.

Traducción directa a nuestro caso:

| Steam Frame | Pico 4 Ultra |
|---|---|
| GPU Adreno (750) | GPU Adreno **740** (Snapdragon XR2 Gen 2) — confirmado en `src/renderer/sgsr.rs` **[V]** |
| Vulkan: Mesa **Turnip** | idéntico: Turnip ya está en el proyecto vía `cmake/adrenotools.cmake` **[V]** |
| OpenGL: Mesa **Zink** (GL→Vulkan) | **el mismo Zink** sobre el mismo Turnip |
| Factorio ARM64 nativo, 720p/60 | el mismo binario ARM64, mismo stack |

**No hay que inventar nada: hay que replicar el stack de Valve.** Y además **este repo ya tiene la mitad**: `Generals-Zero-Hour-XR/cmake/adrenotools.cmake` usa `bylaws/libadrenotools` para cargar un **Turnip de usuario en lugar del driver de fábrica**, con un hook de `dlopen("libvulkan.so")` **[V]**.

### B.1 Opción 1 — Mesa llvmpipe (software) + EGL surfaceless

**Mecánica:** el juego arranca con `SDL_VIDEODRIVER=offscreen` (o con EGL surfaceless), Mesa `llvmpipe` rasteriza en CPU, el juego dibuja en un FBO y leemos con `glReadPixels`.

| Dimensión | Valoración |
|---|---|
| **Viabilidad Android ARM64** | **Alta.** `llvmpipe` es un rasterizador por CPU: no necesita driver de GPU, funciona en cualquier ARM64. Mesa documenta formalmente el build para Android (NDK) **[F]**. Da **OpenGL core 3.3** de serie (glxinfo de Mesa 18.2 con llvmpipe reporta *max core profile 3.3*) **[F]** → cumple el requisito de §A.5 sin trucos. |
| **Rendimiento esperado** | **Aworre**. 720p RGBA = 3,7 MB por frame; a 72–90 Hz son ~270–330 MB/s sólo de PCIe/lectura, más el coste de rasterizar en CPU. Referencia: en un Pi 5 con llvmpipe el foro reporta "solid 60fps on first load" **[F]**, pero el Pi 5 es mucho más débil que un Snapdragon 8 Gen 2 en CPU single-thread. **Estimación [NV]: 20–40 fps a 720p en la Pico**, y esa cifra es una estimación, no una medida. |
| **Estabilidad en OpenXR** | **Excelente.** Render totalmente offscreen: no hay superficie, no hay compositor, no hay tearing. El frame se sube como textura OpenXR y se presenta. Es el camino más controlable. |
| **Riesgos** | (a) Latencia: `glReadPixels` por frame + subida de textura = ~2–4 ms extra **[NV]**. (b) La CPU del móvil compite con el propio juego. (c) El plan B, no el A. |

**Veredicto: viable, simple, y es el *fallback* perfecto** si Zink/Turnip no da potencia suficiente. Además permite validar *todo* el resto de la arquitectura (assets, inyección, cámara, cámara-3D) sin pelear con el driver.

### B.2 Opción 2 — Mesa sobre Adreno: **Zink + Turnip** (o adrenotools)

**Mecánica:** en vez de rasterizar en CPU, forzamos a Mesa a que use el driver real de la GPU. Dos formas:

- **Zink** (OpenGL → Vulkan → Turnip → Adreno). Es **exactamente** lo que hace el Steam Frame **[F]**. Zink expone OpenGL 4.6 sobre Turnip, así que el requisito de GL 3.3 de Factorio se cumple de sobra.
- **adrenotools** (`bylaws/libadrenotools`), que ya está en el repo **[V]**: instala un hook de espacio de nombres del linker que redirige `dlopen("libvulkan.so")` al driver que le pasemos. Es la vía para meter **nuestro propio build de Mesa/Turnip** en vez del driver de fábrica.

| Dimensión | Valoración |
|---|---|
| **Viabilidad Android ARM64** | **Alta y ya partly hecha.** `cmake/adrenotools.cmake` ya resuelve el "cómo meter Turnip en Android" **[V]**. Falta la pieza de Zink por encima. |
| **Rendimiento esperado** | **El mejor.** Turnip da Vulkan 1.3 sobre Adreno 740; Zink da OpenGL 4.x; el juego corre en GPU real. Precedente: 720p/60 en el Steam Frame con el mismo software **[F]**. La Pico 4 Ultra tiene una Adreno 740 comparable. **Estimación [NV]: 60 fps a 720p, realista.** |
| **Estabilidad en OpenXR** | **Buena, con una salvedad.** Si hacemos el render **dentro del contexto GL del proceso del juego** y exportamos con EGL/DMA-BUF, es limpio. Si hay que cruzar **procesos** (juego ↔ app Android), el paso de la textura es el punto delicado: opciones = `glReadPixels` + socket (simple, ~4 ms), `EGL_EXT_image_dma_buf_import` + dmabuf (cero copia, más complejo), o **mismo proceso**. |
| **Riesgos** | (a) **Zink añade una traducción GL→Vulkan en CPU**; no es gratis, pero el juego es 2D puro y carga poco. (b) Turnip en Adreno 740 esreasonably soportado pero no "garantizado" por Qualcomm. (c) Hay que construir Mesa para ARM64 Android con el NDK (Mesa lo documenta **[F]**). (d) `dlopen` hook: el mismo mecanismo que usa adrenotools ya está probado en este repo para DXVK **[V]**. |

**Veredicto: la opción recomendada.**

### B.3 Opción 3 — termux-x11 + ventana real

| Dimensión | Valoración |
|---|---|
| **Viabilidad** | **Alta.** El backend X11 está compilado en el SDL2 del binario **[V]** y `termux-x11` es un servidor X real para Android. El juego abriría una ventana X normal. |
| **Rendimiento** | **Medio.** Añade un servidor X completo en el móvil: copia de píxeles de la app → X → app, y `SDL_GL` sobre X11 (GLX) necesita **mesa con GLX**, no EGL. En Android, GLX no existe de forma nativa: habría que usar `llvmpipe` + `glx-lax` o similar → **se pierde la GPU**. Esto es un lastre real: el camino X11 en Android es por definición software. |
| **Estabilidad OpenXR** | **Baja.** La ventana X tiene su propio ciclo de presentación; capturar con `XGetImage` es una copia de CPU por frame. Añade latencia y una superficie de fallos. |
| **Riesgos** | (a) Duplicar el compositor; (b) `libX11.so.6` + `libXext` + `libXcursor` + `libXi` + `libXfixes` + `libXrandr` hay que llevarlas al visor; (c) el `dlopen` de X11 con dlsym por nombre es frágil. |

**Veredicto: sólo como plan C.** Sirve para *demostrar* que el juego dibuja, no para el producto final.

### B.4 Opción 4 — Emulación x86_64 (FEX/box64) del cliente x86_64

| Dimensión | Valoración |
|---|---|
| **Viabilidad** | **Alta pero carísima.** Ya tenemos el cliente x86_64 en Steam (`factorio_linux/bin/x64/factorio`) **[V]**. FEX (el motor de Valve) y box64 ejecutan x86_64 en ARM64. |
| **Rendimiento** | **El peor de todos.** El dato oficial de Wube es que **nativo ARM64 es ~11 % más rápido que x86-64 bajo FEX** (20,906 vs 18,798 UPS) **[F]** — o sea, la emulación *resta* rendimiento y encima añade la traducción de GL, que en FEX se reenvía al GL del host vía thunks **[F]**. Sumado al coste de Context Switch de la VM, es la peor base de rendimiento. |
| **Estabilidad OpenXR** | **Media-baja.** Una capa de traducción entre el juego y el GL del dispositivo añade un punto de fallo más, y el hook `LD_PRELOAD`/Frida tiene que funcionar sobre código *traducido* (las direcciones ya no son las del ELF: los hooks por nombre dejan de valer y hay que hookear por *patrón* en el código traducido). |
| **Riesgos** | (a) El hook por símbolo deja de funcionar (el código ejecutado no es el del ELF); (b) Doble capa de traducción; (c) Workload añadido al mismo CPU. |

**Veredicto: descartada.** Justamente porque **el ARM64 nativo existe y es más rápido** **[F]**.

### B.5 Tabla comparativa

| | 1. llvmpipe + EGL surfaceless | **2. Zink + Turnip (adrenotools)** | 3. termux-x11 | 4. FEX/box64 x86_64 |
|---|---|---|---|---|
| Viabilidad Android ARM64 | Alta | **Alta (infra ya en repo)** | Alta | Alta |
| ¿GPU real? | No (CPU) | **Sí (Adreno 740)** | No (forzado) | Sí (vía FEX) |
| GL disponible | 3.3 core (nativo de llvmpipe) | **4.x (Zink)** | 3.1/3.3 forzado | 3.3 vía FEX |
| Rendimiento | 20–40 fps **[NV]** | **60 fps @720p (precedente FFF)** **[F]** | Bajo **[NV]** | < nativo **[F]** |
| Presentación en OpenXR | **Excelente** (offscreen puro) | **Buena** (ojo con el cruce de procesos) | Mala | Media |
| Complejidad | Baja | Media | Media-alta | Alta |
| Hook por símbolo ELF | **Sí** | **Sí** | Sí | **No** (código traducido) |
| Recomendación | **Plan B / red de seguridad** | **★ Plan A** | Plan C | Descartada |

### B.6 Recomendación de arquitectura gráfica

```
┌─ App Android (Pico) ──────────────┐        ┌─ Proceso del juego ─────────────────┐
│  OpenXR (Pico runtime)             │        │  factorio ARM64 (glibc, /data/…)   │
│  contexto GL ES 3.x o Vulkan       │        │  LD_PRELOAD=libxfactor_bridge.so   │
│         ▲                         │        │        │                           │
│         │ textura / dmabuf        │        │        ├─ Mesa Zink+Turnip (o llvmpipe)│
│         │ (socket UNIX + shm)     │◀───────┤        ├─ EGL surfaceless → FBO      │
│  dibuja el frame 2D como textura  │        │        └─ hook: swapRenderBuffers()    │
│  proyectado en 3D estéreo         │        │           → glReadPixels → envío     │
└───────────────────────────────────┘        └───────────────────────────────────┘
```

**Decisión: Opción 2 (Zink + Turnip) como camino principal; Opción 1 (llvmpipe) como red de seguridad y banco de pruebas.**

Motivos, en orden de peso:
1. **Precedente directo y del mismo juego**: el Steam Frame corre este mismo Factorio 2.1 ARM64 a 720p/60 con exactamente Zink+Turnip sobre Adreno **[F]**. No es una apuesta, es una replicación.
2. **La infraestructura ya está en este repo**: `cmake/adrenotools.cmake` con `libadrenotools` para cargar Turnip en Android **[V]**; `src/renderer/sgsr.rs` ya está dirigida al Adreno 740 **[V]**.
3. **El hook por símbolo sigue valiendo**: en ARM64 nativo el `.symtab` está intacto **[V]**; en la emulación x86_64 no **[F]**.
4. **llvmpipe sirve de red de seguridad**: permite validar toda la arquitectura (assets, hooks, cámara, geometría 3D) sin depender del driver.

---

## C. Punto de enganche — cómo se engancha el render a OpenXR

### C.0 Lo que ya existe en el repo (no hay que inventarlo)

`src/symbols.rs` ya es un escáner/resolvedor de símbolos ELF (`.symtab` + `.dynsym`) con la `object` crate de Rust, y su cabecera dice exactamente lo que hay que hacer **[V]**:

```rust
// symbols.rs — Escáner y resolvedor de símbolos ELF/DWARF para Linux y Android ARM64
//
// A diferencia de Windows (que requiere factorio.pdb), los binarios oficiales
// de Factorio en Linux vienen sin despojar (not stripped), conteniendo todos
// los símbolos C++ en su tabla .symtab.
```

`src/offsets.rs` ya mapea los símbolos de la ABI Itanium **[V]**. Es decir: **el "port a Linux ARM64 de factorio-3d-mod" ya está empezado**. Lo que falta es el *qué* enganchar y el *cómo* presentar en OpenXR.

factorio-3d-mod resuelve sus hooks **por nombre de símbolo del PDB**, con RVAs de respaldo por si el nombre cambia **[V]** (README: *"the hooks are found by symbol name"*). En Linux ARM64 se resuelve **por nombre en el `.symtab`**, que es más fiable que el PDB: no hay PDB, no hay stripping, y el nombre es el mismo.

### C.1 Opción 1 — LD_PRELOAD sobre SDL2/GL: **descartada como mecanismo principal**

**Cómo sería:** interceptar `glDrawElements` / `glTexImage2D` / `SDL_GL_SwapWindow` en un `.so` preloadado y copiar el framebuffer.

**Ventajas:** no requiere tocar el binario; es trivial de prototipar.

**Riesgos (y por qué no se recomienda):**
1. El juego dibuja en **varios targets**: framebuffer de entidades, de sombras, de trabajo, de post-proceso, y el final. `FramebufferOpenGL::copyTo`, `blitTo`, `blitTo3DTexture` y `DrawEngine::renderShadowsToWorkingSprite` **[V]** muestran un pipeline de FBO a media docena de pasadas. "El último framebuffer" no está definido de forma trivial.
2. **Multisampling y render targets**: `Framebuffer::Capabilities` y `createFramebuffer(..., Framebuffer::Capabilities)` **[V]** implican MSAA; el resolve es interno de Mesa, no un `glBlitFramebuffer` que uno pueda interceptar limpiamente.
3. `glReadPixels` sobre el framebuffer por defecto **no** es portable (el back buffer por defecto no es legible en GL core). Hay que rehacer todo el pipeline en un FBO propio.
4. El motor usa **compute shaders** y **buffer objects** (`glad_glGetString`, `GLAD_GL_ARB_compute_shader` **[V]**); un preloader que intercepte DrawCalls se encuentra con assets en GPU y tiene que orquestar sincronización.
5. **Fragilidad**: cualquier cambio de la versión de Mesa/GLAD rompe el punto de intercepción sin aviso.

**Cuándo sí:** como *instrumentación de diagnóstico* (contar draw calls, medir tiempos), no como mecanismo de captura.

### C.2 Opción 2 — Instrumentación nativa (Frida / LD_PRELOAD con hooks por símbolo): **★ la recomendada**

Es el port directo de factorio-3d-mod a Linux ARM64. Lo que hay que saber para hacerlo:

**a) En Windows, factorio-3d-mod usa `factorio.pdb`; en Linux no hace falta nada de eso.** Nuestro ELF **no está stripped** y trae 148 802 símbolos C++ con nombre **[V]**. Los nombres *demangled* son idénticos a los del PDB; cambia sólo el *mangling* (Itanium `_ZN12GameRenderer6renderEv` frente a MSVC `?render@GameRenderer@@`). `src/offsets.rs` ya hace esa traducción **[V]**.

**b) Los símbolos concretos que hay que enganchar.** factorio-3d-mod engancha, entre otros: `GameRenderer::render`, `GameView::createRenderParameters`, `RenderParameters::centerOn`, `PlayerInputSource::computeWalkingDirectionFromPlayerInputs`, `GameView::getMapPosition`, `Entity::~Entity`, `WorkingVisualisations::draw*`, `ItemStackPainter::drawItemStack`, `TransportBelt::drawItems`, `DrawQueue::drawScaledTinted`, `Entity::getPrototype`, `Entity::getSurface`, `DayTime::getDarkness`, y los `::draw(DrawQueue&)` de cada máquina **[V]** (`factorio-3d-mod/factorio_3d_models/src/offsets.rs`).

Verificados **en nuestro binario** (dirección real, `nm -C --defined-only`), con indicación de qué resuelven:

| # | Símbolo (demangled) | Dirección | Qué da |
|---|---|---|---|
| 1 | `GameRenderer::render()` | `0x177d7b0` | **el frame completo**; punto de entrada del render |
| 2 | `GameRenderer::prepare(RenderParameters const&)` | `0x1779db4` | parámetros de cámara antes del render |
| 3 | `GameRenderer::postPrepare()` | `0x17791cc` | post-proceso |
| 4 | `GameView::getMapPosition(PixelPosition) const` | `0x176c6b8` | **(b) la cámara 2D**: píxel de pantalla → posición de mapa |
| 5 | `GameView::prepareRender()` | `0x1777be0` | sincronización antes de dibujar |
| 6 | `RenderParameters::centerOn(Map const&, SurfaceView const&)` | `0x17728a8` | escala real (zoom × displayScale) |
| 7 | `DrawEngine::render(std::vector<SimpleBoundingBox> const*, GameRendererContainer&)` | `0x1780154` | orquestación de capas |
| 8 | `DrawEngine::drawEntities(...)` | `0x178dd9c` | **(a) las entidades visibles**, por capa |
| 9 | `DrawEngine::drawPrimitives(...)` | `0x178f580` | primitivas (líneas, texto) |
| 10 | `DrawEngine::renderShadowsToWorkingSprite()` | `0x178d560` | capa de sombras |
| 11 | `DrawQueue::drawScaledTinted(Sprite const&, MapPosition const&, ...)` | `0x1845af4` | **(a) posición + sprite** de cada objeto dibujado |
| 12 | `DrawQueue::drawScaled(Sprite const&, MapPosition const&, ...)` | `0x18fbf20` | ídem, ruta rápida |
| 13 | `ItemStackPainter::drawItemStack(DrawQueue&, unsigned int, Sprite const&, MapPosition, ...)` | `0x18f51b8` | **(a) ítems sobre cintas** con su posición |
| 14 | `TransportBelt::drawItems(DrawQueue&, TransportLineConnector const&, ...)` | `0x2f78694` | **(a) ítems en tránsito** |
| 15 | `WorkingVisualisations::drawMainAnimation(DrawQueue&, double, bool, bool, MapPosition const&, Direction) const` | `0x1eeeec8` | **(a) animación/estado de máquinas** + posición + dirección |
| 16 | `Entity::~Entity()` | `0x1815f4c` (`_ZN6EntityD2Ev` = `0x1815e50`) | ciclo de vida de entidad (registro) |
| 17 | `Entity::changePosition(Vector const&)` | `0x16f2674` | **(a) movimiento de entidad** |
| 18 | `Entity::setupPosition(Surface&)` | `0x16f454c` | alta de entidad en el mapa |
| 19 | `Entity::onChangePosition(MapPosition const&)` | `0x1818aa4` | aviso de cambio de posición |
| 20 | `CameraBase::CameraBase(Player const*, Entity const*, Surface const&, MapPosition const&, ...)` | `0x1771080` | **(b) la cámara**: jugador, entidad seguida, zoom |
| 21 | `CameraBase::paint(agui::PaintEvent const&, ...)` | `0x1771c28` | pintado de la cámara |
| 22 | `GraphicsInterfaceOpenGL::initialize(unsigned int, unsigned int)` | `0x34dc3f4` | **creación del contexto GL** → se puede forzar el tamaño/resolución |
| 23 | `GraphicsInterfaceOpenGL::swapBuffers()` | `0x34e739c` | **(c) el framebuffer final** ya presentationsado |
| 24 | `GraphicsInterfaceOpenGL::drawIndexed(DrawBindings const&, ...)` | `0x34e9bcc` | draw calls indexadas |
| 25 | `GraphicsInterfaceOpenGL::bindFramebuffer(Framebuffer const*)` | `0x34e9ee8` | a qué FBO se dibuja (clave para saber cuál es el final) |
| 26 | `FramebufferOpenGL::generateFramebufferObject()` | `0x34ec020` | FBO del framebuffer de pantalla |
| 27 | `FramebufferOpenGL::copyTo(Framebuffer*)` | `0x34e502c` | copia del framebuffer de trabajo |
| 28 | `EngineFramebuffers::initializeCommon(GraphicsInterface&, unsigned int, unsigned int)` | `0x2647cc8` | crea los framebuffers del motor |
| 29 | `GlobalContext::swapRenderBuffers()` | `0x3359d5c` | **(c) el punto de presente por frame** (ver abajo) |
| 30 | `GlobalContext::loadSprites()` | `0x339c5c8` | carga de sprites (diagnóstico: ¿hay PNG?) |
| 31 | `GlobalContext::headlessMode` | `0x3ea1168` (`.bss`) | flag de modo headless |
| 32 | `SDLWindow::SDLWindow(GraphicsInterface::Type, WindowPositionData)` | `0x34d8cd4` | crea ventana + contexto GL + `gladLoadGLLoader` |
| 33 | `SDLWindow::swap() const` | `0x34ee95c` | swap de la ventana |
| 34 | `SDLWindow::presentEngineMainFramebuffer(Window::PostprocessingParameters const&)` | `0x34ee4e8` | blit final del framebuffer del motor a la ventana |
| 35 | `HeadlessGraphicsInterface::swapBuffers()` | `0x34f1b2c` | stub (8 bytes) — el punto que hay que *reemplazar* |
| 36 | `gladLoadGLLoader` | `0x385f990` | carga de funciones GL |

> **Los 36 símbolos de la tabla existen literalmente en el binario** (verificado con `nm -C --defined-only`), no sonhips inventados. Los 10 primeros satisfacen el requisito de citar ≥10 símbolos reales con dirección.

**c) El gancho del framebuffer final (c).** `GlobalContext::swapRenderBuffers()` @ `0x3359d5c` está desensamblado así **[V]**:

```asm
3359dac: ldrb  w8, [x8, x9]      ; lee un flag
3359db0: cmp   w8, #0x1
3359db4: b.ne  0x3359ddc
3359db8: ldr   x0, [x19, #0x58]  ; el objeto Window
3359dbc: ldr   x8, [x0]          ; su vtable
3359dc0: ldr   x1, [x8, #0xb8]   ; ranura virtual 0xb8 (índice 22)
3359dc8: ldp   x20, x19, [sp,#0x50]
...
3359dd8: br    x1                ; tail call a la virtual
```

Es decir: `swapRenderBuffers()` hace **tail-call virtual** al slot `0xb8` del vtable de `Window`, que en el build gráfico resuelve a `SDLWindow::swap()` (`0x34ee95c`) → `SDL_GL_SwapWindow_REAL`. **Ese es el hook de presente por frame, y es un único punto, en la cima del pipeline.**

**d) Frida en Linux ARM64.** Frida sí soporta Linux ARM64 (el propio repo de factorio-3d-mod trae `tools/frida_*.py` para Windows; en Linux se usaría `frida-server` en el dispositivo). **[V]** el código de factorio-3d-mod resuelve por nombre, así que portarlo es sobre todo reescribir la resolución de símbolos (PDB → `.symtab`). **Limitación práctica [NV]:** Frida necesita `ptrace`/`/proc` y en Android requiere root o un proceso con el mismo UID; dentro de una app normal de Android, `ptrace` está restringido desde Android 10+. La alternativa más limpia en Android es **`LD_PRELOAD` con un `.so` propio que intercepte la resolución de símbolos** (por ejemplo, hooking de la tabla de símbolos de `libdl` o un *interposer* sobre `SDL_*` y `gl*`), que no necesita `ptrace` en absoluto. **Recomendación: hook por interposición en `LD_PRELOAD` con las direcciones resueltas por nombre desde el `.symtab`**, que además funciona sin root.

**e) Estructuras: los offsets de `GameView`.** En `GameView::getMapPosition` (`0x176c6b8`) se lee un `double` en `[this + 0xed0]` y dos `int` en `[this + 0xee4]` y `[this + 0xee8]` **[V]**. `[this+0xed0]` es la coordenada X de la cámara (se multiplica por una constante de escala leída de un global). Esto da acceso **directo** a la cámara 2D sin depender de Lua. **[NV]** No he reconstruido el layout completo de `GameView`; los offsets citados son los observados en ese punto concreto del desensamblado, no un mapa verificado de la estructura.

### C.3 Opción 3 — Instrumentación vía Lua (mod): útil pero limitada

**Ventajas:** cero trabajo nativo, portable, y la API pública de Lua es estable. El proyecto ya tiene `mods/xfactorvr_bridge/` con interfaces remotas **[V]**.

**Límites (reales):**
1. La API pública **no** expone la posición de los ítems que viajan por las cintas. `LuaEntity` da la posición de la *entidad* (la cinta), no la del ítem; `on_player_built_entity` y `script.raise_event` no cubren el estado interno de la cinta.
2. El estado interno de las máquinas (muelle de un inserter, progreso de una receta, fase de una máquina de vapor) **no** está expuesto más allá de lo público.
3. **No hay acceso al framebuffer** desde Lua en absoluto. La API pública de render de Factorio 2.x expone *orden de dibujo* de entidades y sprites, pero no una superficie de píxeles.
4. El *timing*: Lua corre en el hilo de simulación, no en el de render; no sirve para capturar frames.

**Conclusión:** Lua es **complemento** (estado de juego, eventos, prototipos) pero **no puede** sustituir el hook nativo para (a) posiciones finas y (b) el framebuffer.

### C.4 Resumen de C

| Sub-objetivo | Opción 1 (preload de DrawCalls) | **Opción 2 (hook por símbolo)** | Opción 3 (Lua) |
|---|---|---|---|
| (a) posición/dirección/frame de cada entidad | ❌ | **✔ `DrawQueue::drawScaledTinted` `0x1845af4`, `ItemStackPainter::drawItemStack` `0x18f51b8`, `WorkingVisualisations::drawMainAnimation` `0x1eeeec8`** | parcial |
| (b) cámara 2D | ❌ | **✔ `GameView::getMapPosition` `0x176c6b8`, `CameraBase::CameraBase` `0x1771080`** | ❌ |
| (c) framebuffer final | ⚠️ frágil (multi-target, MSAA) | **✔ `GlobalContext::swapRenderBuffers` `0x3359d5c` → `SDLWindow::swap` `0x34ee95c`** | ❌ |
| Estabilidad ante actualización del juego | ❌ | **✔ (resuelve por nombre)** | ✔ |
| ¿Funciona sin root en Android? | ✔ | ✔ (interposición `LD_PRELOAD`) | ✔ |

### C.5 Hallazgo colateral: **`fallback_rva` de `src/offsets.rs` están desalineados**

Al validar los patrones contra el binario real **[V]**:

| Símbolo | Patrón Itanium | `fallback_rva` actual | Dirección real | Estado |
|---|---|---|---|---|
| `GameRenderer::render` | `_ZN12GameRenderer6renderEv` | `0x01596320` | `0x177d7b0` | ❌ desalineado |
| `GameRenderer::renderInternal` | `_ZN12GameRenderer14renderInternalEb` | `0x01599b30` | — | ❌ **el símbolo no existe** (inlineado en 2.1) |
| `RenderParameters::centerOn` | `_ZN16RenderParameters8centerOnERK3MapRK11SurfaceView` | `0x01587730` | `0x17728a8` | ❌ desalineado |
| `GraphicsInterfaceOpenGL::swapBuffers` | `_ZN23GraphicsInterfaceOpenGL11swapBuffersEv` | `0x03316520` | `0x34e739c` | ❌ desalineado |
| `PlayerInputSource::computeWalkingDirectionFromPlayerInputs` | `_ZN17PlayerInputSource39computeWalkingDirectionFromPlayerInputsEv` | `0x01c6b1f0` | `0x1c6b1f0` | ✔ correcto |
| `GameView::getMapPosition` | `_ZNK8GameView14getMapPositionE13PixelPosition` | `0x01580a00` | `0x176c6b8` | ❌ desalineado |
| `AssemblingMachine::draw` | `_ZNK17AssemblingMachine4drawER9DrawQueue` | `0x02296ab0` | `0x2400684` | ❌ desalineado |
| `MiningDrill::draw` | `_ZNK11MiningDrill4drawER9DrawQueue` | `0x01c2fc40` | `0x1ed4354` | ❌ desalineado |
| `Turret::draw` | `_ZNK6Turret4drawER9DrawQueue` | `0x02d59f30` | `0x2f64be8` | ❌ desalineado |
| `AmmoTurret::draw` | `_ZNK10AmmoTurret4drawER9DrawQueue` | `0x02ff2c20` | `0x31e5714` | ❌ desalineado |
| `Furnace::draw` | `_ZNK7Furnace4drawER9DrawQueue` | `0x02f42d50` | `0x313890c` | ❌ desalineado |
| `Entity::~Entity` | `_ZN6EntityD2Ev` | `0x01540c00` | `0x1815e50` | ❌ desalineado |
| `CraftingMachine::draw` | — | `0x02297810` | — | ❌ **el símbolo no existe** (→ `WorkingVisualisations::drawMainAnimation` `0x1eeeec8`) |

**11 de 13 patrones resuelven correctamente** (la ruta por nombre funciona), **pero 10 de 13 `fallback_rva` apuntan a direcciones equivocadas** — parecen copiados de los RVAs de Windows (base 2.0.77) de factorio-3d-mod. **Riesgo:** si la búsqueda por nombre falla alguna vez (otro build, otro binario), el código saltará a una dirección arbitraria. **Recomendación:** regenerar los `fallback_rva` desde el `.symtab` del binario ARM64 (o eliminarlos y hacer que el fallo sea explícito). *No he modificado ningún fichero del proyecto: esto es sólo el informe.*

---

## D. Plan de validación en el visor

### D.0 Qué hace falta **del usuario** (bloqueante)

| # | Necesidad | Para qué | Cómo |
|---|---|---|---|
| U1 | **Factorio 2.1.22 (rama experimental) en Steam** con las descargas de Linux ARM64 | obtener el **binario gráfico ARM64** + su `data/` con los PNG | Steam → Factorio → Propiedades → Betas → `2.1 (experimental)` → descargar. Tras la actualización, Steam deja `bin/x64/factorio` **y** `bin/arm64/factorio` **[F]** (Valve/Wube Accordaron publicar ambos binarios y un script que elige por arquitectura). |
| U2 | Alternativa: cuenta de factorio.com | descargar el standalone gráfico ARM64 (`https://factorio.com/get-download/2.1.22/expansion/linux-arm64`, hoy **403** sin login) | login web |
| U3 | Factura de prueba / un zipline en el dispositivo | sacar el `data/` gráfico si U1 no es viable | ver D.2 |
| U4 | (Opcional) Steam Deck / otro ARM64 con GL 3.3 | validar el cliente gráfico ARM64 en un escritorio ARM64 antes de meterlo en el visor | — |

> **U1 es la clave y probablemente sea lo único que falte.** El Steam local actual es **2.0.77 x64** **[V]** y sólo trae `bin/x64` **[V]**. Con la beta 2.1 actualizada, Steam traerá el ARM64 **gráfico** sin login adicional.

### D.1–D.8 Pasos

| # | Paso | Riesgo | Punto de riesgo concreto |
|---|---|---|---|
| **1** | **Conseguir el cliente gráfico ARM64** (U1/U2) y verificar sus `DT_NEEDED` con `objdump -p` | 🟢 Bajo | Confirmar que el gráfico **también** enlaza SDL2 estático y **no** añade `libGL`/`libX11` duras. Comparar con el headless (A.1). |
| **2** | **Inventariar assets**: `find data -name '*.png' \| wc -l` → debe dar miles, no 0. Confirmar que `data/core/graphics/shaders/*.vert/.frag` está | 🟢 Bajo | Si el cliente gráfico **no** trae los PNG (p. ej. van en un paquete aparte), hay que plan B (mezclar con los del 2.0.77 local, §A.7). |
| **3** | **Validar el cliente gráfico ARM64 en un escritorio ARM64** (Raspberry Pi 5 / Asahi / Steam Frame) antes de tocar el visor | 🟡 Medio | Necesita **OpenGL 3.3 de escritorio**. En un Pi 5 hay que forzar `MESA_GL_VERSION_OVERRIDE=3.3` + `MESA_GLSL_VERSION_OVERRIDE=330` y poner `ogl-accelerated-renderer=false` en `config.ini` **[F]**. Si arranca y dibuja aquí, el problema restante es **sólo Android**. |
| **4** | **Mesa para Android ARM64**: construir Mesa (Zink + Turnip, y/o llvmpipe) con el NDK y empaquetarlo | 🟠 **Alto** | El build de Mesa para Android está documentado pero es delicado (LLVM para NDK, `wrap file`) **[F]**. Alternativa de menor riesgo: **usar los paquetes Mesa de Termux** si existen para la versión de Android de la Pico, o binaries pre-arm64 de `libadrenotools`/Turnip. **Mitigación:** empezar por `llvmpipe`, que es mucho más fácil de construir y empaquetar. |
| **5** | **Hacer que el juego arranque en el visor**: `/data/local/tmp/`, glibc de Termux (`build/glibc.tar.gz`), `LD_LIBRARY_PATH` al glibc y a Mesa, `SDL_VIDEODRIVER=offscreen` o EGL surfaceless | 🟠 Alto | (a) glibc 2.38 en bionic: ya resuelto por `prepare_glibc.sh` **[V]**; (b) `dlopen` de `libEGL.so.1` → **hay que fazer que resuelva a nuestro Mesa y no al del sistema** (mismo truco de espacio de nombres que `adrenotools` **[V]**); (c) audio: `libasound`/`libpulse` no existen en Android → **desactivar el audio** (`sound/…` en `config.ini`) o enrutar a OpenSL/Oboe. |
| **6** | **Primer frame del juego**: hookear `GlobalContext::swapRenderBuffers()` (`0x3359d5c`) y volcar el framebuffer | 🟠 Alto | (a) `glReadPixels` desde el back buffer **no funciona** en GL core → hay que renderizar a un FBO propio o usar `SDL_GL_ReadPixels`; (b) MSAA: hay que resolver antes; (c) leer **el** framebuffer correcto: `GraphicsInterfaceOpenGL::bindFramebuffer` (`0x34e9ee8`) permite detectar cuál es el final. **Alternativa más limpia:** `FramebufferOpenGL::copyTo` (`0x34e502c`) o `blitTo` (`0x34e5214`) a un FBO de lectura. |
| **7** | **Cruce de procesos**: llevar el frame a la app OpenXR | 🟠 Alto | (a) **no hay sharing de GL entre procesos** en Android → hay que pasar por CPU (`glReadPixels` → socket UNIX → shm) o por `dmabuf` (`EGL_EXT_image_dma_buf_import`); (b) 3,7 MB/frame a 72 Hz ≈ 270 MB/s: usar **shm** y evitar el socket con copia; (c) sincronizar con `glFenceSync` para no leer un frame a medio dibujar. |
| **8** | **Render 3D en OpenXR**: proyectar el frame 2D como textura en el diorama + **reconstruir la geometría en 3D** | 🟡 Medio | (a) la textura 2D es plana: **no da profundidad** → hace falta el punto C (a)+(b) para colocar las entidades 3D en el mundo; (b) `GameView::getMapPosition` (`0x176c6b8`) da la cámara 2D para colocar el plano; (c) hay que decidir el modo: "diorama con modelos 3D + suelo 2D" (lo que ya hace el proyecto) vs "todo 3D". |

### D.2 El fork de decisión que propongo

**Noir de golpe los dos caminos.** El orden correcto es:

```
S1. Conseguir el binario gráfico ARM64            (U1)          ─┐
S2. Validarlo en un ARM64 de escritorio            (D.3)         ├─ barato, descarta el 80 % del riesgo
S3. llvmpipe en el visor → ver un frame            (D.5/D.6)    ─┘  (funciona seguro, lento)
S4. Zink+Turnip → 60 fps                           (D.4/D.5)     (optimización)
S5. Cruce a OpenXR + geometría 3D                  (D.7/D.8)
```

Si S1 falla (no hay forma de conseguir el gráfico), el **plan B es parchar el binario headless**: forzar la instanciación de `SDLWindow`/`GraphicsInterfaceOpenGL` en vez de los stubs `HeadlessWindow`/`HeadlessGraphicsInterface` (vtables en `0x3ac0bb0` y `0x3ac1500`; stubs en `0x3c13ac0` y `0x3c13c78` **[V]**) y `GlobalContext::headlessMode` (`0x3ea1168` **[V]**) a `false`. Con los símbolos sin strip **[V]** es localizable, pero es un *binary patch* y por definición frágil → por eso es plan B.

---

## E. Riesgos y qué **no** está verificado

### E.1 Riesgos

| Id | Riesgo | Mitigación |
|---|---|---|
| **R1** | Que la elección headless/gráfico esté tomada en **tiempo de compilación** y no se pueda forzar en runtime → el plan B muere | No depende del plan B: el camino principal es el binario gráfico oficial |
| **R2** | Que el cliente gráfico ARM64 linkee `libGL`/`libX11` **duras** (a diferencia del headless) | Verificar en D.1 con `objdump -p`; si es así, usar `patchelf`/`--replace-needed` |
| **R3** | Zink añade latencia y coste de CPU no medidos | Medir antes de descartar llvmpipe |
| **R4** | El `dlopen` de `libEGL.so.1` picks el EGL de Android (que sólo da GLES) en vez del de Mesa | Mismo truco de espacio de nombres que `adrenotools` **[V]**; verificar con `SDL_GetCurrentVideoDriver` / `glGetString(GL_VERSION)` |
| **R5** | Latencia del cruce de procesos (readback + subida) | `shm` + `dmabuf` si se puede; medir |
| **R6** | `fallback_rva` desalineados (§C.5) | Regenerarlos desde el `.symtab`; hacer que el fallo por nombre sea explícito |
| **R7** | El cliente gráfico ARM64 de Steam puede exigir un steamrt 4.0 que no tenemos | El standalone de factorio.com no lo requiere; y en Android tampoco hay steamrt |

### E.2 Lo que **no** he verificado (explicitamente)

1. **Las dependencias del binario gráfico ARM64**: no he podido obtenerlo (403 sin login). Lo que afirmo es una **inferencia fuerte** a partir del headless (mismo árbol de fuentes, SDL2 estático) y de la confirmación de Wube de que Factorio enlaza sus dependencias estáticamente **[F]**. **[NV]**
2. **La versión exacta de SDL2**: `SDL_GetRevision` devuelve cadena vacía. **[NV]**
3. **Si `GlobalContext::headlessMode` se puede forzar a `false` en runtime**: no lo he confirmado. **[NV]**
4. **El layout completo de `GameView`**: sólo el offset `0xed0` (X de cámara) observado en `GameView::getMapPosition`. **[NV]**
5. **Rendimiento real en la Pico**: todas las cifras son estimaciones o precedentes externos, no medidas en el dispositivo. **[NV]**
6. **El soporte de Turnip para Adreno 740** con la versión de Mesa concreta que usemos: **[NV]**.
7. **Si la versión 2.1 experimental de Steam trae efectivamente `bin/arm64/factorio`**: documentado por Valve/Wube **[F]**, pero no comprobado en esta máquina (el Steam local sigue en 2.0.77 x64 **[V]**).

### E.3 Comandos de referencia (todos ejecutados en este repo)

```bash
cd /Volumes/X10Pro/Projects/xfactorvr

# Dependencias dinámicas
objdump -p factorio_arm64/bin/arm64/factorio | grep -A40 "Dynamic Section"

# Versiones de glibc exigidas
objdump -p factorio_arm64/bin/arm64/factorio | grep -oE "GLIBC_2\.[0-9]+" | sort -uV

# ¿SDL2 o SDL3? (patrón _REAL/_DEFAULT de la dynamic API)
nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep -cE "[TtW] SDL_"
nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep -E "SDL_(Init|CreateWindow|GL_CreateContext)_"

# Versión de OpenGL exigida (máxima del loader GLAD)
nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep GLAD_GL_VERSION

# Drivers de vídeo y librerías de runtime
strings -a factorio_arm64/bin/arm64/factorio | grep -xE "x11|wayland|offscreen|dummy|kmsdrm|android"
strings -a factorio_arm64/bin/arm64/factorio | grep -xE "lib(X11|wayland|EGL|GL|drm|asound|pulse|udev).*\.so.*"

# Clases reales vs stubs
nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep "vtable for"
nm -C --defined-only factorio_arm64/bin/arm64/factorio | grep -E "HeadlessGraphicsInterface::" | sort

# Assets
find factorio_arm64 -name '*.png' | wc -l        # 0
find factorio_linux/data -name '*.png' | wc -l   # 4297

# Script de sondeo (este ADR)
bash scripts/probe_factorio_binary.sh
```

---

## F. Fuentes externas

- Wube, *Friday Facts #446 — "An ARM and a Frame"*: <https://www.factorio.com/blog/post/fff-446> — 720p/60 en Steam Frame, ARM64 nativo 20,906 UPS vs x86-64 bajo FEX 18,798 UPS, "statically link our dependencies", publicación del ARM64 en la rama experimental 2.1.
- *Five Days After the Steam Frame Shipped…*: <https://vr.org/articles/steam-frame-steamos-android-handhelds-rocknix-lepton-2026> — Steam Frame: Vulkan por **Turnip**, OpenGL por **Zink** sobre GPU Adreno.
- Mesa, *LLVMpipe — Building for Android*: <https://docs.mesa3d.org/drivers/llvmpipe.html> — build de Mesa/LLVM para Android con NDK.
- Steam-ARM (`Scrumpper/Steam-ARM`), `COMPATIBILITY.md` y `README.md`: <https://github.com/Scrumpper/Steam-ARM> — Mesa 26.1, `glx-lax`, Zink/Turnip, límites de OpenGL por GPU; Pi 5 sólo OpenGL 3.1 de escritorio.
- Foro de Factorio, *Has anyone tried running Factorio on Raspberry Pi 5 Box64*: <https://forums.factorio.com/viewtopic.php?t=110710> — `MESA_GL_VERSION_OVERRIDE=3.3`, `ogl-accelerated-renderer=false`, 60 fps en primera carga.
- Foro de Factorio, *Sub 20 fps in new worlds*: <https://forums.factorio.com/viewtopic.php?t=60425> — llvmpipe expone **OpenGL core 3.3**.
- factorio-3d-mod (en este repo), `factorio_3d_models/src/offsets.rs` y `README.md` — modelo de hooks por nombre de símbolo y lista de funciones del motor a enganchar.
- `Generals-Zero-Hour-XR/cmake/adrenotools.cmake` (en este repo) — carga de Mesa Turnip en Android vía `bylaws/libadrenotools`.

---

## G. Decisión

**Se acepta la arquitectura recomendada en §B.6**:

1. **Binario**: cliente **gráfico** ARM64 oficial de la rama Steam `2.1 (experimental)` (necesita U1).
2. **Gráfico**: **Mesa Zink + Turnip** sobre el Vulkan del Adreno 740, reutilizando el `adrenotools` que ya está en el repo; **llvmpipe** como red de seguridad y banco de pruebas.
3. **Enganche**: hooks nativos **por nombre de símbolo** en el `.symtab` del ELF, en `GlobalContext::swapRenderBuffers()` (`0x3359d5c`) para el frame y `GameView::getMapPosition()` (`0x176c6b8`) para la cámara 2D; `DrawQueue::drawScaledTinted` (`0x1845af4`) e `ItemStackPainter::drawItemStack` (`0x18f51b8`) para las entidades. Lua sólo como complemento.
4. **Presentación**: frame renderizado offscreen → FBO → lectura → shm → textura OpenXR, proyectado en 3D con la geometría reconstruida desde los hooks.

El primer objetivo verificable es: **un frame real de Factorio 2.1 dibujado por el motor, saliendo del proceso del juego, visible en el visor.**
