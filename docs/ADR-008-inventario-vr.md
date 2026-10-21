# ADR-008 — Inventario con manos VR (coger del suelo y de cajas, meter en máquinas)

Estado: **aceptado** (2026-10-21)

## Por qué

La campaña ya se juega entera sin cliente gráfico (ADR-007: el mod crea un
**jugador virtual** y el escenario lo usa). Pero en VR hay **tres acciones que
el jugador no puede hacer**, porque no hay ratón ni ventana de inventario que
arrastrar:

1. **Coger un item del suelo** (la plancha que suelta la perforadora).
2. **Coger un item de una caja** (el pecio del tutorial, las cajas de la
   Factorio).
3. **Meter un item en una máquina (horno) o en una caja.**

Sin esto, el juego se puede *ver* en la mesa holográfica pero no se puede
*jugar*: el jugador se queda atascado en cuanto hay que mover un item de un
sitio a otro.

El remoto `insert_item` que ya existía **no** vale para esto: crea items
nuevos en la máquina, pero **no quita nada del actor**. Es lo que necesita el
guion de la campaña (meter cosas que el actor no tiene), no lo que necesita una
mano.

## Qué se ha hecho

### 1. Dos remotos nuevos en el mod

`mods/xfactorvr_bridge/control.lua`:

| Remoto | Qué hace |
|---|---|
| `take_from_container(x, y, item_name, count)` | Saca un item de una caja (o de los ingredientes de un horno) y lo mete en el inventario del actor. |
| `insert_carried_item(x, y, item_name, count, inv_index)` | **Traspaso real**: quita el item del actor y lo mete en la máquina o en la caja. |

Ambos contestan con `OK|<cantidad>|<nombre>` o `ERR|<motivo>` por `rcon.print`,
como los demás remotos del mod.

Decisiones que importan:

- **`item_name` vacío o `nil` = "el primer item que haya"**. En VR todavía no
  hay HUD de inventario, así que no se puede elegir *qué* item se coge: se
  coge el primero que haya.
- **El orden de los items del actor es estable** (alfabético). El contenido de
  un inventario real de Factorio es una tabla hash y sale en un orden que
  cambia de un frame a otro.
- **Primero se mete en el actor y después se quita de la caja.** Si el
  inventario del actor está lleno, el item **se queda en la caja**: no se
  pierde por el camino. Y si aun así no se puede quitar de la caja, lo que se
  había metido vuelve al actor (no se duplica nada).
- **El indice de inventario depende del tipo de entidad**, y esto no es
  intuitivo. Verificado empíricamente contra Factorio 2.1.22:

  | Entidad | Tipo | Contenido | Combustible |
  |---|---|---|---|
  | `iron-chest` / `wooden-chest` / `steel-chest` | `container` | **1** | — |
  | `stone-furnace` | `furnace` | **2** (`crafter_input`) | 1 (`fuel`) |
  | `assembling-machine-*` | `crafting-machine` | **1** | 2 |

  O sea: en una caja el contenido es el 1, pero en un horno el 1 es el
  combustible y los ingredientes son el 2. El mod lo decide por `entity.type`,
  no por "el primer índice que exista".

- **Un horno no acepta cualquier cosa**: solo funde mineral. Como en VR no se
  puede elegir el item, `insert_carried_item` **prueba los items que lleva el
  actor en orden alfabético y usa el primero que la máquina acepte** (si
  ninguno vale, contesta `ERR|no_space` y no se pierde nada: lo que no cabe
  vuelve al bolsillo).
- **Si la máquina se queda sin combustible, se le mete carbón** que lleve el
  actor (`abastecer_combustible`). Es el caso raro de meter la mano en un
  horno vacío: tiene que ponerse algo a quemar, no solo ingredientes.

### 2. Los items del suelo llegan al cliente en una sección nueva del snapshot

El item del suelo es una entidad `item-on-ground` que **no está en la tabla de
tipos compartidos** (`ENTITY_TYPES_MAP` en el mod ↔ `ENTITY_*` en Rust ↔
`enum EntityType` en C++), así que `serialize_entity` lo descarta y **nunca
llega al cliente**. Sin eso, el visor no puede saber que hay un item donde
apunta el rayo.

La solución **no** puede ser "añadir un tipo de entidad nuevo":

- `LiveEntity` es `#[repr(C)]` y está **espejada tal cual** en
  `android/src/openxr_engine.cpp`, que indexa `s_entity_meshes[entity_type]`
  con un tope de `ENTITY_TYPE_COUNT = 14`. Añadir el tipo 14 lo saltaría en el
  render (el `continue` del `>= ENTITY_TYPE_COUNT`), pero es exactamente el
  tipo de acoplamiento que ya ha roto cosas dos veces.

Por eso los items sueltos van en **una sección nueva del snapshot**:
`"loose":[{"x":6.00,"y":4.00,"n":"iron-ore","c":12}]`. Es **aditiva**: el JSON
del snapshot solo lo parsea Rust (`src/game_bridge.rs`); el motor de VR
consume FFI, no el JSON, así que **no hay que tocar nada del lado C++**.

