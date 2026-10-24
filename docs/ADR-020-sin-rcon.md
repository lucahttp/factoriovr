# ADR-020 — Cómo quitar el RCON: las tres vías que deja el sandbox de Lua

> **Estado**: **diseño cerrado y verificado**, implementación **empezada**
> (el canal de estado por fichero). Falta el resto (ver §7).
> Revisa: `ADR-017` (el sondeo a un hilo), `ADR-018` (sin Termux, sin RCON).

---

## 1. El objetivo

El juego ya corre entero dentro del visor (motor glibc sobre el Adreno,
ADR-019) y la entrada ya es un ratón real inyectado en el SDL del motor. Lo
único que queda atado al **RCON** es el canal entre el visor y el motor:

| Uso | Hoy | Dónde |
|---|---|---|
| Estado (entidades, terreno, inventario) | `rcon.print` + `remote.call` | `src/game_bridge.rs` ↔ `mods/xfactorvr_bridge/control.lua` |
| Acciones VR (colocar, minar, fabricar, teletransportarse) | `remote.call` por RCON | ídem |

Y el usuario lo ha dicho claro: **RCON fuera**.

## 2. El hallazgo que condiciona el diseño

> *"Factorio's Lua sandbox has **no socket, io, os or process API**. A mod
> cannot open a port, spawn a shell, or delete a file — that is deliberate:
> the engine needs every mod to be deterministically replayable in
> multiplayer."*
> — `factorio-mcp-server`, README

Un mod **no puede abrir un puerto**. Por eso `factorio-mcp-server` (y
`codex-companion`) son **dos partes**: el mod, y un "bridge" externo que
habla con el juego. Y las dos partes se hablan por… **RCON**, o por el
patrón de fichero `script-output/`:

> *"The mod exports messages to the platform Factorio data directory under
> `script-output/codex-companion/outbox.jsonl`. The local bridge tails that
> file."*

Es decir: **el patrón de fichero es la vía establecida** para sacar estado
de un mod sin RCON.

Verificado además contra la API de Factorio 2.0: `helpers.write_file(name,
data, append, for_player)` **sí existe** y escribe dentro de
`script-output/` (creando las carpetas que falten).

## 3. La otra vía: la consola por stdin

> *"The Factorio headless server **reads commands from standard input**, so
> anything you can type in in-game chat can be typed into the server
> console — including commands added by mods."*

**El motor lee órdenes por su stdin.** Es decir: el canal de acciones no
necesita ni RCON ni el puente de Lua: basta con escribir
`/silent-command remote.call('xfactorvr', 'place_entity', ...)` en el stdin
del motor, y **el motor lo ejecuta en su propio hilo** (que es exactamente
lo que el puente de Lua no conseguía hacer: la llamada desde otro hilo
tumbaba el motor con SIGABRT).

Confirmado en el propio visor: el log del motor dice
`Error InterruptibleStdioStream.cpp:72: Got EOF on stdin; closing` — o
sea, **lee stdin de verdad** (y en nuestro caso está a EOF porque se
lanza sin nadie al otro lado).

## 4. Las tres opciones (y por qué se elige esta)

| # | Opción | Estado | Veredicto |
|---|---|---|---|
| A | **RCON** (el de hoy) | funcionando | **Rechazada** por el usuario; además el motor ya da error `Couldn't parse RCON data: Maximum payload size exceeded` con la instantánea grande |
| B | **Fichero `script-output/`**: el mod empuja el estado, el visor lo lee | **implementada (lado del mod)** | **Elegida** para el estado: es el patrón establecido y no necesita threads |
| C | **Consola por stdin (FIFO)**: el visor/escritor mete `/silent-command` en el stdin del motor | pendiente | **Elegida** para las acciones: el motor la ejecuta en su hilo, sin tocar Lua |
| D | **Puente de Lua en proceso** (nuestro) | prototipos, **tumba el motor** | Descartada: `remote.call` desde otro hilo da SIGABRT (`invalid index` en `lua_rawgeti`) |

## 5. El diseño final

```text
                      ┌── consola (stdin del motor, FIFO) ──┐
visor (APK) ────────►│  "/silent-command remote.call(...)"   │──► motor ──► mod
   │                  └────────────────────────────────────────┘      (el motor lo
   │                                                                   ejecuta en
   │  lee                                                                su hilo)
   └──── script-output/xfactorvr_bridge/estado.json ◄── mod (lo escribe)
```

