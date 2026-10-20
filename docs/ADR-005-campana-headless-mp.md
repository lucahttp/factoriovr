# ADR-005 — La campaña se juega: headless (simulación + RCON) + cliente gráfico MP

Fecha: 2026-10-20 · Estado: **validado en el banco** (`scripts/test_campana_mp.sh`, salida 0)

Complementa `ADR-002` (cliente gráfico + Zink), `ADR-003` (inyector Linux ARM64),
`ADR-004` (arrancar la partida desde el inyector) y `NOTAS-carga-y-campana.md`.

## 1. Objetivo

Que la **campaña** (el nivel `base/xfv-nivel`, copia de
`base/campaigns/tutorial/level-01`) se pueda jugar, y que la app de VR pueda hablar
con ella por RCON.

La arquitectura deducida por el usuario era:

```
 Factorio headless (servidor)                <- simulación + campaña + RCON + mod puente
   · arranca el escenario: --start-server-load-scenario base/xfv-nivel
   · RCON abierto (el RCON es función de SERVIDOR)
   · el juego corre SIN jugador -> el nivel peta:
       "__level__/control.lua:132: attempt to index field '?'"
        |
        | el cliente grafico SE CONECTA: --mp-connect 127.0.0.1:34197
        v
 Factorio cliente (graficos, ARM64, Mesa+Zink)  <- crea el JUGADOR y pinta el mundo 3D
   · con el inyector puesto: el mod 3D dibuja los modelos y la camara deformada
```

**La arquitectura es correcta y queda validada**, pero hace falta resolver tres
cosas que el enunciado no contemplaba: las rutas del cliente, el nivel (que es de
singleplayer) y los ajustes del servidor.

## 2. Las rutas del cliente (el bloqueo que había)

El error que paraba todo:

```
Error configuring paths: There is no package core in /usr/share/factorio.
Deduced executable directory: /factorio_cli/bin/
```

El motor deduce el directorio de datos (`read-data`) del sitio donde está el
ejecutable, y el `write-data` (config, saves, mods y **el `.lock`**) de ahí
también. Al copiar el binario a `/factorio_cli/` para no pelear por el `.lock`,
el motor ya no encontraba `data/` y se caía.

### 2.1 Lo que NO hace falta

- **No hay que copiar los 261 MB** del binario.
- **No hay que montar la `data/` por bind mount** ni hacer symlinks de `data/`.
- **No hay que tocar `config-path.cfg`.**

### 2.2 Lo que sí funciona (combinación validada)

Un solo árbol de instalación (`/factorio`) y un `config.ini` propio para el
cliente:

```ini
# /factorio_cliente/config/config.ini
[path]
read-data=/factorio/data
write-data=/factorio_cliente
```

y el cliente se lanza con:

```bash
/factorio/bin/arm64/factorio \
    --config /factorio_cliente/config/config.ini \
    --mp-connect 127.0.0.1:34197
```

Lo que hace el motor con eso (log del cliente):

```
  0.000 Config path: /factorio_cliente/config/config.ini
  0.000 Read data path: /factorio/data
  0.000 Write data path: /factorio_cliente [22650/30056MB]
  0.000 Binaries path: /factorio/bin
```

Claves:

- `read-data` es la **data de solo lectura** del juego (`base/`, `core/`): se
  comparte sin problema entre servidor y cliente.
- `write-data` es el **árbol de escritura** (config, saves, mods, y el `.lock`):
  cada proceso necesita el suyo. Por eso el cliente usa `/factorio_cliente`.
- `config-path.cfg` (en `bin/arm64/`) solo decide dónde está el `config.ini`
  **por defecto**; `--config` lo pisa. Los valores `[path]` del `config.ini`
  mandan sobre la deducción (`use-system-read-write-data-directories=false`
  solo afecta a la *generación* inicial de esos valores).

Por qué el symlink del binario no servía: el motor resuelve `/proc/self/exe`
(camino real) y vuelve al `write-data` del servidor. Con `--config` no hay que
tocar el binario en absoluto.

> Nota: en `scripts/start_factorio_client.sh` (el lanzador del visor) se pasaba
> `--config "$FACTORIO_DIR/config/config-path.cfg"`, que es el `.cfg` y no el
> `config.ini`. Con la receta de este ADR hay que pasar el `config.ini`.

## 3. El nivel de campaña es de singleplayer (dos parches)

`base/campaigns/tutorial/level-01` asume que cuando arranca el escenario ya hay
un jugador. En un servidor el cliente entra después, así que hay que adaptarlo.

### 3.1 Parche 1 — esperar al jugador