La struct `LooseItem` vive en `src/tabletop.rs` y la lista se guarda en
`SimulationWorld.loose_items`.

### 3. La decisión de qué hacer la toma Rust, no el mod

`src/openxr/actions.rs` decide **qué** hacer según lo que hay bajo el rayo, y
`src/lib.rs` lo ejecuta. La decisión es una función pura y testeada
(`decide_vr_action`):

| Mano | Bajo el rayo hay… | Acción |
|---|---|---|
| **Gatillo** | item suelto | `pick_up_item` |
| **Gatillo** | caja | `take_from_container` (el primer item) |
| **Gatillo** | horno | `insert_carried_item` inv **2** (ingredientes) + combustible si está vacío |
| **Gripo** | caja | `insert_carried_item` inv **1** (el contenido de la caja) |
| **Gripo** | horno | `insert_carried_item` inv **2** (¡el 1 del horno es el combustible!) |
| Cualquiera | nada | **nada**: el gatillo sigue siendo el clic de ratón de siempre |

Reglas que no se rompen:

- **Si no hay nada interactuable, el gatillo no hace nada destructivo**: se
  comporta igual que antes (clic emulado).
- **La pinza a dos manos manda sobre el gripo**: si los dos mandos están
  agarrados, el gripo **no** mete items (escala de la mesa).
- **El joystick sigue desplazando el mapa** igual que antes.

### 4. FFI nuevos en `src/lib.rs`

Con la misma forma de construir el comando que los demás:

- `xfactorvr_pick_up_item(tile_x: f32, tile_y: f32) -> bool`
- `xfactorvr_take_from_container(tile_x: f32, tile_y: f32, item_name: *const c_char, count: u32) -> bool`
- `xfactorvr_insert_carried_item(tile_x: f32, tile_y: f32, item_name: *const c_char, count: u32, inv_index: u32) -> bool`
- `xfactorvr_vr_hand_action(tile_x: f32, tile_y: f32, hand: u32) -> bool` → decide
  y ejecuta (0 = gatillo, 1 = gripo).

`item_name` puede ser `NULL` o `""`: entonces significa "el primero que
haya" (en el remoto de coger) o "el que lleva el actor en la mano" (en el de
meter).

## Consecuencias

- **El enganche en el motor de VR está hecho** (`android/src/openxr_engine.cpp`).
  El motor llama a `xfactorvr_vr_hand_action(casilla_x, casilla_y, 0|1)` con la
  casilla que apunta el rayo y el botón que se ha pulsado (0 = gatillo,
  1 = gripo), **antes** de su propia lógica de ratón. Si la mano ha hecho
  algo (devuelve true), el resto del gatillo de ese pulso no se ejecuta: son
  acciones distintas sobre el mismo objetivo. Si no hay nada que hacer
  (devuelve false), el gatillo sigue siendo el ratón de siempre (colocar,
  minar, lootear) y el joystick y la pinza no se tocan.
- **El HUD de inventario** (el "plus" de la barra rápida) queda **fuera**: es
  opcional y vive en el renderizador. Con este ADR el inventario ya se puede
  *usar*; solo falta poder *verlo* con detalle.
- **La tabla de tipos de entidad sigue igual**: ni el mod ni Rust ni el C++
  cambian un solo valor de `ENTITY_TYPES_MAP`. El canal de los items sueltos
  es paralelo.
- El remoto `insert_item` **no se toca**: sigue siendo el que usa el guion de
  la campaña (crea items, no los mueve).

## Cómo se valida

`scripts/test_inventario_vr.sh` (banco nuevo) contra un Factorio headless
real. El banco usa el escenario **`freeplay`** y no el de la campaña: la
campana va dando items al jugador y llenando cajas por su cuenta (los nodos
`wait-for-chest-to-fill` y `take-items`), y eso se mezclaría con las
mediciones. Aquí se mide el inventario y nada más; la campaña la comprueba el
banco del jugador virtual, que se ejecuta al final.

1. Una **caja con items dentro** → `take_from_container` → el item **llega al
   actor** y la caja se queda con uno menos.
2. Un **item en el suelo** → `pick_up_item` → el item **llega al actor** y la
   entidad `item-on-ground` desaparece.
3. Un **item en un horno** → `insert_carried_item` → el mineral **sale del
   actor y entra en el horno** (traspaso) → y se vuelve a sacar.
4. Un **item en una caja** → `insert_carried_item` → el item sale del bolsillo
   del actor y aparece en la caja.
5. El **snapshot** lleva la sección `loose` con los items del suelo (y es JSON
   válido, como espera el cliente VR).
6. **Se vuelve a pasar `scripts/test_jugador_virtual.sh`**: 6 fases, 0 fallos.

Además, `cargo test` (34 tests, antes 26) cubre la decisión de la mano, la
clasificación de lo que hay bajo el rayo y el parseo de la sección `loose`.
