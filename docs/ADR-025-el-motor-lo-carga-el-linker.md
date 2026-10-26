# ADR-025 — En el visor el motor lo carga el linker, no el kernel

> **Estado**: implementado y verificado en parte (el inyector ya encuentra el
> motor en el visor; falta la comprobación completa en el dispositivo).

---

## 1. El problema

El inyector (`libxfactorvr_injector.so`) se preloadea en el motor con
`LD_PRELOAD` para enganchar el render del juego y mandar al visor el
framebuffer y la geometría. En el visor **nunca se había activated**, y no
por un error del hook: el inyector ni siquiera veía al motor.

## 2. Por qué

Android usa **bionic**; el motor de Factorio ARM64 es un binario de
**glibc**. En el escritorio no hay problema: el motor se ejecuta
directamente y el núcleo lo mapea como programa principal.

En el visor hay que hacerlo con el linker de glibc a mano:

```sh
ld-linux-aarch64.so.1 --library-path <libs> /data/local/tmp/factorio/bin/arm64/factorio
```

Eso cambia dos cosas:

1. **El proceso principal es el linker**, no el motor.  `/proc/self/exe`
   apunta a `ld-linux-aarch64.so.1`.
2. **Cuando corren los constructores de los preloads, el motor todavía no
   está cargado**: el linker inicializa los `LD_PRELOAD` *antes* de mapear
   el programa que le han pasado.

Consecuencias medidas en el visor:

```
[xfv-injector] info: objeto cargado: @0x6fd4890000          <- el linker
[xfv-injector] info: objeto cargado: linux-vdso.so.1@...
[xfv-injector] info: objeto cargado: /data/local/tmp/libxfactorvr_injector.so@...
[xfv-injector] info: objeto cargado: .../libresolv.so.2@...
[xfv-injector] info: objeto cargado: .../libm.so.6@...
[xfv-injector] info: objeto cargado: .../libc.so.6@...
[xfv-injector] info: objeto cargado: .../ld-linux-aarch64.so.1@...
[xfv-injector] info: ejecutable: /data/local/tmp/glibc/lib/ld-linux-aarch64.so.1
[xfv-injector] info: simbolos de funcion leidos: 37          <- los del linker
```

37 símbolos, no los 129.219 del motor. Y como `es_el_juego()` miraba
`/proc/self/exe`, **el inyector ni siquiera intentaba instalarse**: se
quedaba callado.

En el contenedor (donde el motor se ejecuta directamente) todo funcionaba:
por eso los bancos en contenedor pasaban y el visor no.

## 3. La solución

Tres cosas, todas en `injector-linux/src/resolver.rs` y `lib.rs`:

1. **`motor_en_mapa()`**: busca el motor en `/proc/self/maps` (cada fichero
   aparece con su ruta). Es la forma fiable: un programa principal siempre
   sale en `dl_iterate_phdr` con el nombre **vacío**, igual que el linker,
   así que por la lista de objetos no se pueden distinguir.
2. **`proceso_principal()`** usa ese mapa (con el `dl_iterate_phdr` como
   plan B) y devuelve la base y la ruta **del motor**.
3. **`hay_motor_cargado()`** y, si el motor aún no ha aparecido, un **hilo
   espera** a que aparezca (el motor tarda una fracción de segundo en
   cargarse y un par de segundos en llegar al menú: hay tiempo de sobra
   para enganchar los hooks).

El log de diagnóstico (`lista_cargados()`) deja ver la lista de objetos
cargados, que es como se encontró el problema.

## 4. De paso: el RCON se abía cada 30 frames

`tick_rcon()` escribía `RCON_ABIERTO` pero **nunca lo leía**, así que
llamaba a `AppManager::startRconIfEnabled()` cada 30 frames. Esa función
crea una interfaz **nueva** del RCON cada vez: se intentaban abrir dos
sockets en el mismo puerto y el motor acababa con un SIGSEGV.

Medido en el banco de autostart: dos líneas *"abriendo el RCON"* seguidas y,
justo después, *"Received SIGSEGV"* en `processEvents`.

## 5. Lo que NO se decide aquí

El motor sigue arrancándose con el linker a mano. La alternativa sería
parchear el `PT_INTERP` del motor para que el núcleo lo ejecute directamente
(como en el contenedor), pero eso toca un fichero de 261 MB y depende de que
Android acepte un intérprete en `/data`: no compensa mientras el hilo que
espera al motor funcione.

## 6. Bancos

- `scripts/test_autostart.sh` (el que antes petaba con el SIGSEGV).
- `scripts/test_injector.sh`.
- `scripts/bisect_cliente.sh` (nuevo): en el visor, prueba el cliente con
  y sin cada pieza (shim, inyector, proxy, audio, tamaño de ventana) y dice
  en cuál se cuelga.
