# ADR-007 — La campaña se juega en un servidor headless (jugador virtual)

Estado: **aceptado** (2026-10-21) · **actualizado** (2026-10-21: la campaña
llega hasta `victory` con cero jugadores conectados; barra rápida y
combustible automáticos)

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

### 4. La barra rápida virtual se rellena sola

En el APK el inventario del jugador **se dibuja pero no es interactivo**: en VR
todavía no se pueden coger items con las manos. El tutorial tiene un nodo
(`add_filter`) que exige tener la perforadora **en la barra rápida** como
filtro:

```lua
for page=1,4 do
  for slot=1,10 do
    local slot_content = game.players[1].get_quick_bar_slot(page, slot)
    if slot_content and slot_content.type == "filter"
       and slot_content.filter.name == 'burner-mining-drill' then
      return true
    end
  end
end
```

Con un jugador real lo hace la GUI arrastrando el item. Sin GUI, el nodo no se
cumple nunca.

**Lo resuelve el mod**: `actualizar_barra_rapida()` mete en la barra rápida
virtual los items que lleva el actor (el real o el virtual) de una lista
corta (`QUICK_BAR_ITEMS`: mineral, placa, engranaje, el filtro, la caja, el
horno...), en la **primera ranura libre**. El nodo se cumple solo, sin tocar
la UI del visor.

Reglas:

- **Nunca pisa una ranura puesta a mano.** El remoto `set_quick_bar` (que es
  lo que llama el FFI `xfactorvr_set_quick_bar` desde el lado Rust) marca la
  ranura como *manual* en `xfactorvr_bar_manual`, y el relleno automático solo
  toca ranuras vacías o que el propio mod había rellenado.
- Se refresca al final de los remotos que cambian el inventario (`craft`,
  `manual_mine`, `mine_entity`, `pick_up_item`, `interact_entity`,
  `interact_machine`, `insert_item`, `place_entity`) y dos veces por segundo
  desde el `on_tick`, por si el actor coge algo por otra vía.
- El remoto nuevo `get_quick_bar` devuelve el contenido (marca `!` las
  ranuras manuales): lo usa el banco y lo puede usar el visor.

### 5. El combustible se mete solo

Por lo mismo, el jugador no puede meter carbón en el horno con las manos. Al
interactuar con una máquina, el mod le mete el carbón que lleva el actor en el
inventario de combustible si la máquina no tiene (`abastecer_combustible()`).
Con eso el nodo `put-coal` del tutorial se cumple sin inventario interactivo.

### 6. El último nodo: el minado del item del suelo

El nodo `show-miner-blockage` espera este evento:

```lua
if event.name == defines.events.on_player_mined_entity
   and event.entity.name == 'item-on-ground' then
  return true
elseif event.name == defines.events.on_picked_up_item then
  return true
end
```

Son **eventos de jugador**: el juego solo los lanza si hay un jugador real
(con su controlador). Un personaje virtual no los lanza nunca.

**Lo resuelven el mod y el escenario parcheado**:

1. El mod tiene un remoto nuevo, `pick_up_item(x, y)`: recoge el item del
   suelo (la entidad `item-on-ground`) y lo mete en el inventario del actor.
   Con un jugador real deja que lo haga el motor (así salta el evento de
   verdad); con el personaje virtual lo coge a mano.
2. Al recogerlo, el mod **anota** el minado en su `storage`
   (`xfactorvr_minado`).
3. El escenario parcheado, dentro del envoltorio de `story_update`, pregunta
   por el remoto `get_mined_item` (cada 5 ticks, no cada tick) y, si hay algo,
   le pasa a la historia **el mismo evento que habría lanzado el juego**:

   ```lua
   _xfv_story_update_real(st, {
     name = defines.events.on_player_mined_entity,
     entity = { name = pendiente.name, position = pendiente.position },
     tick = event.tick,
   }, next_level, onwin)
   ```

4. El evento se guarda en el `storage` del escenario hasta que la historia lo
   consume: si el nodo todavía no estaba listo, no se pierde (se reintenta en
   el siguiente tick).

O sea: **el escenario parcheado escucha un evento propio del mod y ejecuta la
misma lógica del nodo**.

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

Y estos tres, que aparecieron al cerrar el ultimo nodo:

- **`LuaEntity.item` NO existe en la API de 2.1**: la pila del item del suelo
  esta en `LuaEntity.stack`. Con `item` el `pcall` se come el error y el item
  se pierde en silencio.
- **`mine_entity` NO borra la entidad al instante**: el personaje "coge" el
  item y la entidad sigue viva un par de ticks. No se puede mirar si la
  entidad sigue viva, sino **lo que devuelve `mine_entity`**.
- **Al lado del item del suelo esta la perforadora**: el item cae en la
  posicion de descarga de la perforadora, a menos de un tile de ella. Un
  `mine_entity` "a pelo" podia **demoler la perforadora** y romper la
  campana. Por eso el remoto filtra por `name = "item-on-ground"` y hay un
  remoto nuevo, `pick_up_item`, que solo recoge items del suelo.

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
7. **La barra rápida virtual se rellena sola**: cuando el jugador lleva la
   perforadora, el mod la mete en la barra sin que nadie la ponga a mano (el
   banco comprueba el contenido con el remoto `get_quick_bar`).
8. **El nivel llega AL FINAL**: el banco hace de jugador (teletransportarse,
   coger el pecio, señalar y minar el mineral, fabricar el horno, colocarlo,
   meterle el mineral y el combustible, fabricar la perforadora, colocarla
   sobre la veta y abonarla, recoger el item del suelo, fabricar la caja de
   hierro, colocarla donde suelta el mineral la perforadora y vaciarla) y la
   historia llega a `victory`.

El banco **falla si el nivel no llega a `victory`**: comprobar que se llega al
final es parte del banco, no un aviso.

Ojo: el nivel de campaña tiene `auto_pause=true` en el banco del servidor; con
`auto_pause` el mapa se congela si no hay jugadores y el nivel no puede
avanzar. En este banco se pone `auto_pause=false`.

### Dónde se para (y por qué)

**Ya no se para: la campaña se completa hasta `victory`** con cero jugadores
conectados.

El nodo que faltaba (`show-miner-blockage`) se cierra con el evento propio
del mod que se describe en el punto 6 de "Qué se ha hecho": el mod anota el
minado y el escenario parcheado se lo pasa a la historia como
`on_player_mined_entity`.

## Qué queda pendiente

- **El inventario del jugador en VR sigue sin ser interactivo.** El mod lo
  salva de dos formas (barra rápida automática y combustible automático), pero
  en general el jugador **no puede coger items con las manos**: no hay
  "coger item del suelo", "mover item al horno" ni "coger item de la caja" en
  la UI. Para que el APK sea interactivo de verdad hay que añadir esas
  acciones (con `pick_up_item` e `insert_item` ya estan los remotos del lado
  del mod).
- El visor (VR) todavia no tiene acciones para "minar veta" con el pico ni para
  "colocar maquina": el banco las llama por RCON, que es lo mismo que hara el
  cliente cuando se conecte.
- La barra rápida automática solo rellena la barra **virtual** (la del
  personaje). Con un jugador real de verdad se usa su barra nativa, que es
  del jugador y no se toca.
- El cliente grafico **sigue siendo la via buena** cuando funciona (Pico con
  Termux:X11, PC de escritorio): con el el nivel usa el jugador real y el stub
  no hace nada.
