# Pista: jugar la campaña en la Pico 4 Ultra con Termux

El camino que funciona hoy es **servidor headless + visor OpenXR**, y lo que
falta es el **jugador**: el nivel de campaña usa `game.players[1]` en su
`init`, y ese jugador lo aporta el **cliente gráfico**.

## Por qué el cliente gráfico no puede ir en /data/local/tmp

El motor de Factorio es un binario de **glibc** y hace `dlopen` de
`libGL.so.1` para crear el contexto OpenGL. En Android:

- Un proceso del **shell** (`/data/local/tmp`) **no puede hacer `dlopen`** de
  una librería de ahí: SELinux no da `execute_no_trans` sobre
  `shell_data_file` → `Could not initialize OpenGL / GLES library`.
- El **driver Vulkan del Adreno** (`/vendor/lib64/hw/vulkan.adreno.so`) es un
  `.so` de **bionic**: un proceso glibc no puede cargarlo (dos libc en el
  mismo proceso).
- Y la **app tampoco puede**: la Pico no deja ejecutar un ELF desde el
  sandbox de una app (`execv` → `Permission denied`).

Es decir: con lo que hay hoy en el dispositivo, el cliente gráfico **no tiene
dónde vivir**.

## La vía: dentro de Termux (la que usa la comunidad)

La comunidad corre Factorio ARM64 en Android con **Termux** (hay posts con
+800 upvotes): Termux crea un userspace Debian y se usa **Termux:X11** para
el gráfico. Dentro de Termux:

- el proceso es **bionic**, así que el X server de Termux:X11 le da el
  contexto OpenGL **por el socket X11** (sin dlopen raro de por medio);
- el binario ARM64 de Factorio arranca con el loader del paquete `glibc` de
  Termux (`pkg install glibc`);
- el motor tiene que estar **dentro** de Termux, porque `/sdcard` es
  `noexec`.

## Pasos

En la Pico (Termux):

```bash
pkg install x11-repo
pkg install termux-x11-nightly    # o termux-x11
pkg install glibc
termux-x11 &                       # servidor X en :99
```

El motor se deja en `/sdcard/XFactorVR/factorio` (el despliegue lo pone) y el
script lo copia al home de Termux la primera vez (porque `/sdcard` es
`noexec`):

```bash
cp ~/start_factorio_termux.sh ~/
sh ~/start_factorio_termux.sh
```

El script (`scripts/start_factorio_termux.sh` en el repo) hace:

1. copia el motor de `/sdcard/XFactorVR/factorio` a `$HOME/factorio`;
2. arranca el **servidor headless** (el del RCON) si no está;
3. arranca el **cliente gráfico** con `--mp-connect 127.0.0.1:34197`, que es
   el que aporta el jugador que la campaña exige;
4. deja los logs en `~/logs/`.

## Mientras tanto: jugar sin cliente gráfico

El **servidor headless** funciona en el visor y el **APK** se conecta a él por
RCON (`Conexion RCON con Factorio ESTABLECIDA` en el logcat). Con eso se ve el
diorama 3D en MR y se pueden mandar órdenes al juego (mover al jugador,
interactuar, minar, fabricar).

Lo que no hay es el **jugador de verdad**, y por eso el nivel de campaña no
avanza: es `game.players[1]`.

La alternativa sin Termux es que el **mod** cree un *jugador virtual* (un
personaje de la fuerza) y que el nivel se parchee para usarlo en vez de
`game.players[1]`. Es viable (el nivel del tutorial usa 24 métodos distintos
de la API de jugador, casi todos no-ops) y está pendiente.

## Resumen de lo que hay que mirar

| Dónde | Qué |
|---|---|
| `adb logcat -s XFactorVR_Launcher:V` | qué hace el lanzador (copia al sandbox, arranca el motor, conecta el RCON) |
| `adb logcat -s XFactorVR_OpenXR:V` | la vista: RCON conectado, modelos 3D cargados, diorama |
| `/data/local/tmp/factorio_arm64/logs/standalone.log` | el servidor headless (padlock, escenario, jugador) |
| `~/logs/client.log` (en Termux) | el cliente gráfico (contexto GL, conexión al servidor) |
