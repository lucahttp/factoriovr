#!/usr/bin/env python3
"""
preparar_campana_mp.py — Deja el nivel de campaña listo para multijugador
y para jugar SIN cliente gráfico (servidor headless).

Por qué hace falta
-----------------
El nivel de campaña es una copia de `base/campaigns/tutorial/level-01` de
Factorio 2.1.22, pensado para singleplayer. En un servidor headless el
escenario arranca SIN jugadores (el cliente gráfico se conecta después) y
el nivel peta con:

    __level__/control.lua:132: attempt to index field '?' (a nil value)
    __core__/lualib/story.lua:71: in function 'story_update'
    -> "The scenario level caused a non-recoverable error."

Ademas, el nivel usa `game.show_message_dialog`, que en multijugador falla
siempre con "Map doesn't contain 1 player, this function can't be used"
(comprobado contra el motor real, ver docs/ADR-005).

Los parches
-----------
1. `init` de la rama 'setup': si todavia no hay jugador, rearma los
   marcadores de "init ya llamado" y devuelve `true` (el módulo `story`
   aborta ese evento). El siguiente tick reintenta, asi que la historia
   arranca sola en cuanto hay alguien: el jugador real que entra con el
   cliente gráfico o el PERSONAJE VIRTUAL que crea el mod xfactorvr_bridge.

2. `game.show_message_dialog(...)` -> `story_show_message_dialog(...)`:
   el helper del módulo `story` (ya cargado por el nivel con
   `require("story")`) es el que sabe qué hacer en multijugador (imprime el
   texto al jugador en vez de abrir el diálogo de un jugador concreto).

3. Se inyecta el stub `jugador()`: una función que devuelve el jugador real
   si lo hay y, si no, el personaje virtual del mod envuelto en una tabla que
   emula la API de LuaPlayer. Con esto el nivel de campaña arranca y AVANZA
   en un servidor headless, sin nadie conectado.

4. `game.players[1]` -> `jugador()` y `game.get_player(1)` -> `jugador()`:
   todas las referencias al jugador del nivel pasan por el stub.

Es idempotente: si el control.lua ya está parcheado no lo vuelve a tocar,
y siempre deja una copia intacta en `<escenario>/control.lua.orig`.

Uso:
    python3 scripts/preparar_campana_mp.py /ruta/al/escenario
    python3 scripts/preparar_campana_mp.py /ruta/al/escenario --copiar-como nombre
"""
import argparse
import os
import shutil
import sys

# --- parche 1: esperar al jugador -------------------------------------------
ORIGINAL_INIT = """      name = 'setup',
      init = function()
        game.players[1].disable_recipe_groups()
"""

PARCHE_INIT = """      name = 'setup',
      init = function(event, st)
        -- [XFactorVR-MP] El nivel original asume singleplayer: cuando el
        -- escenario arranca ya hay un jugador. En un servidor headless no hay
        -- NINGUNO, asi que aqui se espera a que exista el jugador real o el
        -- personaje virtual que crea el mod xfactorvr_bridge.
        -- Se rearma el marcador de "init ya llamado" para que el modulo story
        -- vuelva a intentarlo en el siguiente tick.
        if not _xfv_hay_jugador() then
          -- Se avisa al mod para que lo cree ya: su on_init puede haber sido
          -- demasiado pronto para tener superficie.
          pcall(function() remote.call("xfactorvr", "get_virtual_player") end)
          st.init_called_last_on_story_index = 0
          st.init_called_last_on_position = 0
          return true
        end
        jugador().disable_recipe_groups()
"""

# --- parche 2: dialogos de multijugador -------------------------------------
ORIGINAL_DIALOG = "game.show_message_dialog("
PARCHE_DIALOG = "story_show_message_dialog("

# --- parche 3: el stub del jugador virtual ----------------------------------
#
# El stub va justo despues de los `require` del principio del control.lua.
#
# OJO: por la interfaz remota NO cruza una entidad (los objetos Lua no se
# pueden pasar por el remoto), asi que el mod devuelve el `unit_number` del
# personaje y aqui se localiza con `game.get_entity_by_unit_number` (el
# prototipo "character" del juego base lleva el flag "get-by-unit-number").