- **Estado**: el mod calcula la instantánea y la **escribe** en
  `script-output/xfactorvr_bridge/estado.json` cada 20 ticks (~1/3 s). El
  visor la lee y se la pasa al mismo parser que usaba con el RCON → **el
  renderizador no cambia**.
- **Acciones**: el visor/escritor escribe el comando de consola en el stdin
  del motor. El motor lo ejecuta en su hilo correcto y el mod recibe el
  `remote.call`.
- **Fin de RCON**: el servidor arranca sin `--rcon-port` y el mod deja de
  hablar con `rcon.print`.

### Detalles que ya están resueltos

1. **El centro de vista**: el mod no puede leer ficheros, así que no puede
   saber dónde mira el jugador. Solución: el centro viaja como **orden**
   (`set_view_center`) y se guarda en `storage`; el empuje periódico usa
   ese centro. Si el visor no ha dicho nada, se usa la posición del jugador.
2. **El fichero tiene que ser legible por la app**: `script-output` cae
   bajo el `write-data` del servidor, que hoy es `/data/local/tmp/...` (la
   app no puede leerlo: SELinux). **El `write-data` del servidor tiene que
   pasar a `/sdcard/XFactorVR/srv`**, donde la app ya tiene
   `MANAGE_EXTERNAL_STORAGE` concedido (ADR-019).
3. **La FIFO no puede vivir en `/sdcard`**: el FUSE de Android no admite
   ficheros especiales. Va en `/data/local/tmp` y la escribe el launcher
   (`mkfifo`); a ella llega el **inyector** (que corre como `shell`, luego sí
   la ve) o el propio shell.
4. **El terreno**: sigue siendo lo más caro. El `terrain` de la instantánea
   son miles de tiles. Con el canal de fichero no hay límite de tamaño
   (el de RCON sí lo tenía), pero el coste de generarlo sigue ahí.

## 6. Lo que ya está implementado

- `mods/xfactorvr_bridge/control.lua`:
  - `construir_snapshot(cx, cy, radius)` — la instantánea, extraída del
    remoto para que la usen los dos canales.
  - `centro_de_la_vista()` — el centro pedido por el visor (o la posición
    del jugador).
  - `set_view_center(cx, cy)` — **orden** nueva para fijar el centro.
  - Empuje periódico a `script-output/xfactorvr_bridge/estado.json` con
    `helpers.write_file` (cada 20 ticks).
  - El remoto `get_world_snapshot` se mantiene (compatibilidad).

## 7. Lo que falta (en orden)

1. **`write-data` del servidor en `/sdcard/XFactorVR/srv`** (en
   `scripts/start_factorio_standalone.sh`) para que el fichero de estado sea
   legible por la app.
2. **Lado del visor**: leer `estado.json` en vez de pedir el snapshot por
   RCON. El parser del snapshot ya existe (`apply_snapshot`): si el
   formato del fichero es el mismo JSON, **solo hay que cambiar de origen**.
   El FFI y el motor C++ **no cambian**.
3. **Canal de acciones por la consola**: FIFO en el stdin del motor
   (`scripts/start_factorio_standalone.sh`) + el inyector (o el visor)
   escribiendo ahí el `/silent-command`.
4. **Apagar RCON**: el servidor sin `--rcon-port` y el mod sin
   `rcon.print`.
5. **Canal de geometría** (el otro camino, ya descrito en §"qué falta"): el
   inyector ya captura `wx/wy/direccion/modelo`, que son justo los campos
   que el visor necesita. Con eso el mundo **no** viene del snapshot y el
   renderizador 3D del visor queda alimentado por el motor real.

## 8. Nota sobre el proyecto publicado

`luhttp/factoriovr` (el mismo trabajo, publicado en Reddit: *"Factorio in
VR… using the Linux ARM64 version of Factorio tailored for the Pico 4
Ultra"*) demuestra que la vía **mod → visor** funciona en un visor Pico 4
Ultra con el binario ARM64 de Linux. El repositorio no es público, así que
aquí no se puede leer su código; el diseño de este ADR es compatible con
esa arquitectura.
