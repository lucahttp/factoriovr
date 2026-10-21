# ADR-007 — La campaña se juega en un servidor headless (jugador virtual)

Estado: **aceptado** (2026-10-21)

## Por qué

El nivel de campaña (`base/campaigns/tutorial/level-01`) es **singleplayer**: su
`init` usa `game.players[1]`. En un servidor headless no hay nadie conectado,
así que el nivel nunca arranca su lógica (ADR-005 lo dejó esperando al cliente
gráfico).

En la **Pico 4 Ultra el cliente gráfico no arranca**:

- SELinux no deja hacer `dlopen` de Mesa desde `/data/local/tmp` (contexto
  `shell_data_file`).
- La Pico tampoco deja ejecutar un ELF dentro del sandbox de una app.
- La única vía que queda es Termux, y depende de que el usuario instale
  Termux:X11.

O sea: depender del cliente gráfico es depender de algo que aquí no tenemos.

## Qué se ha hecho

Ni el visor ni la Pico necesitan un "jugador de verdad": lo que hace falta es
que **el nivel de campaña tenga alguien a quien preguntar**.

### 1. El mod crea un jugador virtual

`mods/xfactorvr_bridge/control.lua` crea un **personaje** de la fuerza `player`
cuando no hay ningún jugador conectado y lo expone por el remoto
`xfactorvr.get_virtual_player`.

Ojo con dos detalles del API de Factorio 2.1.22 (el aviso del enunciado era
de 2.0 y ya no aplica):

- **`LuaForce.create_character` NO existe** en 2.1.22. La API actual solo tiene
  `LuaPlayer.create_character`, que exige un jugador ya conectado. La vía que
  usa el propio juego base es
  `surface.create_entity{name = "character", position = ..., force = ...}`
  (ver `base/script/team-production/team_production.lua`).
- Una entidad de tipo `character` es un **`LuaControl`**, igual que un jugador:
  tiene `position`, `surface`, `force`, `teleport()`, `mine_entity()`,
  `can_reach_entity()`, `get_item_count()` y `get_main_inventory()`. Por eso el
  personaje virtual se mueve, mina, fabrica e interactúa con las máquinas
  igual que el jugador real.

Por el remoto **no cruza una entidad** (los objetos Lua no se pueden pasar),
así que el mod devuelve el `unit_number` y el escenario lo localiza con
`game.get_entity_by_unit_number` (el prototipo `character` lleva el flag
`get-by-unit-number`).

Las órdenes que antes solo funcionaban con un jugador real
(`teleport_player`, `interact_entity`, `manual_mine`, `craft` y las de
inventario) ahora operate sobre el **actor**: el jugador real si lo hay y, si
no, el personaje virtual.

### 2. El escenario usa ese personaje

`scripts/preparar_campana_mp.py` inyecta en el `control.lua` del nivel un stub
`jugador()`:

- devuelve el jugador real si lo hay;
- si no, el personaje virtual del mod, envuelto en una tabla que emula la API
  de `LuaPlayer` (24 métodos: casi todos no-ops, los que necesitan el mundo
  leen del personaje de verdad);
- y sustituye `game.players[1]` → `jugador()` y `game.get_player(1)` →
  `jugador()`.

Además:

- El `init` de la rama `setup` espera a que haya jugador (real o virtual) antes
  de arrancar la historia.
- `game.show_message_dialog` → `story_show_message_dialog` (como en ADR-005).
- Se envuelve `story_update` para que, **cuando no hay ningún jugador real**,
  le pase a la historia un evento sintético de "acción del jugador" en cada
  tick. Sin esto la historia se queda clavada: el tutorial espera eventos de
  jugador (seleccionar el mineral con el ratón, abrir la GUI de fabricación) y
  un personaje sin controlador de GUI **nunca** los recibe. Con un jugador real
  conectado el nivel se comporta exactamente igual que siempre.

> En Factorio solo hay **un manejador por evento y por script**, así que no se
> puede registrar otro `script.on_event(on_tick)`: el del nivel machacaría el
> nuestro. Por eso se envuelve `story_update` en vez de añadir un manejador.

### 3. La GUI virtual

Un personaje **no tiene GUI**: `opened_gui_type` devuelve `nil` y
`crafting_queue_size` devuelve 0. El tutorial mira las dos cosas. El mod
emula una GUI mínima: cuando el jugador virtual fabrica algo, queda "en la GUI
de fabricación" (`defines.gui_type.controller`) con el item en la cola durante
2 segundos, y el stub lo devuelve como `opened_gui_type` y
`crafting_queue_size`.