STUB_JUGADOR = '''
-- [XFactorVR-MP] ==================================================================
-- JUGADOR VIRTUAL para el nivel de campana.
--
-- El nivel de tutorial es singleplayer: su init usa game.players[1]. En un
-- servidor headless no hay nadie conectado, asi que el nivel se queda
-- esperando a un jugador que no existe (o peta). El mod xfactorvr_bridge crea
-- un personaje de la fuerza "player" y lo expone por remoto.
--
-- Este stub devuelve el jugador real si lo hay y, si no, ese personaje
-- virtual envuelto en una tabla que emula la API de LuaPlayer. Casi todos los
-- metodos son no-ops (son de la GUI del jugador, que en un servidor headless
-- no existe); los que necesitan el mundo (get_item_count, can_reach_entity,
-- cursor_stack, position, force, surface, character) leen del personaje de
-- verdad.
--
-- OJO: la info del remoto se cachea por tick. El nodo "add_filter" del
-- tutorial mira los 40 huecos de la barra rapida (4 paginas x 10) y cada
-- consulta es una llamada al remoto: sin cache serian 40 llamadas por tick.
local _xfv_cache_ok = false
local _xfv_cache_tick = nil
local _xfv_cache_info = nil

local function _xfv_info()
  if _xfv_cache_ok and _xfv_cache_tick == game.tick then
    return _xfv_cache_info
  end
  _xfv_cache_ok = true
  _xfv_cache_tick = game.tick
  _xfv_cache_info = nil
  local ok, res = pcall(function()
    return remote.call("xfactorvr", "get_virtual_player")
  end)
  if ok then _xfv_cache_info = res end
  return _xfv_cache_info
end

local function _xfv_personaje_virtual()
  local info = _xfv_info()
  if type(info) ~= "table" then return nil, nil end

  local un = info.unit_number
  if not un then return nil, info end
  local ent = nil
  pcall(function() ent = game.get_entity_by_unit_number(un) end)
  if ent and ent.valid then return ent, info end
  return nil, info
end

-- ¿Hay alguien jugando? El jugador real o el personaje virtual.
local function _xfv_hay_jugador()
  local p = game.players[1]
  if p and p.valid then return true end
  return _xfv_personaje_virtual() ~= nil
end

-- El cursor del jugador: el nivel mira `cursor_stack.valid_for_read` y
-- `cursor_stack.count`, asi que nunca puede ser nil.
local _xfv_cursor_vacio = {
  valid_for_read = false,
  valid_for_write = false,
  count = 0,
  name = "",
  empty = true,
}

-- Valores que el nivel espera encontrar EXACTAMENTE (ni nil ni true).
local _xfv_fijos = {
  name = "VR",
  tag = "xfactorvr_player",
  index = 1,
  admin = false,
  connected = false,
  opened_self = false,
  input_method = defines.input_method.keyboard_and_mouse,
  minimap_enabled = false,
  zoom = 2,
  game_view_settings = {},
}

-- player.disable_recipe_groups()/disable_recipe_subgroups() no existen para un
-- personaje: se emulan dejando la fuerza sin ninguna receta habilitada.
local function _xfv_desactivar_recetas()
  local f = game.forces[1]
  if not f or not f.recipes then return end
  pcall(function()
    for name, _ in pairs(f.recipes) do
      local r = f.recipes[name]
      if r then r.enabled = false end
    end
  end)
end

-- Metodos de la GUI del jugador que un personaje no tiene. Casi todos son
-- no-ops: al nivel solo le importa que no peten.
local _xfv_metodos = {
  set_goal_description = function() end,
  clear_recipe_notifications = function() end,
  set_controller = function() end,
  print = function() end,
  disable_recipe_groups = _xfv_desactivar_recetas,
  disable_recipe_subgroups = _xfv_desactivar_recetas,
}

-- Tabla que emula la API de LuaPlayer sobre el personaje virtual.
-- En Lua una entidad de tipo "character" es un LuaControl igual que un
-- jugador, asi que casi todo (position, surface, force, opened, selected,
-- crafting_queue_size, get_item_count, can_reach_entity, teleport...) sale
-- directamente del personaje.
--
-- `info` es lo que contesta el mod por el remoto: de ahi sale la GUI virtual
-- (un personaje no tiene GUI, pero el nivel de campana la mira).
local function _xfv_emulador(ch, info)
  -- La barra rapida virtual: un personaje no tiene barra rapida (es del
  -- jugador con raton), pero el nivel de campana mira si el jugador tiene la
  -- perforadora asignada a un hueco antes de dejarle colocarla.
  local function get_quick_bar_slot(page, slot)
    if not info or not info.quick_bar then return nil end
    local nombre = info.quick_bar[tostring(page) .. "_" .. tostring(slot)]
    if not nombre then return nil end
    return { type = "filter", filter = { name = nombre } }
  end

  return setmetatable({}, {
    __index = function(_, k)
      -- `character` es el propio personaje (un jugador tiene su personaje).
      if k == "character" then
        if ch and ch.valid then return ch end
        return nil
      end
      -- La GUI del jugador virtual (el personaje no tiene ninguna).
      if k == "opened_gui_type" then
        if info and info.gui_type ~= nil then return info.gui_type end
      end
      -- La cola de fabricacion virtual (el personaje no puede usar la real:
      -- no tiene controlador, asi que el juego no le fabrica nada).
      if k == "crafting_queue_size" then
        if info and info.crafting_queue_size and info.crafting_queue_size > 0 then
          return info.crafting_queue_size
        end
      end
      -- Propiedades vivas del personaje.
      if ch and ch.valid then
        local ok, v = pcall(function() return ch[k] end)
        if ok and v ~= nil then return v end
      end
      local fijo = _xfv_fijos[k]
      if fijo ~= nil then return fijo end
      if k == "get_quick_bar_slot" then return get_quick_bar_slot end
      local m = _xfv_metodos[k]
      if m ~= nil then return m end
      -- El cursor nunca puede ser nil.
      if k == "cursor_stack" then return _xfv_cursor_vacio end
      return nil
    end,
    __newindex = function(_, k, v)
      -- Las escrituras (zoom, minimap_enabled, game_view_settings) son de la
      -- GUI del jugador: se intentan y, si el personaje no las tiene, se
      -- ignoran sin romper nada.
      if ch and ch.valid then
        pcall(function() ch[k] = v end)
      end
    end,
  })
end

-- El "jugador" del nivel: el real si hay alguien, si no el virtual.
-- Puede devolver nil si todavia no hay personaje (el init del nivel lo
-- comprueba antes de usarlo).
local function jugador()
  local p = game.players[1]
  if p and p.valid then return p end
  local ch, info = _xfv_personaje_virtual()
  if not ch then return nil end
  return _xfv_emulador(ch, info)
end

-- [XFactorVR-MP] El ULTIMO nodo del tutorial (show-miner-blockage) espera
-- este evento:
--
--     if event.name == defines.events.on_player_mined_entity
--        and event.entity.name == 'item-on-ground' then
--
-- Ese evento es del JUEGO, no del script: solo se lanza si hay un JUGADOR REAL
-- (con su controlador). Un personaje virtual no lo lanza nunca, por mucho que
-- mine la entidad del suelo, asi que la historia se queda clavada ahi.
--
-- El mod anota cuando el jugador virtual recoge un item del suelo y lo
-- cuenta por el remoto. Aqui se le pasa a la historia el mismo evento que
-- habria lanzado el juego, con la entidad "item-on-ground".
local function _xfv_minado_pendiente(tick)
  -- Lo que ya se sabe y todavia no ha servido (puede que el nodo no estuviera
  -- listo cuando llego el minado).
  if storage.xfv_minado then return storage.xfv_minado end
  -- No se pregunta en cada tick (son 60 por segundo): con 5 va sobrado.
  if not tick or (tick % 5) ~= 0 then return nil end
  local ok, info = pcall(function()
    return remote.call("xfactorvr", "get_mined_item")
  end)
  if ok and type(info) == "table" and info.name then
    storage.xfv_minado = info
    return info
  end
  return nil
end

-- [XFactorVR-MP] El nivel solo avanza cuando le llega un evento del juego, y
-- casi todos los que espera son de JUGADOR (seleccionar un mineral con el
-- raton, abrir la GUI de fabricacion, minar...). Un personaje virtual no
-- tiene controlador de GUI, asi que esos eventos no le llegan NUNCA y la
-- historia se queda clavada.
--
-- Solucion: se envuelve story_update para que, cuando no hay ningun jugador
-- real conectado, le pase a la historia un evento sintetico de "accion del
-- jugador" en cada tick. Las condiciones de los nodos son de ESTADO (selected,
-- get_item_count, la distancia...), asi que el tutorial avanza en cuanto el
-- jugador virtual ha hecho lo que el nodo pide. Con un jugador real conectado
-- esto NO se hace: el nivel se comporta exactamente igual que siempre.
--
-- OJO: no se puede registrar otro script.on_event(on_tick): en Factorio solo
-- hay un manejador por evento y por script, asi que el del nivel (que
-- incluye on_tick) machacaria el nuestro.
if story_update then
  local _xfv_story_update_real = story_update
  story_update = function(st, event, next_level, onwin)
    local r = _xfv_story_update_real(st, event, next_level, onwin)
    if not game.players[1] and event.name == defines.events.on_tick then
      -- El minado del jugador virtual: se le pasa a la historia el evento
      -- on_player_mined_entity con la entidad "item-on-ground", que es
      -- exactamente lo que lanzaria el juego con un jugador de verdad.
      local pendiente = _xfv_minado_pendiente(event.tick)
      if pendiente then
        local antes = st.story_position
        _xfv_story_update_real(st, {
          name = defines.events.on_player_mined_entity,
          entity = { name = pendiente.name, position = pendiente.position },
          tick = event.tick,
        }, next_level, onwin)
        -- Si la historia ha avanzado, el evento ya ha servido: se limpia.
        if st.story_position ~= antes then
          storage.xfv_minado = nil
        end
      end
      _xfv_story_update_real(st, { name = "script_raised_built", tick = event.tick },
                             next_level, onwin)
    end
    return r
  end
end
-- [XFactorVR-MP] ==================================================================
'''

