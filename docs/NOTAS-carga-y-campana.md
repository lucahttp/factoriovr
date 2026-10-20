# Notas — Carga de partida en el cliente gráfico y arranque de la campaña

Fecha: 2026-10-20 · Complementa `ADR-002` (arquitectura) y `ADR-004` (autostart del inyector).

Estas notas juntan la evidencia medida, que es lo que cuesta volver a conseguir.

## 1. Hechos verificados

| Hecho | Cómo se comprobó |
|---|---|
| El cliente gráfico ARM64 arranca y renderiza con Zink | `Initialised OpenGL:[1] zink Vulkan 1.4(...); driver: 4.6 (Core Profile)`, 4.696 sprites, compresión BCn por GPU, 329 MB de VRAM |
| El cliente tiene `--load-game FILE` y `--load-scenario [MOD/]NAME` | `--help`: "start Factorio and load a game/scenario **in singleplayer**" (a diferencia de `--start-server`, que va a headless) |
| El motor acepta un nivel de campaña copiado como escenario | Se copió `base/campaigns/tutorial/level-01` (con su `blueprint.zip`) a `base/scenarios/xfv-nivel/` y `--load-scenario base/xfv-nivel` lo carga: "Loading level.dat: 1413855 bytes", "Map version 1.2.0-93" |
| El inyector engancha el motor por dentro | "hook vivo: 9900 frames" con el cliente real |
| El bucle principal **no** está atascado | gdb: el hilo 1 está en `mainLoopStep → swapRenderBuffers → SDL_GL_SwapWindow → dri_flush → syscall`; con ventana 320×180 el juego va a 80 fps |
| La carga **no** avanza | el log se queda en "Checksum for script __level__/control.lua" y el RCON no abre |
| El bloqueo **no lo causa el inyector** | el control sin inyector se atasca en el mismo punto |
| El bloqueo **no lo causa nuestro manipuleo de la pila de estados** | con `--load-scenario` (camino oficial del motor, sin tocar nada) se atasca igual |
| El OOM mataba el juego | con la VM a 4 GB el proceso moría (zombi) durante la carga; con 6 GB sobrevive |

## 2. Lo que sabe el motor (de la investigación con breakpoints)

- El motor llama `process(AppManager*)` **sobre el estado de arriba de la pila** cada frame.
- `InLoadingGameBox::process()` es **lo único** que llama a `startRconIfEnabled()` → sin él no hay RCON.
- `InLoadingGameBox::process()` tiene un guardia: si el estado de arriba no es `InGame` (tipo 4) **cancela la carga** ("Map loading failed: Loading cancelled").
- `InGame::process()` exige `MainLoop+0x2c0` (el `GameView`), que es NULL al arrancar; el `GameView` lo crea el propio `InLoadGameDialog::process()` (`MainLoop+0x2b8`).
- En los intentos con inyector, `InLoadingGameBox::process()` se llamó **0 veces**.

## 3. Interpretación

El camino que usa un jugador real (menú → "Continue Game" → diálogo → `InGame` + `InLoadingGameBox`)
está bien; lo que falla es que, en este entorno, **el motor nunca llega a invocar el `process()`
del estado que carga la partida**. Como el `process()` del `InLoadingGameBox` es el único que
completa la carga y arranca el RCON, la partida se queda a medio cargar para siempre.

Pistas a comprobar (en marcha):
1. Qué hay entre `run()` (MainLoop.cpp:414) y `swapRenderBuffers()` (MainLoop.cpp:731) en
   `mainLoopStep()`, y si esa parte se ejecuta (breakpoint + contador).
2. Si el juego espera un evento de ventana (expose/idle) que en Xvfb+lavapipe no llega:
   probarlo con `xdotool` (mover la ventana, forzar un expose).
3. Si con `SDL_VIDEODRIVER=x11` y **sin** Zink (GLX por software de Mesa) la carga sí
   completa: eso distinguiría "problema de Zink/lavapipe" de "problema del entorno".

## 4. Consecuencia para el objetivo

- **La campaña necesita un jugador real** (`game.players[1]` en el `on_init` del nivel), así que
  el headless no vale: hace falta el cliente gráfico con la partida cargada y el RCON abierto.
