# ADR-015 — El cliente gráfico en la Pico: seis bugs de payload y el diagnóstico del EGL

> **Estado**: implementado (parte) / documentado. 2026-10-23.
> Trabalho sobre `PICO OS 5.15.7`, `Pico 4 Ultra` (A9210), ARM64.
>
> Contexto: `ADR-013` (el motor dibuja) y `PLAN-PICO-4-ULTRA.md` F0 (¿hay GPU
> real en la Pico?).

---

## 1. Lo que se descubrió en el dispositivo

| Comprobación | Resultado | Consecuencia |
|---|---|---|
| `adb devices` | `PA92Y0QGL7030038G`, PICO OS **5.15.7**, Android 14 | — |
| `su -c id` | **no hay `su`**; `adb root` → "adbd cannot run as root in production builds" | Sin root: nada de `setenforce 0` ni de `execute_no_trans` |
| `ls -l /dev/kgsl-3d0` | `crw-rw-rw- 1 system system` (666) | Se **puede** abrir el nodo de GPU |
| `cat /sys/class/kgsl/kgsl-3d0/gpu_model` | **Adreno740v3** | GPU confirmado |
| `logcat` durante Turnip | **`kgsl: 19029 is a illegal gpu render pid`** | El kernel **rechaza** el trabajo de GPU: el usuario `shell` no está en el grupo `graphics` ⇒ **Turnip no puede usar la GPU** desde el shell |
| `ls /dev/dri` | `card0`, `card1` (666) pero **ningún `renderD*`** | Mesa no tiene nodos DRM ⇒ su plataforma DRM no expone devices |
| `pm list packages` | `com.termux`, **`com.termux.x11`** | La receta de la comunidad (Termux + X11) está disponible |

**Conclusión de la GPU**: con la Pico sin root, **el motor (glibc) no puede usar
el Adreno**. Turnip necesita el grupo `graphics`; el ICD de Adreno de `/vendor`
es bionic y no se puede `dlopen` en un proceso glibc. La GPU sí la usa **el APK**
(nativo, bionic, por las APIs de Android). Es decir: la GPU en la Pico está
reservada al visor; el motor se queda en software.

---

## 2. Los seis bugs del payload (todos reales, todos encontrados en el dispositivo)

El cliente gráfico **nunca** había abierto ventana en la Pico: los cuatro
intentos anteriores (`cliente.log` … `cliente4.log`) morían igual, con
`Could not initialize OpenGL / GLES library`. Ese error era la punta de un
hilo de seis bugs.

### Bug 1 — La carpeta de Mesa no estaba en el camino de búsqueda
El launcher ejecutaba el motor así:

```sh
exec env LD_PRELOAD="$PRELOAD" "$LOADER" --library-path "$GLIBC_LIB" "$GAME_BIN" ...
```

`--library-path` apuntaba **solo** al runtime de glibc. El motor trae SDL2
estático y carga la capa gráfica en runtime con `dlopen("libGL.so.1")`, así que
el `dlopen` no encontraba nada.
**Arreglo**: `--library-path "$MESA_LIB:$GLIBC_LIB"` y además
`LD_LIBRARY_PATH` (el `dlopen` posterior usa esa variable, no la del enlazador).

### Bug 2 — Faltaban 29 librerías en el payload
`zink_dri.so`, `libEGL_mesa.so.0`, `libvulkan_lvp.so` y `libvulkan_freedreno.so`
dependen de `libexpat`, `libz`, `libzstd`, `libelf`, toda la familia `xcb`,
`libwayland-*`, `libffi`, `libedit`, `libtinfo`, `libxml2`, `libz3`, `libicu`,
`liblzma` y `libGLX`… ninguna estaba en el payload (el runtime de glibc de la
Pico tampoco las trae).

### Bug 3 — `podar_glibc_debian()` se llevaba por delante las que faltaban
El poda borraba el directorio `lib/` entero para quitar la glibc de Debian. En
Debian con `/usr` fusionado, **zlib instala `libz.so.1` en `lib/`** y
**expat instala `libexpat.so.1` en `lib/`**: el poda se los llevaba con la glibc.
**Arreglo**: borrar solo los ficheros que son de la glibc (lista explícita) y
mudar el resto a `usr/lib` (que es donde está el camino de búsqueda).

### Bug 4 — `podar_alias_dri()` dejaba a Mesa sin driver software
El megadriver de Mesa son 43 nombres (enlaces duros) del mismo binario de 24 MB.
El poda dejaba solo `zink_dri.so` ⇒ **no existía `swrast_dri.so`** ⇒ Mesa no
podía ofrecer el device software de EGL.
**Arreglo**: los alias se convierten en **enlaces simbólicos** al fichero real
(symlinks, no hardlinks: en Android el `link()` falla por SELinux), y el
fichero real se llama siempre `zink_dri.so`.

### Bug 5 — glvnd no encontraba el vendor de EGL
`libEGL.so.1` es el **despachador** de glvnd; el vendor real
(`libEGL_mesa.so.0`) lo busca en `/usr/share/glvnd/egl_vendor.d`, que **no
existe en el dispositivo** (el manifiesto del payload apunta a rutas absolutas
del host). Sin vendor, glvnd no expone extensiones ni devices y SDL2 muere con
`Could not find a valid EGL device to initialize`.
**Arreglo**: el launcher escribe el manifiesto con la ruta del dispositivo y
exporta `__EGL_VENDOR_LIBRARY_DIRS` y `__EGL_VENDOR_LIBRARY_FILENAMES`.