# El stub se inyecta despues de la ultima linea de `require` del principio.


def parche_init(texto: str):
    """Espera al jugador. Devuelve (texto, aplicado)."""
    if "-- [XFactorVR-MP]" in texto:
        return texto, False
    if ORIGINAL_INIT not in texto:
        print("  AVISO: no encuentro el patron de 'setup' (¿nivel nuevo?)")
        return texto, False
    return texto.replace(ORIGINAL_INIT, PARCHE_INIT, 1), True


def parche_dialog(texto: str):
    """show_message_dialog -> story_show_message_dialog. (texto, aplicado)"""
    if PARCHE_DIALOG in texto:
        return texto, False
    n = texto.count(ORIGINAL_DIALOG)
    if n == 0:
        return texto, False
    return texto.replace(ORIGINAL_DIALOG, PARCHE_DIALOG), True


def parche_stub(texto: str):
    """Inyecta el stub `jugador()`. Devuelve (texto, aplicado)."""
    if "_xfv_personaje_virtual" in texto:
        return texto, False
    lineas = texto.split("\n")
    # Punto de insercion: despues de la ultima linea `require` del principio.
    ultimo = None
    for i, linea in enumerate(lineas[:40]):
        s = linea.strip()
        if s.startswith("require"):
            ultimo = i
    if ultimo is None:
        print("  AVISO: no hay `require` donde colgar el stub")
        return texto, False
    nuevas = (
        lineas[: ultimo + 1]
        + [""]
        + STUB_JUGADOR.strip("\n").split("\n")
        + [""]
        + lineas[ultimo + 1 :]
    )
    return "\n".join(nuevas), True


