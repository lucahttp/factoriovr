# ADR-019 — La GPU en la Pico: por qué el cliente gráfico no abría contexto

> **Estado**: **resuelto y medido en el dispositivo** (2026-10-24).
> Revisa: `ADR-013` (el motor dibuja), `ADR-015` (el cliente en la Pico).

---

## 1. El síntoma

El cliente gráfico de Factorio moría al arrancar, en el visor:

```
0.026 Error SDLWindow.cpp:342: Failed to create an application window.
      SDL_Error: Could not find a valid EGL device to initialize
```

El motor **sí** arranca el servidor headless; lo que falla es el contexto GL.

## 2. Lo que se sospechaba (y era falso)

La hipótesis inicial era que el override del driver iba mal:

> `MESA_LOADER_DRIVER_OVERRIDE=kgsl` — el spike anterior usó `zink`, el
> override equivocado.

**Es falso**, y se comprueba en los binarios del payload:

| Binario | ¿Lee `MESA_LOADER_DRIVER_OVERRIDE`? |
|---|---|
| `libvulkan.so.1` (loader de Vulkan) | **NO** — la cadena no aparece en el binario |
| `libEGL_mesa.so.0` (vendor EGL) | **SÍ** |
| `dri/zink_dri.so` | **SÍ** |

Es el override del driver **DRI (capa GL)** y ahí el driver correcto es
`zink` (en Android no hay `/dev/dri/renderD*`, así que sin override el DRI
no autodetecta nada). El ICD de Vulkan se elige aparte, con
`VK_ICD_FILENAMES` / `VK_DRIVER_FILES`. No había ningún conflicto.

Además, este build de Mesa identifica a Turnip como **`freedreno`**
(el binario contiene `freedreno` y `turnip`; **`kgsl` aparece 0 veces**):
`kgsl` fue el nombre del driver **GL** de freedreno (`kgsl_dri.so`), que
Mesa 23.x ya no trae en Ubuntu noble.

## 3. La causa real: tres cosas, no una

### 3.1 SELinux niega `/dev/dri/*` al dominio `shell`

```
avc: denied { read } for name="renderD128" dev="tmpfs" ino=1385
     scontext=u:r:shell:s0 tcontext=u:r:object_r:graphics_device:s0
     tclass=chr_file permissive=0
```

`/dev/kgsl-3d0` (etiqueta `gpu_device`) **sí** se abre y **sí** deja
reservar memoria de GPU; `/dev/dri/*` (etiqueta `graphics_device`) **no**.

La plataforma de **devices** de Mesa —la que usa el driver `offscreen` de
SDL2— enumera los devices **por DRM**. Al no poder abrirlos, Mesa devuelve
**cero devices** y SDL2 no encuentra ninguno que inicializar.

Medido con `SDL_HINT_EGL_DEVICE=0`: el error cambia a
*"Invalid EGL device is requested"*, o sea `num_egl_devices == 0`.

### 3.2 El vendor de EGL no se cargaba: `__EGL_VENDOR_LIBRARY_FILENAMES`

`glvnd` busca el vendor de EGL en los **manifiestos JSON** de los
directorios de `__EGL_VENDOR_LIBRARY_DIRS`. La variable
`__EGL_VENDOR_LIBRARY_FILENAMES` espera **manifiestos**, no `.so`; el
launcher le apuntaba al `.so` y glvnd se quedaba **sin vendor**:
la cadena de extensiones de cliente salía **vacía** y
`eglQueryDevicesEXT` devolvía 0.

El trace del cargador lo dice sin rodeos:

```
find library=libEGL.so.1 [0]; searching
  trying file=.../libEGL.so.1
calling init: .../libEGL.so.1          <-- y NUNCA dlopen de libEGL_mesa.so.0
```

**Arreglo**: quitar `__EGL_VENDOR_LIBRARY_FILENAMES` y dejar solo
`__EGL_VENDOR_LIBRARY_DIRS` (con el manifiesto `10_mesa.json`).

### 3.3 El Mesa de Ubuntu no sirve en el visor

Con el entorno ya arreglado, el Mesa de Ubuntu (25.2) sigue dando **0
devices**: su plataforma de devices necesita DRM y no tiene backend para
KGSL.

El que sí funciona es el fork **`lfdevs/mesa-for-android-container`**
(Mesa 26.3, base **noble**, el mismo ABI de glibc que el runtime del
visor). Trae los parches para contenedores en Android (backend KGSL de
freedreno) y su tabla de compatibilidad incluye **Adreno 740** con
**OpenGL de escritorio**, sin Zink.

