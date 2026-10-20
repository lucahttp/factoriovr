# Pista de pruebas en la Pico 4 Ultra

Todo lo que hace falta para meter Factorio real (ARM64) dentro del visor y jugarlo en MR.
Los pasos están validados en el contenedor ARM64; lo que falta es probarlos en el dispositivo.

## 0. Requisitos ya resueltos

| Pieza | Dónde | Estado |
|---|---|---|
| Cliente gráfico Factorio 2.1.22 ARM64 | `factorio_client_arm64/factorio` (2 GB) | descargado y listo |
| Motor headless ARM64 | `factorio_arm64/` | listo |
| Mesa con Zink + glibc (Debian arm64) | `build/mesa/` (193 MB) | listo, `build/mesa.tar.gz` |
| Inyector nativo (detours ARM64) | `injector-linux/target/aarch64-unknown-linux-gnu/release/libxfactorvr_injector.so` | funcionando |
| Atlas de sprites reales + manifiesto | `build/factorio_assets/` | 5 páginas, 84 MB de VRAM |
| APK de la app OpenXR | `build/XFactorVR_Pico4Ultra.apk` | 1,95 MB |

## 1. Conectar y desplegar

```bash
cd /Volumes/X10Pro/Projects/xfactorvr
adb devices                 # la Pico debe aparecer
bash scripts/deploy_pico.sh --campana
```

Qué hace `--campana`:

1. Sube el motor, el cliente gráfico, el payload de Mesa, el inyector, el mod, el atlas y el APK.
2. Prepara el nivel de campaña para multijugador (`scripts/preparar_campana_mp.py`): el nivel es
   de single-player, así que hay que esperar al jugador en su `init` y usar el helper
   `story_show_message_dialog`, que es el único que funciona en MP.
3. Arranca el **servidor** con el escenario (`--start-server-load-scenario`, `auto_pause=true`).
4. Arranca el **cliente gráfico**, que se conecta por `--mp-connect` y aporta el jugador real.
5. Lanza la app OpenXR.

## 2. Qué hay que ver

En el log del visor (`adb logcat -s XFactorVR_Launcher:V libxfactorvr:V`):

| Línea | Significa |
|---|---|
| `Initialised OpenGL: zink Vulkan ...` | **Zink habla con el driver del Adreno** (no lavapipe). Es la prueba clave. |
| `hook vivo: N frames` | el inyector está enganchado dentro del juego |
| `Starting RCON interface at IP ADDR` | el servidor tiene el RCON (el puente del mod) |
| `PlayerJoinGame ... mode(create)` | el cliente creó al jugador que la campaña exige |

Y en el juego: la campaña cargada, con el jugador presente, y el terreno y las máquinas
dibujados por nuestra app.

## 3. Problemas típicos y dónde mirar

| Síntoma | Causa probable | Qué hacer |
|---|---|---|
| `SDL couldn't be initialized` | sin servidor X | el cliente necesita el X de termux-x11 (`DISPLAY=:99`); el lanzador lo busca solo |
| `ZINK: failed to choose pdev` | el ICD de Adreno no se encuentra | el lanzador busca en `/vendor/lib64/hw/vulkan`; comprobar con `ls /vendor/lib64/hw/vulkan` |
| `mprotect ... failed` | **SELinux** bloquea el inyector | ejecutar el juego como root (`su`) o desactivar el ExecMemAllowed en el kernel |
| El juego muere durante la carga | **OOM**: el motor pide ~2,5 GB con las texturas | el visor tiene 12 GB; si aun asi, bajar `--window-size` |
| `Directory name of mod ... doesn't match` | la carpeta del mod no es `<nombre>_<version>` | el despliegue lo deriva del `info.json`; no renombrar a mano |
| `Incompatible Factorio version` | el `factorio_version` del mod no cuadra | el mod declara `2.1`; el motor del visor es 2.1.22 |

## 4. Antes de nada: el banco automático

El mismo escenario se valida sin dispositivo, contra el motor real:

```bash
bash scripts/test_campana_mp.sh     # 7 fases, 15 comprobaciones
bash scripts/test_injector.sh        # el hook sigue vivo
```

El banco sale con **0** si todo va bien, **1** si algo falla y **2** si el entorno no permite
validarlo (para no dar por bueno lo que no se ha comprobado).

## 5. Lo que falta para el "modo 3D" completo

- El **cliente gráfico** pinta el juego en 2D con Zink; nuestra app pinta encima el diorama 3D
  con los sprites reales del atlas (ya implementado y validado offline con
  `scripts/preview_scene.py`).
- El **mod 3D de verdad** (modelos `.glb` y cámara deformada, como el de GDani31) se está
  portando al inyector: el juego dibujaría los modelos 3D en su propio contexto GL.
- La **publicación estéreo**: capturar el framebuffer del juego y presentarlo por OpenXR.

## 6. Notas de arquitectura (por si hay que tocar esto)

- El RCON es una función del **servidor**, no del cliente: por eso el juego corre en dos
  procesos. El servidor simula y expone el RCON; el cliente aporta el jugador y pinta.
- El cliente usa un `config.ini` propio con `read-data` compartido y `write-data` aparte: si
  no, el motor deduce el `write-data` del servidor y los dos pelean por el `.lock`.
- En campaña, `auto_pause` debe ser **true**: con `false` el mapa avanza antes de que llegue el
  jugador y el nivel muere.
- El mod es el único puente de datos: estado del mundo (entidades, terreno, yacimientos),
  inventario y órdenes. El `global` de Factorio 2.x es `storage`.