- El camino **headless + RCON + atlas de sprites** (ADR-001) sigue siendo válido y probado como
  plan B, y es el que permite construir y verificar todo el gameplay hoy.
- En el **visor** el entorno es distinto (Zink sobre el driver Vulkan real del Adreno, y un
  servidor X de termux-x11), así que el bloqueo del banco puede no aparecer. **Es la hipótesis
  principal a comprobar en el dispositivo.**

## 5. Cómo reproducir

```bash
# contenedor ARM64 con Mesa+Zink+lavapipe
podman exec xfactorvr-client bash -c '
  pkill Xvfb; Xvfb :79 -screen 0 640x360x24 & sleep 3
  export DISPLAY=:79 XDG_RUNTIME_DIR=/tmp/xdg
  export LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri
  export MESA_LOADER_DRIVER_OVERRIDE=zink
  export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json
  export LIBGL_ALWAYS_SOFTWARE=1
  cd /factorio && ./bin/arm64/factorio --window-size 640x360 \
    --rcon-port 27015 --rcon-password xfactorvr --load-scenario base/xfv-nivel'
# el RCON deberia abrir; se comprueba con:
python3 scripts/rcon_probe.py 127.0.0.1 27015 xfactorvr "/silent-command game.print('hola')"
```

## 6. Hallazgo del 2026-10-20 (tarde): el RCON es una función de SERVIDOR

En el `--help` del cliente, `--rcon-port` y `--rcon-password` aparecen bajo **"Server
options"**, no como opciones de single player. Y el `config.ini` solo tiene
`local-rcon-socket` / `local-rcon-password`, que son "when launching MP server from the
menu". **El cliente gráfico en single player no tiene RCON**: por eso el inyector no
consigue que abra el puerto, y no es un bug del inyector.

### La arquitectura que encaja con todo

```
 Factorio headless (servidor)                 <- simulación + campaña + RCON + mod puente
   · arranca el escenario de campaña: --start-server-load-scenario base/<campaña>
   · RCON abierto (es función de servidor) y el mod puente ya talking
   · el juego corre SIN jugador -> el escenario muere:
       "__level__/control.lua:132: attempt to index field '?'"
          (el nivel usa game.players[1] en su init)
        |
        | el cliente grafico SE CONECTA al servidor: --mp-connect 127.0.0.1:34197
        v
 Factorio cliente (graficos, ARM64, Mesa+Zink)  <- crea el JUGADOR y pinta el mundo 3D
   · con el inyector puesto: el mod 3D dibuja los modelos y la camara deformada
```

El cliente conectado aporta el **jugador real** que la campaña exige, y el servidor aporta
el **RCON** por el que nuestra app lee el estado y manda ordenes. Los dos procesos son el
mismo juego 2.1.22, asi que el estado y las ordenes son los mismos.

### Estado de la prueba MP (en curso)

- El servidor con `--start-server-load-scenario` arranca, llega a InGame y abre el RCON: OK.
- El cliente con `--mp-connect` todavia no entra: hay que darle su propio arbol de datos
  (el `.lock` y la deducci��n de `data/` fallan cuando se copia el binario). Es un detalle de
  rutas, no de logica: el cliente necesita su directorio con `bin/arm64/factorio` y `data/`
  como symlink, y hay que comprobar que deduce bien la raiz.
- El inyector, ademas, se activa en CUALQUIER proceso por el que pase LD_PRELOAD
  (grep, tail, cut...): hay que restringirlo al ejecutable de Factorio.

### Ideas que quedaron descartadas con evidencia

| Idea | Resultado |
|---|---|
| Forzar `AppManager::isAppInMenu()` a false | refutado con breakpoints: no es la puerta |
| Que el entorno grafico fuera el problema | refutado: zink, GLX software, sin vsync y offscreen fallan igual; el headless va bien |
| Que fuera el inyector | refutado: el control sin inyector se atasca igual |
| Que fuera el OOM | era parte: con 4 GB lo mataba; con 6 GB sobrevive, pero sigue atascado |
| Que el RCON se pudiera abrir en single player | descartado: el RCON es de servidor |