### Bug 6 — El vendor de EGL no se carga en la Pico *(diagnóstico en curso)*

> **Corrección (2026-10-23, tarde)**: la primera versión de este ADR decía que
> el `libEGL_mesa.so.0` de Debian no exportaba símbolos `egl*` y por eso el
> despachador de glvnd se quedaba sin vendor. **Era un error de lectura**:
> con `llvm-nm` se comprobó que el vendor del *contenedor* (que funciona) también
> exporta **cero** símbolos `egl*` — es normal, el vendor de Mesa no expone los
> puntos de entrada directamente.
>
> Lo que sí está medido con la sonda:
>
> | | contenedor (funciona) | Pico (falla) |
> |---|---|---|
> | extensiones de cliente del despachador | 17 (EGL_EXT_device_base, EGL_MESA_platform_surfaceless, …) | **vacía** |
> | plataforma surfaceless | EGL 1.5 OK | **NULL (EGL_BAD_DISPLAY)** |
>
> ⇒ **glvnd no consigue cargar su vendor en la Pico.** El manifiesto
> `50_mesa.json` está en el payload y el launcher escribe además uno con la
> ruta del dispositivo, exportando `__EGL_VENDOR_LIBRARY_DIRS` y
> `__EGL_VENDOR_LIBRARY_FILENAMES`; el glvnd del payload **sí** reconoce esas
> variables (`strings` las muestra) y su ruta por defecto es
> `/etc/glvnd/egl_vendor.d:/usr/share/glvnd/egl_vendor.d`.
>
> Lo que queda por probar (próxima prueba, pendiente): desplegar el **Mesa
> exacto del contenedor** (Mesa 25.2.8 + glvnd 1.7, 41 MB) y ver si con esa
> pila el cliente abre ventana. Si abre, la diferencia es la versión de
> glvnd/Mesa; si no, el problema es de cómo glvnd descubre el vendor en
> Android.
>
> El payload pasa a construirse con **Ubuntu noble (24.04)** en vez de Debian
> bookworm, que además es la misma base del contenedor. De paso: los `.deb` de
> Ubuntu van comprimidos con **zstd**, que el extractor no soportaba.

### Y una autocomprobación para que no vuelva a pasar
`prepare_mesa.py` ahora verifica, **de forma transitiva**, que todas las
`DT_NEEDED` de las librerías críticas están en el payload (y avisa de las que
faltan antes de empaquetar). Es el chequeo que faltaba: sin él, un payload
incompleto se desplegaba en el dispositivo y el error que salía no señalaba la
causa.

---

## 3. La sonda que lo destapó: `scripts/probe_egl.c`

Una sonda EGL mínima (sin includes, porque el sysroot de glibc del proyecto no
trae cabeceras) que responde:

- ¿qué librerías EGL se cargan y cuáles exportan?,
- ¿hay devices (`eglQueryDevicesEXT`)?,
- ¿se puede crear un display surfaceless?

```sh
aarch64-linux-android34-clang --target=aarch64-linux-gnu \
    --sysroot=build/glibc_sysroot -O2 -o build/probe_egl scripts/probe_egl.c \
    -Lbuild/glibc_sysroot/link -ldl
```

Es la herramienta de diagnóstico que faltaba: sin ella, el motor solo decía
"Could not initialize OpenGL" y no se sabía ni qué faltaba.

---

## 4. Lo que queda para que el cliente abra ventana en la Pico

El camino que queda, por orden de coste:

1. **Termux:X11** (ya instalado en el dispositivo). El servidor X es un proceso
   **bionic** con acceso a la GPU; el motor (glibc) habla **GLX por el socket** ⇒
   **no hay problema de dos libc** y el render va **con GPU de verdad**. Es la
   receta de la comunidad (r/termux). Coste: hay que meter el runtime de glibc y
   el payload de Mesa dentro del almacenamiento de Termux (SELinux no deja al
   proceso de la app leer `/data/local/tmp`).
2. **Root** en la Pico: abre KGSL para Turnip (GPU real en el proceso glibc) y
   `execute_no_trans` para el inyector. En PICO OS 5.15.7 hay que buscar
   exploit; los públicos eran para ≤ 5.13.3.
3. **Quedarse en software**: si el payload de Ubuntu 25.2 ya deja al cliente
   abriendo ventana con lavapipe/llvmpipe, el motor dibuja en la Pico (a pocos
   FPS) y V1 funciona. Los FPS no son requisito: con que sea jugable vale.

---

## 5. Referencias

- `ADR-013` — el motor dibuja; el inyector es el puente (sin RCON).
- `PLAN-PICO-4-ULTRA.md` — F0 (GPU/FPS) y F1 (anclaje a la mesa).
- `scripts/prepare_mesa.py` — payload de Mesa (Ubuntu noble) + autocomprobación.
- `scripts/start_factorio_client.sh` — launcher del cliente en el visor.
- `scripts/spike_gpu_pico.sh` — el spike: ¿GPU real? ¿cuántos FPS?
- `scripts/probe_egl.c` — la sonda EGL.
- r/termux — "Factorio ARM64 running natively on Android" (Termux + X11 + PRoot).