## Bugs encontrados de paso (estaban en el mod)

- `LuaInventory.get_contents` en 2.0 devuelve una **lista** de
  `ItemWithQualityCount`, no un mapa nombre → cantidad. Recorrerlo con
  `for name, value in pairs(...)` metía en el inventario items con el nombre
  del índice (`1`, `2`, `3`...): **los items saqueados se perdían**. Ahora se
  normaliza con `inventory_contents()`.
- `LuaInventory.remove` espera un `ItemStackIdentification` (`{name=, count=}`),
  no `(nombre, cantidad)`: los ingredientes nunca se consumían.
- Al buscar la entidad más cercana se encontraba **al propio personaje**
  (está en la posición del jugador) y le vaciaba su propio inventario. Ahora se
  excluye por `unit_number` (en `interact_entity`, `interact_machine` e
  `insert_item`).
- `surface.create_entity{raise_built = true}` lanza `script_raised_built`, no
  `on_built_entity`: por eso construir no advancesa la historia del tutorial.
- `insert_item` metía el item en el inventario del **propio jugador virtual**
  cuando este estaba junto a la máquina (que es justo lo que pasa al colocar
  el horno): el combustible se perdía en el bolsillo en vez de ir al horno.
- `place_entity` colocaba **dos hornos** en el mismo sitio si se le pedía dos
  veces: el nivel se queda mirando el primero que encuentra (el que no tiene
  el mineral) y la historia se atasca.
- `begin_crafting` sobre un personaje sin controlador **se come los
  ingredientes** y la cola no crece: los items desaparecian. Con un personaje
  virtual la fabricacion tiene que ser instantanea (a mano); la cola real
  solo se usa con un jugador de verdad.

## Cómo se verifica

`scripts/test_jugador_virtual.sh` arranca **solo el servidor headless** (sin
cliente gráfico, sin X) con el escenario de campaña y comprueba por RCON:

1. El servidor arranca y abre el RCON.
2. **No hay ningún jugador conectado** (el caso que antes no funcionaba).
3. 0 errores `non-recoverable` y 0 errores de script en el log.
4. El mundo carga el nivel (hay superficie y hay historia).
5. El jugador virtual existe: personaje en la superficie, con posición e
   inventario, y el remoto `get_virtual_player` lo devuelve.
6. Las órdenes del mod funcionan sobre él: `teleport_player` lo mueve,
   `manual_mine` mete el mineral en su inventario real y `craft` fabrica.
7. **El nivel avanza**: la historia pasa de `setup` a los nodos que dependen
   del jugador. El banco hace de jugador (teletransportarse, coger el pecio,
   señalar y minar el mineral, fabricar el horno, colocarlo, meterle el
   mineral y el combustible, fabricar la perforadora, asignarla a la barra
   rápida, colocarla sobre la veta y abonarla) y la historia llega hasta
   `show-miner-blockage`, que es el nodo 58 de 59.

Ojo: el nivel de campaña tiene `auto_pause=true` en el banco del servidor; con
`auto_pause` el mapa se congela si no hay jugadores y el nivel no puede
avanzar. En este banco se pone `auto_pause=false`.

### Dónde se para (y por qué)

El tutorial llega a `show-miner-blockage` y ahí se para. Su condición es:

```lua
if event.name == defines.events.on_player_mined_entity and event.entity.name == 'item-on-ground' then
  return true
elseif event.name == defines.events.on_picked_up_item then
  return true
end
```

Son **eventos de jugador**: el juego solo los lanza si hay un jugador real
(con su controlador). Un personaje virtual no los lanza por mucho que mines el
mineral del suelo con `mine_entity`. Es el limite real del jugador virtual: la
campaña se juega casi entera, pero el ultimo paso (recoger el mineral del suelo
con el ratón) necesita un cliente de verdad.

## Qué queda pendiente

- El ultimo nodo (`show-miner-blockage` → colocar la caja → recoger → victoria)
  necesita eventos de jugador de verdad. Habria que "fabricarlos" (que el mod
  empuje a la historia un evento sintetico de minado, como ya hace con el
  evento sintetico de tick) o que el nivel se parche para aceptar el estado en
  vez del evento.
- El visor (VR) todavia no tiene acciones para "minar veta" con el pico ni para
  "colocar maquina": el banco las llama por RCON, que es lo mismo que hara el
  cliente cuando se conecte.
- El cliente grafico **sigue siendo la via buena** cuando funciona (Pico con
  Termux:X11, PC de escritorio): con el el nivel usa el jugador real y el stub
  no hace nada.