```lua
      name = 'setup',
      init = function(event, st)
        -- [XFactorVR-MP] ...
        if not game.players[1] then
          st.init_called_last_on_story_index = 0
          st.init_called_last_on_position = 0
          return true
        end
        game.players[1].disable_recipe_groups()
```

El módulo `story` (`data/core/lualib/story.lua`) llama al `init` de la rama una
sola vez por posición; al rearmar los marcadores `init_called_last_on_*` a 0 se
vuelve a llamar en el siguiente tick. Con eso la historia arranca sola en
cuanto el servidor crea al jugador.

### 3.2 Parche 2 — los diálogos

El nivel llama a `game.show_message_dialog`, que **falla siempre en
multijugador**, incluso con el jugador conectado:

```
$ /silent-command game.show_message_dialog({text={'prueba-dialog'}})
Cannot execute command. Error: Map doesn't contain 1 player, this function can't be used
```

(mientras que `/silent-command rcon.print('jugadores='..#game.players..' idx='..
..game.players[1].index)` devuelve `jugadores=1 idx=1`: el jugador existe y es
el índice 1).

El módulo `story` ya trae el helper correcto, que en MP imprime el texto al
jugador en vez de abrir el diálogo:

```lua
function story_show_message_dialog(param, player)
  if game.is_multiplayer() then
    if player then player.print(param.text) else game.print(param.text) end
  else
    game.show_message_dialog(param)
  end
end
```

Así que el parche es `game.show_message_dialog(` →
`story_show_message_dialog(` (9 llamadas en el nivel).

### 3.3 Cómo se aplican

`scripts/preparar_campana_mp.py` (idempotente, deja copia `control.lua.orig`):

```bash
python3 scripts/preparar_campana_mp.py /factorio/data/base/scenarios/xfv-nivel
```

## 4. Ajustes del servidor (dos detalles que NO son opcionales)

### 4.1 `require_user_verification: false`

Sin esto el servidor rechaza al cliente del banco (que no tiene cuenta de
factorio.com):

```
ConnectionRequestReplyConfirm ... from(IP ADDR:({127.0.0.1:45191}))
Refusing connection for address (IP ADDR:({127.0.0.1:45191})), username (). UserVerificationMissing
```

### 4.2 `auto_pause: true` (¡y NO `false`!)

Con `auto_pause: false` el mapa avanza en cuanto arranca el servidor y el nivel
 peta **antes de que el cliente entre** (0,47 s después de arrancar):

```
  0.451 Info ServerMultiplayerManager.cpp:809: updateTick(157301) changing state from(CreatingGame) to(InGame)
  0.470 Error MainLoop.cpp:1488: Exception at tick 157301: The scenario level caused a non-recoverable error.
Error while running event level::on_tick (ID 0)
__level__/control.lua:132: attempt to index field '?' (a nil value)
stack traceback:
	__level__/control.lua:132: in function 'init'
	__core__/lualib/story.lua:71: in function 'story_update'
```

Con `auto_pause: true` (el valor por defecto de Factorio) el servidor **congela
la simulación mientras no hay jugador**: el mapa no avanza, el nivel no peta, y
cuando entra el cliente se reanuda con el jugador ya dentro. Es además lo que
hace que el banco sea robusto: con el juego pausado, hasta un comando RCON
temprano es inofensivo.

> Con `auto_pause: false` (lo que usa `scripts/server-settings.json` para el
> modo "standalone sin cliente") la campaña **no** arranca: el nivel muere
> siempre. Son dos configuraciones incompatibles; para la campaña hay que
> dejar `auto_pause: true`.

## 5. El protocolo RCON (medido)

- El RCON es de **servidor**: el cliente gráfico en single player no lo abre
  (en `--help` sale bajo "Server options").
- El **primer comando de consola de cada sesión** puede ser rechazado por el
  motor ("Using Lua console commands will disable achievements. Please repeat
  the command to proceed"): la respuesta llega vacía. Hay que reintentar (es lo
  que hace `execute_collect_retry` en `src/game_bridge.rs`).
- El texto **tiene que empezar por `/`** para que se trate como orden de
  consola. Sin `/` va como mensaje de chat y no devuelve nada:

| orden | respuesta |
|---|---|
| `rcon.print('tick='..game.tick)` (sin `/`) | vacía (tratatado como chat) |
| `/silent-command rcon.print('tick='..game.tick)` | `tick=6149` |
| `/c rcon.print('prueba-c')` | `prueba-c` |
| `remote.call('xfactorvr', 'get_world_snapshot', 16, 0, 0)` (sin `/`) | vacía |
| `/silent-command remote.call('xfactorvr', 'get_inventory')` | `[]` |

## 6. Resultado medido

Banco: `bash scripts/test_campana_mp.sh` → **exit 0**.

