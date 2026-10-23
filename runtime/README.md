# `runtime/` — El módulo de runtime: ejecutar Factorio Linux ARM64 dentro de Android

Este módulo es la respuesta a *"¿no podemos correr el ejecutable del juego
Linux ARM64 sin Termux?"*. **Sí se puede**, y aquí está por qué.

## El problema, con datos

El binario del juego (`bin/arm64/factorio`) es un ELF ARM64 **glibc**, y en
Android:

- el kernel es Linux → ejecutar un ELF propio **sí** se puede;
- pero Android es **bionic**, no glibc → al binario le falta su cargador
  (`ld-linux-aarch64.so.1`) y sus bibliotecas;
- y el juego pide **OpenGL 3.3 core**, que Android no tiene.

Comprobado con la tabla de símbolos del propio binario (no hay ni una
`libX11` ni una `libvulkan` en su `NEEDED`): el juego **no depende de X11**.
SDL2 carga EGL/Vulkan en tiempo de ejecución, así que el servidor X es
opcional.

## Las tres piezas (y ninguna es Termux)

| Pieza | Qué es | De dónde sale | Tamaño |
|---|---|---|---|
| `glibc/` | el cargador y las libs de glibc (2.44) | el paquete `glibc` de Termux | ~90 MB |
| `mesa/` | Mesa con **Zink** (OpenGL sobre Vulkan) + **lavapipe** (Vulkan por software) + **libGLESv2** | `scripts/prepare_mesa.py` (paquetes .deb de Debian) | ~240 MB |
| `x11/` | un servidor X virtual (**Xvfb**) por si se quiere el driver `x11` de SDL2 en vez de `offscreen` | el contenedor de pruebas | ~5 MB |

**Ninguna de las tres necesita que Termux esté instalado**: son ficheros, y se
despliegan en `/data/local/tmp` (o en el directorio interno de la app).

## Por qué hace falta cada una

- **glibc**: sin el cargador, `execve` falla con *"No such file or directory"*.
- **Mesa**: el cliente gráfico pide OpenGL 3.3 core. Zink lo da sobre Vulkan.
  En la Pico el ICD de Adreno es **bionic** y un proceso glibc no puede
  cargarlo, así que se usa **lavapipe** (Vulkan por software): lento, pero
  funciona y no depende de la GPU.
- **libGLESv2**: el driver `offscreen` de SDL2 (el único que funciona en
  Android sin servidor X) pide **EGL + OpenGL ES 2.0**. Sin `libGLESv2.so.2`
  el cliente arranca y muere con
  `Could not initialize OpenGL / GLES library`.
- **Xvfb**: opcional. Da un servidor X real (driver `x11` de SDL2) para quien
  prefiera esa vía, pero en Android necesita un `/tmp` escribible (su fichero
  de lock), así que va con `proot` o con un `-lockDir` equivalente.

## Despliegue

```bash
# 1. Empaquetar el módulo entero (glibc + mesa + x11)
bash scripts/empaquetar_runtime.sh

# 2. Subirlo a la Pico y desplegarlo
bash scripts/deploy_pico.sh --runtime
```

Los ficheros pesados **no** van en el APK (el motor son 324 MB y Mesa 240 MB);
en el APK va solo lo lightweight: los modelos 3D y el módulo X11.

## Lo que se comprobó en la Pico (Pico 4 Ultra, Android 14)

- El motor **headless** arranca con este runtime y habla RCON ✓
  (`factorio --start-server --rcon-port 27015`).
- El cliente gráfico con el driver `offscreen` **llega a la ventana de SDL**
  (`Available displays: 1`) y solo tropieza con la falta de `libGLESv2` ✓
  (arreglado en `prepare_mesa.py`).
- El motor **no** necesita Termux ni Termux:X11 para el modo headless, y con
  `libGLESv2` tampoco para el cliente gráfico.

## Nota sobre el "almacenamiento con ámbito" (Android 11+)

Una app **no puede leer `/sdcard` arbitrario** (`Permission denied`), solo su
propio `Android/data/<paquete>/files/`. Por eso los modelos 3D se despliegan
ahí y no en `/sdcard/XFactorVR`.