## 4. El resultado, medido en la Pico 4 Ultra

```
Initialised OpenGL:[1] zink Vulkan 1.4(Turnip Adreno (TM) 740v3 (MESA_TURNIP));
      driver: 4.6 (Core Profile) Mesa 26.3.0-devel
Graphics settings preset: integrated-gpuhigh
Graphics options: [Graphics quality: high] [Video memory usage: all]
Sprites loaded
Estimated VRAM usage for textures: 10.32 MB
Factorio initialised
```

Y el visor, con el motor dibujando sobre la mesa real:

```
openxr_engine: atlas de suelo cargado (1 paginas)
PxrMetric: FPS=90/90, FrmCpu=2.95ms, FrmGpu=2.53ms, GPU=33%/456Mhz
```

**V1 funciona: el motor dibuja sobre la GPU de verdad del visor.**

## 5. Lo que hizo falta para llegar ahí

| # | Pieza | Por qué |
|---|---|---|
| 1 | `scripts/probe_kgsl.c` | separa "el kernel deja reservar memoria" de "el driver falla" |
| 2 | `scripts/probe_egl2.c` | enumera devices EGL de verdad y abre contexto (la primera sonda, `probe_egl.c`, **mintió**: glvnd esconde los alias `EXT` y hay que pedirlos con `eglGetProcAddress`, no con `dlsym` — la prueba es que da lo mismo en el contenedor, donde EGL funciona) |
| 3 | `scripts/spike_turnip_pico.sh` | barrido de ICD de Vulkan con el motor real |
| 4 | Mesa de contenedores de Android | su platform de devices no necesita `/dev/dri` |
| 5 | `libLLVM.so.20.1` (paquete `libllvm20` de noble) | lo pide la `libgallium` del Mesa de contenedores |
| 6 | `__EGL_VENDOR_LIBRARY_DIRS` en vez de `..._FILENAMES` | sin esto glvnd no carga el vendor |
| 7 | `appops set ... MANAGE_EXTERNAL_STORAGE allow` | la app es `targetSdk=34`: sin esto no puede leer `/sdcard` y el atlas no aparece |
| 8 | `data/base/graphics` (1,1 GB) | el paquete ligero del servidor no trae sprites; el cliente gráfico sí los necesita |

## 6. Decisiones

| # | Decisión | Por qué |
|---|---|---|
| 1 | El ICD de Vulkan se elige **siempre** con `VK_ICD_FILENAMES` | el loader de Vulkan no lee `MESA_LOADER_DRIVER_OVERRIDE` (verificado en el binario) |
| 2 | `MESA_LOADER_DRIVER_OVERRIDE=zink` para la capa GL | en Android no hay `renderD*`; `kgsl` ya no existe en Mesa 23.x |
| 3 | El vendor de EGL se busca **por directorio** | `__EGL_VENDOR_LIBRARY_FILENAMES` espera manifiestos y rompe la carga |
| 4 | Mesa de contenedores en Android, no el de Ubuntu | el de Ubuntu no abre contexto sin `/dev/dri` |
| 5 | Se mantiene el glvnd del payload | funciona con el vendor de contenedores tal cual |
| 6 | Turnip (no el ICD del fabricante) | el ICD de Adreno es bionic y reventaría dentro del proceso glibc (dos libc en el mismo proceso) |

## 7. Cómo se reproduce

```bash
bash scripts/deploy_pico.sh          # instala APK, payload de Mesa y assets
bash scripts/spike_turnip_pico.sh    # kernel + barrido de ICD (diagnóstico)
```

En el visor:

```bash
adb shell appops set com.xfactorvr.tabletop MANAGE_EXTERNAL_STORAGE allow
adb shell "cd /data/local/tmp && nohup env XFACTORVR_USE_TURNIP=1 \
  SERVER_ADDR=127.0.0.1 GAME_DIR=/data/local/tmp/factorio \
  sh /data/local/tmp/start_factorio_client.sh > /data/local/tmp/cli.log 2>&1 &"
adb logcat -d | grep -aE 'Initialised OpenGL|PxrMetric|atlas de suelo'
```

## 8. Lo que queda

1. **Mirar el visor** y confirmar cómo se ve la mesa de 1,80×1,20 m con el
   passthrough alrededor y la fábrica real encima.
2. Cerrar el doble disparo (XFCE + RCON) y el arranque de Minecraft PE.
3. Medir el coste de laGPU en el visor cuando el motor dibuja (el `FrmGpu`
   del visor es del propio visor; el del motor aún no está medido).