La siguiente salida es de una ejecución **desde el nivel intacto** (se restauró
`control.lua.orig` antes de lanzar el banco, que vuelve a parchearlo solo):
contenedor `xfactorvr-client`, ARM64, Factorio 2.1.22 build 87719.

```
=== [1] Preflight del entorno ===
  OK    contenedor 'xfactorvr-client' con datos del juego y display :99

=== [2] Preparacion (nivel para MP, mod puente, arbol de datos del cliente) ===
  Escenario: /factorio/data/base/scenarios/xfv-nivel
    parcheado: /factorio/data/base/scenarios/xfv-nivel/control.lua
      · espera al jugador antes de arrancar la historia
      · show_message_dialog -> story_show_message_dialog (multijugador)
  OK    mod puente instalado en servidor y cliente
  OK    ajustes de servidor escritos (require_user_verification=false, auto_pause=true)
  OK    config del cliente: read-data=/factorio/data, write-data=/factorio_cliente

=== [3] Arranque del servidor headless con el escenario de campana ===
  OK    servidor arrancado y RCON escuchando en el puerto 27015 (tras 2s)
  OK    el escenario no peta con el servidor solo (auto_pause lo congela)

=== [4] Arranque del cliente grafico y conexion al servidor ===
  OK    el cliente arranca con el inyector (/injector.so)
  OK    el cliente se conecto y el servidor creo el jugador (tras 22s)
    29.986 Info InputActionHandler.cpp:5065: UpdateTick (157303) processed PlayerJoinGame peerID(1) playerIndex(0) mode(create)
  2026-10-20 21:25:13 [JOIN]  joined the game
  ... dejando correr la historia del nivel 20s

=== [5] Comprobaciones por RCON (jugador, tick y mod puente) ===
  RCON: tick1=1301 | conectados=1 | jugadores=1 | nombre=
  RCON: snapshot del puente: {"entities":[{"t":"tree-04","x":-3.06,"y":-16.25,"o":0.000,"s":0,"p":0.000,"h":1.000,"f":0,"d":0},{"t":"tree-03","x":13.63,"y":-10.31,"o":0.000,...
  OK    el RCON ve conectados=1 (jugador conectado)
  OK    el nivel tiene jugadores=1 (el escenario crea al jugador)
  RCON: tick1=1301 -> tick2=1631
  OK    el tick avanza (1301 -> 1631): la simulacion corre
  OK    el mod puente responde por RCON con el estado del mundo (23 entidades)

=== [6] Revision de los logs ===
  OK    0 errores 'non-recoverable' en servidor y cliente
  servidor: write-data=/factorio
  cliente : write-data=/factorio_cliente read-data=/factorio/data
  OK    el cliente tiene su propio write-data (no pelean por el .lock)
  OK    el cliente comparte los datos de solo lectura del juego (/factorio/data)
  OK    el inyector sigue vivo dentro del cliente (23 avisos de hook)

=== [7] Limpieza ===
  OK    procesos detenidos

===========================================================
RESULTADO: CORRECTO (7 fases, 0 fallos)
La campana se juega: servidor headless + cliente grafico MP.
===========================================================
```

Log del servidor en la misma ejecución (el escenario carga, el mod puente
arranca, el cliente entra y el juego sigue):

```
  0.430 Script @__xfactorvr_bridge__/control.lua:837: [XFactorVR] Bridge mod initialized.
  0.451 Info ServerMultiplayerManager.cpp:809: updateTick(157301) changing state from(CreatingGame) to(InGame)
  0.452 Info RemoteCommandProcessor.cpp:119: Starting RCON interface at IP ADDR:({0.0.0.0:27015})
 29.953 Info ServerSynchronizer.cpp:594: nextHeartbeatSequenceNumber(1871) adding peer(1)
 29.986 Info InputActionHandler.cpp:5065: UpdateTick (157303) processed PlayerJoinGame peerID(1) playerIndex(0) mode(create)
2026-10-20 21:25:13 [JOIN]  joined the game
```

### 6.1 La historia del nivel avanza de verdad (no solo "no peta")

Con el cliente dentro, moviendo al jugador por RCON se ve cómo la historia del
nivel progresa (el objetivo que el nivel le pone al jugador es el indicador de
en qué rama está):

```
$ /silent-command rcon.print('pos='..tostring(game.players[1].position.x)..','..tostring(game.players[1].position.y))
  respuesta (1 paquete/s): 'pos=0,0\n'

$ /silent-command game.players[1].teleport({x=12.5,y=12.5})
  respuesta (1 paquete/s): ''            <- la orden no devuelve valor

$ /silent-command rcon.print('goal='..table.concat(game.players[1].get_goal_description(),' | '))
  respuesta (1 paquete/s): 'goal=goal-search-wreck\n'

$ /silent-command rcon.print('wreck='..tostring(game.get_entity_by_tag('wreck') and game.get_entity_by_tag('wreck').position.x)..','..tostring(game.get_entity_by_tag('wreck') and game.get_entity_by_tag('wreck').position.y))
  respuesta (1 paquete/s): 'wreck=4.5,-7.5\n'
```