def parche_referencias(texto: str):
    """game.players[1] y game.get_player(1) -> jugador(). (texto, aplicado)
    OJO: no se toca el stub ya inyectado: el stub usa game.players[1] a
    proposito para saber si hay un jugador real de verdad."""
    marca = "-- [XFactorVR-MP] ===="
    ini = texto.find(marca)
    fin = texto.find(marca, ini + 1) if ini >= 0 else -1
    if ini >= 0 and fin > ini:
        antes, stub, despues = texto[:ini], texto[ini:fin], texto[fin:]
    else:
        antes, stub, despues = texto, "", ""

    n1 = antes.count("game.players[1]")
    n2 = antes.count("game.get_player(1)")
    if n1 == 0 and n2 == 0:
        return texto, False
    antes = antes.replace("game.players[1]", "jugador()")
    antes = antes.replace("game.get_player(1)", "jugador()")
    print(f"    · {n1} referencias a game.players[1] y {n2} a game.get_player(1) -> jugador()")
    return antes + stub + despues, True


def parchear(control_lua: str) -> bool:
    """Parchea el control.lua in situ. Devuelve True si lo ha cambiado."""
    with open(control_lua, encoding="utf-8") as fh:
        original = fh.read()

    texto, c1 = parche_init(original)
    texto, c2 = parche_dialog(texto)
    # OJO: la sustitucion de referencias va ANTES de inyectar el stub, para
    # que el stub no se reescriba a si mismo (menciona game.players[1] en sus
    # comentarios).
    texto, c4 = parche_referencias(texto)
    texto, c3 = parche_stub(texto)

    if not (c1 or c2 or c3 or c4):
        print(f"  ya parcheado: {control_lua}")
        return False

    # Copia intacta por si hay que deshacer el parche.
    orig = control_lua + ".orig"
    if not os.path.exists(orig):
        shutil.copy2(control_lua, orig)
        print(f"  copia intacta: {orig}")

    with open(control_lua, "w", encoding="utf-8") as fh:
        fh.write(texto)
    print(f"  parcheado: {control_lua}")
    if c1:
        print("    · espera al jugador antes de arrancar la historia")
    if c2:
        print("    · show_message_dialog -> story_show_message_dialog (multijugador)")
    if c3:
        print("    · stub del jugador virtual (jugador()) inyectado")
    if c4:
        pass  # ya lo ha dicho parche_referencias
    return True