O sea: el nivel pasó de `setup` → `movement` → `think-2` (el diálogo que
petaba) → `search-wreck`, y sigue vivo (`non-recoverable: 0`, tick 13361).
Las ramas que usan `set_goal_description` y `set_gui_arrow` (que son de
jugador) también funcionan en multijugador.

Lo que **no** se ha probado es llegar al final del nivel: las ramas finales
exigen acciones de jugador reales (abrir la GUI del pecio, coger piezas,
colocar un horno) y la última usa `game.set_game_state{... next_level='level-02'}`
que es API de campaña de singleplayer.

Y con el **inyector** puesto (cadena real del visor), el hook sigue vivo
dentro del cliente gráfico:

```
=== inyector (stdout cliente) ===
[xfactorvr] hook vivo: 1440 frames
[xfactorvr] hook vivo: 2100 frames
```

y el RCON con el inyector dentro:

```
$ /silent-command rcon.print('tick='..game.tick)
  respuesta (1 paquete/s): 'tick=6149\n'
$ /silent-command rcon.print('conectados='..#game.connected_players)
  respuesta (1 paquete/s): 'conectados=1\n'
```

## 7. Cómo se reproduce

```bash
# 1. El banco entero (prepara, arranca, comprueba y limpia)
bash scripts/test_campana_mp.sh ; echo "exit=$?"

# 2. A mano, dentro del contenedor
podman exec xfactorvr-client bash -c '
  python3 /tmp/preparar_campana_mp.py /factorio/data/base/scenarios/xfv-nivel
  cd /factorio && (sleep 1800 | ./bin/arm64/factorio \
      --start-server-load-scenario base/xfv-nivel \
      --port 34197 --rcon-port 27015 --rcon-password xfactorvr \
      --server-settings /factorio/config/server-settings.json) \
      > /factorio/campana_mp_server.log 2>&1 &'

podman exec xfactorvr-client bash -c '
  cd /factorio_cliente && env DISPLAY=:99 SDL_VIDEODRIVER=x11 \
    LIBGL_DRIVERS_PATH=/usr/lib/aarch64-linux-gnu/dri \
    MESA_LOADER_DRIVER_OVERRIDE=zink \
    VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
    LD_PRELOAD=/injector.so \
    /factorio/bin/arm64/factorio \
      --config /factorio_cliente/config/config.ini \
      --mp-connect 127.0.0.1:34197 --window-size 640x360 --disable-audio \
      > /factorio/campana_mp_client.log 2>&1 &'

# 3. Preguntar al servidor
podman exec -i xfactorvr-client python3 - --file /tmp/cmds.txt 27015 xfactorvr \
  < scripts/rcon_probe.py
```

## 8. Limitaciones y riesgos que quedan abiertos

| Tema | Estado |
|---|---|
| El jugador entra con **nombre vacío** (`game.players[1].name == ""`) | El motor 2.1 solo usa un nombre verificado de factorio.com o el que se escribe en el diálogo del juego; con `--mp-connect` no hay diálogo. Cosmético, no rompe nada (el puente usa `game.players[1]`). |
| El nivel es el **tutorial level-01**: `game.set_game_state{... next_level='level-02'}` (línea 842) es de campaña de singleplayer | No probado: el banco no completa el nivel. Si se quiere encadenar niveles hay que mirar ese punto. |
| `auto_pause` y el modo "standalone sin cliente" (`scripts/server-settings.json`) son **incompatibles** | Con `auto_pause: false` el mapa corre sin jugador y el nivel de campaña peta. Son dos modos distintos: el "standalone" necesita `false`, la campaña necesita `true`. |
| El RCON se abre **en el servidor**, no en el cliente | Confirmado: en `--help` el RCON está bajo "Server options". El cliente gráfico no lo abre. |
| El primer comando RCON de cada sesión puede ser rechazado | El banco lo tolera (duplica cada orden); `src/game_bridge.rs` ya lo hace con `execute_collect_retry`. |
| El cliente gráfico carga los assets (~25-30 s) antes de conectar | En el banco son ~24 s desde el arranque hasta `PlayerJoinGame`. El visor tiene que aguantar esa espera. |
| El mapa se sirve al cliente desde `/factorio/temp/mp-save-0.zip` | El motor lo genera solo; no hay que preparar nada. |