def copiar_como(origen: str, destino: str) -> None:
    """Copia el escenario a otro nombre (para no tocar el original)."""
    if os.path.exists(destino):
        print(f"  ya existe, no se copia: {destino}")
        return
    os.makedirs(destino, exist_ok=True)
    for nombre in ("control.lua", "blueprint.zip", "description.json"):
        src = os.path.join(origen, nombre)
        if os.path.exists(src):
            shutil.copy2(src, os.path.join(destino, nombre))
    locale = os.path.join(origen, "locale")
    if os.path.isdir(locale):
        shutil.copytree(locale, os.path.join(destino, "locale"), dirs_exist_ok=True)
    print(f"  escenario copiado: {destino}")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("escenario", help="directorio del escenario (el que tiene control.lua)")
    ap.add_argument("--copiar-como", default=None,
                    help="nombre de un escenario nuevo al que copiar (en el mismo padre)")
    args = ap.parse_args()

    escenario = os.path.abspath(args.escenario)
    control = os.path.join(escenario, "control.lua")
    if not os.path.isfile(control):
        print(f"ERROR: no existe {control}")
        return 2

    print(f"Escenario: {escenario}")
    if args.copiar_como:
        padre = os.path.dirname(escenario)
        destino = os.path.join(padre, args.copiar_como)
        copiar_como(escenario, destino)
        escenario = destino
        control = os.path.join(escenario, "control.lua")

    parchear(control)
    return 0


if __name__ == "__main__":
    sys.exit(main())
