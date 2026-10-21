-- xfactorvr_bridge/control.lua
-- Puente entre el motor real de Factorio 2.0 y el tabletop OpenXR de XFactorVR.
--
-- El visor (Pico 4 Ultra) no ejecuta Factorio: habla con el servidor headless
-- por RCON usando la interfaz remota "xfactorvr". Este modulo:
--   1. Serializa el estado real de la fabrica (entidades, yacimientos, inventario).
--   2. Ejecuta las acciones de gameplay que llegan desde el visor
--      (colocar, minar, interactuar, fabricar).
--
-- Sistemas de direccion: Factorio 2.0 usa 16 direcciones (0 = norte, 4 = este,
-- 8 = sur, 12 = oeste) y los radianes se obtienen con dir * TAU/16.
-- Ver factorio-3d-mod/src/entities.rs (dir_to_yaw) para la referencia.

local ENTITY_TYPES_MAP = {
    ["assembling-machine-1"] = 1,
    ["assembling-machine-2"] = 1,
    ["assembling-machine-3"] = 1,
    ["electric-mining-drill"] = 2,
    ["burner-mining-drill"] = 2,
    ["offshore-mining-drill"] = 2,
    ["stone-furnace"] = 3,
    ["steel-furnace"] = 3,
    ["electric-furnace"] = 3,
    ["campfire"] = 3,
    ["gun-turret"] = 4,
    ["laser-turret"] = 4,
    ["radar"] = 5,
    ["roboport"] = 6,
    ["wooden-chest"] = 7,
    ["iron-chest"] = 7,
    ["steel-chest"] = 7,
    ["transport-chest"] = 7,
    ["small-electric-pole"] = 8,
    ["medium-electric-pole"] = 8,
    ["big-electric-pole"] = 8,
    ["substation"] = 8,
    ["transport-belt"] = 9,
    ["fast-transport-belt"] = 9,
    ["express-transport-belt"] = 9,
    ["underground-belt"] = 9,
    ["splitter"] = 9,
    ["inserter"] = 10,
    ["fast-inserter"] = 10,
    ["long-handed-inserter"] = 10,
    ["stack-inserter"] = 10,
    ["stack-inserter-1"] = 10,
}

-- Ids de item: deben coincidir con las constantes ITEM_* de src/game_bridge.rs.
local ITEM_IDS_MAP = {
    ["iron-ore"] = 1,
    ["copper-ore"] = 2,
    ["coal"] = 3,
    ["stone"] = 4,
    ["iron-plate"] = 5,
    ["copper-plate"] = 6,
    ["iron-gear-wheel"] = 7,
    ["electronic-circuit"] = 8,
    ["assembling-machine-1"] = 9,
    ["assembling-machine-2"] = 9,
    ["assembling-machine-3"] = 9,
    ["electric-mining-drill"] = 10,
    ["burner-mining-drill"] = 10,
    ["stone-furnace"] = 11,
    ["steel-furnace"] = 11,
    ["electric-furnace"] = 11,
    ["transport-belt"] = 12,
    ["fast-transport-belt"] = 12,
    ["express-transport-belt"] = 12,
    ["inserter"] = 13,
    ["fast-inserter"] = 13,
    ["long-handed-inserter"] = 13,
    ["small-electric-pole"] = 14,
    ["medium-electric-pole"] = 14,
    ["gun-turret"] = 15,
    ["radar"] = 16,
    ["roboport"] = 17,
    ["wooden-chest"] = 18,
    ["iron-chest"] = 18,
    ["steel-chest"] = 18,
    ["copper-cable"] = 19,
    ["iron-stick"] = 20,
    ["stone-brick"] = 21,
    ["steel-plate"] = 22,
    ["automation-science-pack"] = 23,
    ["firearm-magazine"] = 24,
    ["solid-fuel"] = 25,
    ["lubricant"] = 26,
    ["sulfuric-acid"] = 27,
    ["petroleum-gas"] = 28,
    ["plastic-bar"] = 29,
    ["battery"] = 30,
    ["advanced-circuit"] = 31,
    ["processing-unit"] = 32,
    ["engine-unit"] = 33,
    ["electric-engine-unit"] = 34,
    ["flying-robot-frame"] = 35,
    ["low-density-structure"] = 36,
    ["rocket-fuel"] = 37,
    ["uranium-235"] = 38,
    ["uranium-238"] = 39,
    ["satellite"] = 40,
}

-- Recetas que rota el jugador al interactuar con una ensambladora.
local RECIPE_ROTATION = {
    "iron-gear-wheel",
    "electronic-circuit",
    "transport-belt",
    "inserter",
    "automation-science-pack",
    "firearm-magazine",
}

-- ---------------------------------------------------------------------------
-- Direcciones (Factorio 2.0: 16 direcciones, defines.direction.east == 4)
-- ---------------------------------------------------------------------------

local EAST_STEP = defines.direction.east or 4
local TAU = 2.0 * math.pi
local QUARTER = math.pi * 0.5

--- Convierte una direccion de Factorio (0..15) a radianes.
local function direction_to_rad(dir)
    return (dir / (EAST_STEP * 4.0)) * TAU
end

--- Convierte radianes a la direccion cardinal mas cercana (0, 4, 8 u 12).
local function rad_to_direction(rad)
    local quarter = math.floor(((rad / QUARTER) % 4.0) + 0.5) % 4.0
    return (quarter * EAST_STEP) % 16
end

-- ---------------------------------------------------------------------------
-- Inventario virtual
--
-- El servidor headless arranca sin jugador conectado, asi que game.players[1]
-- suele ser nil. Para que el juego sea jugable igual, el modulo mantiene un
-- inventario propio persistido en `storage`. Si alguien conecta un cliente
-- real, se usa el inventario de ese jugador.
-- ---------------------------------------------------------------------------

local START_INVENTORY = {
    ["burner-mining-drill"] = 1,
    ["stone-furnace"] = 1,
    ["iron-plate"] = 8,
    ["coal"] = 1,
}

local function real_player()
    local p = game.players[1]
    if p and p.valid then return p end
    return nil
end

--- Estado persistente del mod.
--- Factorio 2.0 elimino la tabla global en favor de storage; usar `global`
--- en 2.x provoca un error no recuperable en on_init que impide arrancar la
--- partida (detectado con el servidor real en contenedor ARM64).
local function state()
    return storage
end

-- ---------------------------------------------------------------------------
-- Jugador virtual
--
-- El nivel de campana (base/campaigns/tutorial/level-01) es singleplayer: su
-- init usa game.players[1]. En un servidor headless no hay nadie conectado,
-- asi que el nivel nunca arranca su logica y la campana no se puede jugar.
-- Para que se pueda jugar sin cliente grafico, este mod crea un PERSONAJE de
-- la fuerza "player" y lo deja disponible para el escenario.
--
-- En la API de Factorio una entidad de tipo "character" es un LuaControl,
-- igual que un jugador: tiene position, surface, force, teleport(),
-- mine_entity(), can_reach_entity(), get_item_count() y get_main_inventory().
-- Por eso el personaje virtual se mueve, mina, fabrica e interactua con las
-- maquinas exactamente igual que el jugador real.
--
-- OJO: LuaForce.create_character NO existe en 2.1.22 (la API actual solo
-- tiene LuaPlayer.create_character, que exige un jugador ya conectado). La
-- via que usa el propio juego base es surface.create_entity{name="character"}
-- (ver base/script/team-production/team_production.lua).
-- ---------------------------------------------------------------------------

local VIRTUAL_TAG = "xfactorvr_player"
local VIRTUAL_NAME = "VR"

-- Tipos de GUI que usa el jugador virtual. Se leen con reserva porque el
-- banco de pruebas unitarios no define `defines.gui_type`.
local GUI_NONE = (defines.gui_type and defines.gui_type.none) or 0
local GUI_CONTROLLER = (defines.gui_type and defines.gui_type.controller) or 3

--- Entidad del personaje virtual, o nil si todavia no existe.
local function virtual_character()
    local st = state()
    local c = st.xfactorvr_character
    if c and c.valid then return c end
    st.xfactorvr_character = nil
    return nil
end

--- Crea el personaje virtual si no hay jugador real. Devuelve la entidad o nil.
local function ensure_virtual_character()
    -- Si hay un jugador de verdad, el escenario usa ese: no creamos nada.
    if real_player() then return nil end

    local c = virtual_character()
    if c then return c end

    local surface = game.surfaces[1]
    if not surface then return nil end
    local force = game.forces[1]
    if not force then return nil end

    -- Punto de aparicion de la fuerza en esa superficie.
    local pos = { 0, 0 }
    local ok_spawn, spawn = pcall(function() return force.get_spawn_position(surface) end)
    if ok_spawn and spawn then
        if spawn.x then
            pos = { spawn.x, spawn.y }
        elseif spawn[1] then
            pos = { spawn[1], spawn[2] }
        end
    end

    local function crear(p)
        local ok, ent = pcall(function()
            return surface.create_entity{
                name = "character",
                position = p,
                force = force,
                direction = defines.direction.south,
            }
        end)
        if ok and ent then return ent end
        return nil
    end

    local ent = crear(pos)
    if not ent then
        -- El punto de aparicion puede estar ocupado: se busca hueco cerca.
        local ok_free, libre = pcall(function()
            return surface.find_non_colliding_position("character", pos, 10, 2)
        end)
        if ok_free and libre then
            ent = crear(libre)
        end
    end
    if not ent then return nil end

    -- Nombre visible (el "tag" de un jugador no existe en una entidad: el
    -- nombre visible es la mejor aproximacion y es lo que se ve en el mapa).
    pcall(function() ent.name_tag = VIRTUAL_NAME end)

    state().xfactorvr_character = ent

    local pos = ent.position or { x = 0, y = 0 }
    log("[XFactorVR] Jugador virtual '" .. VIRTUAL_NAME .. "' creado en " ..
        tostring(pos.x) .. "," .. tostring(pos.y) .. " (unidad " ..
        tostring(ent.unit_number) .. ")")

    return ent
end

--- El actor sobre el que actuan las ordenes: el jugador real si hay alguno
--- conectado y, si no, el personaje virtual.
local function actor()
    local p = real_player()
    if p then return p end
    return ensure_virtual_character()
end

--- Estado virtual de la GUI del jugador virtual.
---
--- Un personaje NO tiene GUI: el juego solo se la da a un jugador con raton
--- (y el nivel de campana mira `opened_gui_type` y `crafting_queue_size`).
--- El mod lo emula: cuando el jugador virtual fabrica algo, esta "en la GUI
--- de fabricacion" con el item en la cola, que es justo lo que el tutorial
--- espera ver.
local function virtual_gui()
    local st = state()
    if not st.xfactorvr_gui then
        st.xfactorvr_gui = { type = nil, crafts = 0, caduca = 0 }
    end
    return st.xfactorvr_gui
end

--- El inventario principal del actor (jugador real o personaje virtual).
local function actor_inventory()
    local a = actor()
    if not a then return nil end
    local ok, inv = pcall(function() return a.get_main_inventory() end)
    if ok then return inv end
    return nil
end

--- Fuerza activa: la del actor si hay alguno (jugador real o personaje
--- virtual) y, si no, la primera del juego. En Factorio 2.0 las recetas ya no
--- cuelgan de `game.recipe_prototypes` sino de `force.recipes`, asi que hace
--- falta una fuerza de referencia.
local function active_force()
    local a = actor()
    if a and a.valid and a.force then return a.force end
    return game.forces[1]
end

local function virtual_inventory()
    local st = state()
    if not st.xfactorvr_inventory then
        st.xfactorvr_inventory = {}
        for name, count in pairs(START_INVENTORY) do
            st.xfactorvr_inventory[name] = count
        end
    end
    return st.xfactorvr_inventory
end

--- Numero de unidad del actor (para no confundirse con el al buscar
--- entidades cerca: el propio personaje esta ahi mismo).
local function actor_unit_number(a)
    if not a then return nil end
    local ok, un = pcall(function() return a.unit_number end)
    if ok and un then return un end
    local ok2, ch = pcall(function() return a.character end)
    if ok2 and ch then
        local ok3, un2 = pcall(function() return ch.unit_number end)
        if ok3 then return un2 end
    end
    return nil
end

--- La entidad valida mas cercana al punto, SIN contar el propio actor.
--- Sin esto, interactuar "en la posicion del jugador" se encontraba al
--- propio personaje y le vaciaba su propio inventario.
local function entidad_mas_cercana(found, x, y, actor_un)
    local mejor, mejor_dist = nil, nil
    for _, e in ipairs(found) do
        if e.valid then
            local mismo = false
            if actor_un then
                local ok, un = pcall(function() return e.unit_number end)
                if ok and un == actor_un then mismo = true end
            end
            if not mismo then
                local dx = e.position.x - x
                local dy = e.position.y - y
                local d = dx * dx + dy * dy
                if not mejor_dist or d < mejor_dist then
                    mejor, mejor_dist = e, d
                end
            end
        end
    end
    return mejor
end

--- Normaliza el contenido de un inventario a una lista de
--- {name, count, quality}.
--- OJO: en Factorio 2.0 `LuaInventory.get_contents` devuelve una LISTA de
--- ItemWithQualityCount, no un mapa nombre -> cantidad. Recorrerlo con
--- `for name, value in pairs(...)` mete en el inventario items con el nombre
--- del indice (1, 2, 3...), que es justo lo que pasaba antes: los items
--- saqueados se perdian. Se soportan las dos formas por si acaso.
local function inventory_contents(inv)
    local lista = {}
    if not inv then return lista end
    local ok, contents = pcall(function() return inv.get_contents() end)
    if not (ok and contents) then return lista end
    for key, value in pairs(contents) do
        if type(value) == "table" then
            if value.name and value.count then
                table.insert(lista, {
                    name = value.name,
                    count = value.count,
                    quality = value.quality,
                })
            end
        elseif type(key) == "string" and type(value) == "number" then
            table.insert(lista, { name = key, count = value })
        end
    end
    return lista
end

--- Quita `count` items de un inventario concreto.
--- En Factorio 2.0 `LuaInventory.remove` espera un ItemStackIdentification
--- ({name=, count=}); la forma antigua (nombre, cantidad) se deja como
--- respaldo porque hay builds que la aceptan.
local function inv_take(inv, name, count, quality)
    if not inv or count <= 0 then return 0 end
    local spec = { name = name, count = count }
    if quality then spec.quality = quality end
    local ok, removed = pcall(function()
        return inv.remove(spec)
    end)
    if ok and removed and removed > 0 then return removed end
    local ok2, removed2 = pcall(function() return inv.remove(name, count) end)
    if ok2 and removed2 and removed2 > 0 then return removed2 end
    return 0
end

--- Cantidad de un item en el inventario activo.
--- El inventario activo es el del jugador real si lo hay y, si no, el del
--- personaje virtual (que es un inventario de verdad, asi que el nivel de
--- campana ve los mismos items con get_item_count).
local function inv_get(name)
    local inv = actor_inventory()
    if inv then
        local ok, count = pcall(function() return inv.get_item_count(name) end)
        if ok and count then return count end
    end
    return virtual_inventory()[name] or 0
end

--- Anade items al inventario activo. Devuelve cuantos se pudieron agregar.
local function inv_add(name, count, quality)
    if count <= 0 then return 0 end
    local inv = actor_inventory()
    if inv then
        local spec = { name = name, count = count }
        if quality then spec.quality = quality end
        local ok, inserted = pcall(function()
            return inv.insert(spec)
        end)
        if ok and inserted then return inserted end
    end
    local v = virtual_inventory()
    v[name] = (v[name] or 0) + count
    return count
end

--- Quita items del inventario activo. Devuelve cuantos se quitaron.
local function inv_remove(name, count)
    if count <= 0 then return 0 end
    local inv = actor_inventory()
    if inv then
        local have = inv_get(name)
        local take = math.min(have, count)
        if take > 0 then
            -- Antes se usaba `remove_item`, que no existe en 2.0: el pcall lo
            -- silenciaba y los items nunca se consumian del inventario.
            local removed = inv_take(inv, name, take)
            if removed > 0 then return removed end
        end
    end    local v = virtual_inventory()
    local have = v[name] or 0
    local take = math.min(have, count)
    if take > 0 then
        v[name] = have - take
    end
    return take
end

--- Quita el primero de varios nombres alternativos que este disponible.
local function inv_remove_any(names, count)
    for _, name in ipairs(names) do
        if inv_get(name) > 0 then
            local taken = inv_remove(name, count)
            if taken > 0 then return taken end
        end
    end
    return 0
end

-- ---------------------------------------------------------------------------
-- Barra rapida virtual
--
-- En VR el inventario del jugador se DIBUJA pero no es interactivo: no se
-- pueden coger items con las manos. El tutorial de campana tiene un nodo
-- ("add_filter") que exige que la perforadora este en la barra rapida como
-- filtro. Con esta ayuda el mod lo pone solo: cuando el actor lleva un item
-- que el nivel puede usar de la barra, lo mete en la primera ranura libre y
-- el nodo se resuelve sin tocar la UI del visor.
--
-- OJO: NUNCA se pisa una ranura que el jugador haya puesto a mano. Esas
-- ranuras se apuntan en `xfactorvr_bar_manual` y el mod automatico no las
-- toca (solo rellena las vacias y las que el propio mod habia rellenado).
-- ---------------------------------------------------------------------------

-- Items que el nivel de campana puede necesitar en la barra rapida.
local QUICK_BAR_ITEMS = {
    "burner-mining-drill",   -- el filtro que pide el nodo "add_filter"
    "stone-furnace",
    "iron-chest",
    "wooden-chest",
    "iron-ore",
    "copper-ore",
    "coal",
    "stone",
    "iron-plate",
    "copper-plate",
    "iron-gear-wheel",
}

--- La barra rapida virtual y el registro de ranuras puestas a mano.
local function barra_rapida()
    local st = state()
    if not st.xfactorvr_bar then st.xfactorvr_bar = {} end
    if not st.xfactorvr_bar_manual then st.xfactorvr_bar_manual = {} end
    return st.xfactorvr_bar, st.xfactorvr_bar_manual
end

--- ¿Esta este nombre en la lista?
local function esta_en_lista(lista, nombre)
    for _, n in ipairs(lista) do
        if n == nombre then return true end
    end
    return false
end

--- ¿Esta este item en la barra?
local function esta_en_barra(bar, nombre)
    for _, n in pairs(bar) do
        if n == nombre then return true end
    end
    return false
end

--- La primera ranura libre de la barra (nil si no queda ninguna).
local function primera_ranura_libre(bar, manual)
    for page = 1, 4 do
        for slot = 1, 10 do
            local clave = tostring(page) .. "_" .. tostring(slot)
            if bar[clave] == nil and not manual[clave] then
                return clave
            end
        end
    end
    return nil
end

--- Actualiza la barra rapida virtual con lo que lleva el actor.
---
--- Rellena las ranuras libres con los items que lleva el actor (el real o el
--- virtual) y vacia las que el propio mod habia rellenado y que ya no se
--- llevan. Las ranuras puestas a mano no se tocan.
local function actualizar_barra_rapida()
    local bar, manual = barra_rapida()

    -- 1. Lo que lleva el actor.
    local carried = {}
    for _, nombre in ipairs(QUICK_BAR_ITEMS) do
        if inv_get(nombre) > 0 then
            carried[#carried + 1] = nombre
        end
    end
    if #carried == 0 then return end

    -- 2. Las ranuras automaticas cuyo item ya no se lleva se vacian.
    for clave, nombre in pairs(bar) do
        if not manual[clave] and not esta_en_lista(carried, nombre) then
            bar[clave] = nil
        end
    end

    -- 3. Los items que faltan van a la primera ranura libre.
    for _, nombre in ipairs(carried) do
        if not esta_en_barra(bar, nombre) then
            local clave = primera_ranura_libre(bar, manual)
            if clave then
                bar[clave] = nombre
            end
        end
    end
end

-- ---------------------------------------------------------------------------
-- Combustible de las maquinas
--
-- En VR el inventario no es interactivo, asi que el jugador no puede meter
-- carbon en el horno a mano. El nodo "put-coal" del tutorial lo espera, asi
-- que al interactuar con una maquina el mod le mete el combustible que lleva
-- el actor.
-- ---------------------------------------------------------------------------

--- Mete carbon en una maquina (horno, perforadora de combustible) si el actor
--- lleva carbon y la maquina no tiene. Devuelve cuantos items se metieron.
local function abastecer_combustible(e)
    if not e or not e.valid then return 0 end

    local ok_fuel, fuel_inv = pcall(function() return e.get_fuel_inventory() end)
    if not (ok_fuel and fuel_inv) then return 0 end

    local ok_count, count = pcall(function() return fuel_inv.get_item_count("coal") end)
    if not (ok_count and count) then return 0 end
    if count >= 1 then return 0 end

    if inv_get("coal") <= 0 then return 0 end

    local removed = inv_remove("coal", 1)
    if removed <= 0 then return 0 end

    local ok, inserted = pcall(function()
        return fuel_inv.insert({ name = "coal", count = 1 })
    end)
    if not (ok and inserted and inserted > 0) then
        -- Si no se ha podido meter, el carbon vuelve al bolsillo.
        inv_add("coal", 1)
        return 0
    end
    return inserted
end

-- ---------------------------------------------------------------------------
-- Recoger items del suelo
--
-- El juego solo lanza on_player_mined_entity cuando hay un JUGADOR REAL (con
-- su controlador). Un personaje virtual no lo lanza nunca, asi que el ultimo
-- nodo del tutorial ("show-miner-blockage") se queda clavado. El mod anota
-- aqui el minado y el escenario parcheado se lo pasa a la historia.
-- ---------------------------------------------------------------------------

--- Recoge un item del suelo (entidad "item-on-ground") y lo mete en el
--- inventario del actor. Devuelve true si el item ha acabado en el
--- inventario.
---
--- OJO: hay dos trampas aqui (las dos costaron una tarde):
---
---  1. La API de 2.1 NO tiene `entity.item`: la pila del item del suelo
---     esta en `entity.stack`. Con `item` el pcall se come el error y el
---     item se pierde.
---  2. `mine_entity` NO borra la entidad al instante: el personaje "coge"
---     el item y la entidad sigue viva un par de ticks. Por eso no se puede
---     mirar si la entidad sigue viva, sino lo que devuelve mine_entity.
---
--- Con un JUGADOR DE VERDAD se deja que lo haga el motor (asi el juego
--- lanza sus eventos de verdad). Con el PERSONAJE VIRTUAL se coge a mano:
--- el motor lo hace de forma asincrona y a veces no llega.
local function recoger_item_del_suelo(e)
    local p = actor()
    if not p then return false end

    -- 1) Camino normal (solo con jugador real): que lo haga el motor.
    if real_player() then
        local ok, minado = pcall(function() return p.mine_entity(e, true) end)
        if ok and minado then return true end
    end

    -- 2) A mano: se lee la pila del item y se mete en el inventario.
    local ok_stack, stack = pcall(function() return e.stack end)
    if not (ok_stack and stack) then return false end

    local ok_nombre, nombre = pcall(function() return stack.name end)
    local ok_cant, cantidad = pcall(function() return stack.count end)
    if not (ok_nombre and ok_cant) then return false end
    if not nombre or not cantidad or cantidad <= 0 then
        pcall(function() e.destroy() end)
        return false
    end

    local inv = actor_inventory()
    if not inv then return false end

    local spec = { name = nombre, count = cantidad }
    local ok_cal, calidad = pcall(function() return stack.quality end)
    if ok_cal and calidad then spec.quality = calidad end

    local ok_ins, inserted = pcall(function() return inv.insert(spec) end)
    if not (ok_ins and inserted and inserted > 0) then return false end

    -- El item ya esta en la mochila: la entidad del suelo sobra.
    pcall(function() e.destroy() end)
    return true
end

--- Anota que el actor ha recogido un item del suelo.
---
--- El escenario parcheado pregunta por el remoto "get_mined_item" y le pasa
--- a la historia el mismo evento (on_player_mined_entity) que habria
--- lanzado el juego.
local function anotar_minado(nombre, pos)
    state().xfactorvr_minado = {
        name = nombre,
        position = pos,
        tick = game.tick,
    }
end

--- Busca el item del suelo mas cercano a un punto. Devuelve la entidad o nil.
---
--- OJO: se filtra por nombre ("item-on-ground") y no por "lo que haya": al
--- lado del item esta la perforadora, y si se mina lo que se pille se
--- demoleria.
local function buscar_item_del_suelo(surface, x, y)
    if not surface then return nil end
    local found = surface.find_entities_filtered({
        position = { x, y },
        radius = 1.0,
        name = "item-on-ground",
    })
    for _, e in ipairs(found) do
        if e.valid then return e end
    end
    return nil
end

--- Vacia un inventario de una entidad en el inventario del actor.
--- Devuelve cuantos items se han movido.
local function loot_inventory(inv)
    local cogido = 0
    for _, stack in ipairs(inventory_contents(inv)) do
        if stack.count and stack.count > 0 then
            local taken = inv_add(stack.name, stack.count, stack.quality)
            if taken > 0 then
                cogido = cogido + taken
                -- Del origen solo se quitan los items que se han llevado.
                inv_take(inv, stack.name, taken, stack.quality)
            end
        end
    end
    return cogido
end

-- ---------------------------------------------------------------------------
-- Serializacion del mundo
-- ---------------------------------------------------------------------------

local MAX_ENTITIES = 3000
local RESOURCE_CELL = 4.0

local function status_of(e)
    -- `status` solo existe en algunas entidades y leerlo puede fallar; se
    -- protege igual que el resto de propiedades opcionales.
    local ok, s = pcall(function() return e.status end)
    if not ok or not s then return 0 end
    if s == defines.entity_status.working then return 1 end
    if s == defines.entity_status.low_power then return 2 end
    if s == defines.entity_status.no_fuel then return 3 end
    if s == defines.entity_status.no_ingredients then return 4 end
    if s == defines.entity_status.disabled_by_control_behavior then return 5 end
    return 0
end

local function health_of(e)
    -- En Factorio 2.0 el maximo de vida es `max_health`; `health_max` ya no
    -- existe y devolvia nil, dejando siempre la salud al 100%.
    local ok_ratio, ratio = pcall(function() return e.get_health_ratio() end)
    if ok_ratio and ratio then
        if ratio < 0 then ratio = 0 elseif ratio > 1 then ratio = 1 end
        return ratio
    end

    local max = e.max_health
    local cur = e.health
    if max and cur and max > 0 then
        local r = cur / max
        if r < 0 then r = 0 elseif r > 1 then r = 1 end
        return r
    end
    return 1.0
end

--- Tipo de entidad de juego segun el mapa (0 = no soportada).
local function entity_type_of(e)
    local t = e.type
    if t == "tree" then return 12 end
    if t == "simple-entity" then return 12 end
    if t == "unit" or t == "unit-spawner" then return 11 end
    return ENTITY_TYPES_MAP[e.name] or 0
end

--- Serializa una entidad jugable en el formato compacto del protocolo.
local function serialize_entity(e)
    if not (e and e.valid) then return nil end
    if e.type == "resource" then return nil end

    local type_id = entity_type_of(e)
    if type_id == 0 then return nil end

    local orientation = 0.0
    if e.direction then
        orientation = direction_to_rad(e.direction)
    end

    local progress = 0.0
    local arm_angle = 0.0

    if type_id == 10 then
        -- Insertador: el brazo se extiende hacia la pila que sostiene.
        -- La posicion de la pila da el avance y el angulo del brazo.
        local held = e.held_stack_position
        if held then
            local dx = held.x - e.position.x
            local dy = held.y - e.position.y
            local len = math.sqrt(dx * dx + dy * dy)
            progress = math.min(1.0, len)
            arm_angle = math.atan2(dx, -dy)
        end
    else
        -- En Factorio 2.x leer `crafting_progress` en una entidad que no es
        -- ensambladora LANZA ERROR ("Entity is not crafting-machine"), no
        -- devuelve nil. Sin pcall, una sola piedra en el mapa abortaba la
        -- instantanea entera.
        local ok_cp, cp = pcall(function() return e.crafting_progress end)
        if ok_cp and cp then progress = cp end
    end

    if type_id == 9 then
        -- Cinta: la API Lua de Factorio 2.0 no expone la posicion de los
        -- items sobre la banda, asi que la fase de avance se sintetiza con
        -- el tick del juego. La direccion de flujo si viene del motor real.
        local phase = (game.tick + math.floor(e.position.x * 7 + e.position.y * 13)) % 60
        progress = phase / 60.0
    end

    -- Frame de animacion y direccion: el motor de VR necesita el frame
    -- exacto para dibujar el sprite real (y no una caja). `e.animation` solo
    -- existe en las entidades animadas, asi que va protegido.
    local frame = 0
    local ok_anim, anim = pcall(function() return e.animation end)
    if ok_anim and anim then
        local ok_f, f = pcall(function() return anim.frame end)
        if ok_f and f then frame = f end
    end

    local out = string.format(
        '{"t":"%s","x":%.2f,"y":%.2f,"o":%.3f,"s":%d,"p":%.3f,"h":%.3f,"f":%d,"d":%d',
        e.name, e.position.x, e.position.y, orientation, status_of(e), progress,
        health_of(e), frame, e.direction or 0)
    if type_id == 10 then
        out = out .. string.format(',"a":%.3f', arm_angle)
    end
    return out .. "}"
end

--- Acumula los recursos de una zona en parches compactos de 4x4 casillas.
local function collect_patches(found)
    local buckets = {}
    local order = {}
    for _, e in ipairs(found) do
        if e.valid and e.type == "resource" then
            local ore = ITEM_IDS_MAP[e.name]
            if ore and ore <= 4 then
                local bx = math.floor(e.position.x / RESOURCE_CELL)
                local by = math.floor(e.position.y / RESOURCE_CELL)
                local key = bx .. "," .. by .. "," .. ore
                local bucket = buckets[key]
                if not bucket then
                    bucket = { x = 0.0, y = 0.0, rem = 0.0, n = 0, ore = ore }
                    buckets[key] = bucket
                    table.insert(order, key)
                end
                bucket.x = bucket.x + e.position.x
                bucket.y = bucket.y + e.position.y
                bucket.rem = bucket.rem + (e.amount or 0)
                bucket.n = bucket.n + 1
            end
        end
    end

    local out = {}
    for _, key in ipairs(order) do
        local b = buckets[key]
        local cx = b.x / b.n
        local cy = b.y / b.n
        table.insert(out, string.format(
            '{"t":%d,"x":%.2f,"y":%.2f,"r":2.4,"rem":%d}',
            b.ore, cx, cy, math.floor(b.rem)))
    end
    return out
end

-- ---------------------------------------------------------------------------
-- Terreno: el suelo real del mapa (hierba, tierra, arena, agua, caminos).
--
-- El motor de VR dibuja el terreno con los tiles de verdad del juego, no con
-- un color plano. Para que el payload siga siendo compacto se manda el tile
-- DOMINANTE del area y solo los que difieren de ese.
-- ---------------------------------------------------------------------------
local function collect_terrain(surface, area)
    local x0, y0 = area[1][1], area[1][2]
    local x1, y1 = area[2][1], area[2][2]
    local w = x1 - x0 + 1
    local h = y1 - y0 + 1

    local names = {}
    local name_idx = {}
    local counts = {}
    local grid = {}

    for y = y0, y1 do
        local row = {}
        for x = x0, x1 do
            local nombre = nil
            local ok, tile = pcall(function() return surface.get_tile(x, y) end)
            if ok and tile and tile.name then
                nombre = tile.name
            end
            local i = name_idx[nombre]
            if not i then
                i = #names + 1
                name_idx[nombre] = i
                names[i] = nombre
                counts[i] = 0
            end
            counts[i] = counts[i] + 1
            row[x - x0 + 1] = i
        end
        grid[y - y0 + 1] = row
    end

    -- El tile mas frecuente es la base: el resto se manda como excepciones.
    local base, mejor = 1, -1
    for i, c in pairs(counts) do
        if c > mejor then
            mejor, base = c, i
        end
    end

    local excepciones = {}
    for y = 0, h - 1 do
        for x = 0, w - 1 do
            local i = grid[y + 1][x + 1]
            if i ~= base then
                excepciones[#excepciones + 1] =
                    string.format("[%d,%d,%d]", x, y, i)
            end
        end
    end

    return string.format(
        '{"base":%d,"names":[%s],"x0":%d,"y0":%d,"w":%d,"h":%d,"tiles":[%s]}',
        base, '"' .. table.concat(names, '","') .. '"',
        x0, y0, w, h, table.concat(excepciones, ","))
end

-- ---------------------------------------------------------------------------
-- Interfaz remota
-- ---------------------------------------------------------------------------

remote.add_interface("xfactorvr", {

    -- Instantanea completa del area visible alrededor de (cx, cy).
    get_world_snapshot = function(radius, cx, cy)
        radius = radius or 32
        cx = cx or 0
        cy = cy or 0

        local surface = game.surfaces[1]
        if not surface then
            local empty = '{"entities":[],"patches":[],"terrain":{}}'
            rcon.print(empty)
            return empty
        end

        local area = {
            { cx - radius, cy - radius },
            { cx + radius, cy + radius },
        }

        local ok, found = pcall(function() return surface.find_entities(area) end)
        if not ok or not found then
            local empty = '{"entities":[],"patches":[],"terrain":{}}'
            rcon.print(empty)
            return empty
        end

        local entities = {}
        local patches = collect_patches(found)

        for _, e in ipairs(found) do
            if #entities >= MAX_ENTITIES then break end
            local s = serialize_entity(e)
            if s then table.insert(entities, s) end
        end

        -- El terreno se recoge aparte: son miles de tiles y no deben
        -- romper el caso de que get_tile falle en alguna posicion.
        local terreno = "{}"
        local ok_t, t_err = pcall(function()
            return collect_terrain(surface, area)
        end)
        if ok_t and t_err then
            terreno = t_err
        end

        local out = '{"entities":[' .. table.concat(entities, ",") ..
            '],"patches":[' .. table.concat(patches, ",") ..
            '],"terrain":' .. terreno .. '}'
        rcon.print(out)
        return out
    end,

    -- Coloca una entidad. La orientacion llega en radianes.
    place_entity = function(type_name, x, y, orientation_rad)
        local surface = game.surfaces[1]
        if not surface or not type_name or type_name == "" then
            rcon.print("ERR|no_surface")
            return false
        end

        local dir = rad_to_direction(orientation_rad or 0)

        -- Sin tecnologia de automatizacion el jugador solo tiene la perforadora
        -- de combustible: se usa esa en su lugar.
        local alternatives = { type_name }
        if type_name == "electric-mining-drill" then
            table.insert(alternatives, "burner-mining-drill")
        elseif type_name == "burner-mining-drill" then
            table.insert(alternatives, "electric-mining-drill")
        end

        local chosen = type_name
        for _, alt in ipairs(alternatives) do
            if inv_get(alt) > 0 then
                chosen = alt
                break
            end
        end

        -- Si ya hay una maquina de ese tipo en el sitio, no se coloca otra:
        -- en el tutorial de la campana el "colocar el horno" se puede pedir
        -- mas de una vez, y con dos hornos el nivel se queda mirando el que
        -- no tiene el mineral (el primero que se encontro).
        for _, alt in ipairs(alternatives) do
            local ok_hay, hay = pcall(function()
                return surface.find_entities_filtered({
                    position = { x, y },
                    -- 0.8: la maquina se coloca en el centro de la casilla,
                    -- que puede estar a 0.71 de la posicion pedida.
                    radius = 0.8,
                    name = alt,
                })
            end)
            if ok_hay and hay and #hay > 0 then
                rcon.print("OK")
                return true
            end
        end

        local ok, created = pcall(function()
            return surface.create_entity({
                name = chosen,
                position = { x, y },
                direction = dir,
                force = "player",
                fast_replace = true,
                spill = false,
                raise_built = true,
            })
        end)

        if not ok or not created then
            rcon.print("ERR|create_failed")
            return false
        end

        -- Las ensambladoras nacen sin receta: se les asigna la primera de la
        -- lista para que la cadena logistica tenga algo que fabricar.
        if created.type == "assembling-machine" then
            pcall(function() created.set_recipe(RECIPE_ROTATION[1]) end)
        end

        inv_remove_any(alternatives, 1)
        actualizar_barra_rapida()
        rcon.print("OK")
        return true
    end,

    -- Mina un edificio y devuelve sus materiales al inventario.
    --
    -- Si en el sitio hay un item del suelo, se recoge ese (es lo que el
    -- jugador haria) y NO se toca el resto: al lado del item suele estar la
    -- perforadora, y demolerla rompe la campana.
    mine_entity = function(x, y)
        local surface = game.surfaces[1]
        if not surface then return false end

        local item = buscar_item_del_suelo(surface, x, y)
        if item then
            local nombre = item.name
            local pos = { x = item.position.x, y = item.position.y }
            if recoger_item_del_suelo(item) then
                anotar_minado(nombre, pos)
                actualizar_barra_rapida()
                rcon.print("OK")
                return true
            end
            rcon.print("ERR|no_pick")
            return false
        end

        local found = surface.find_entities_filtered({
            position = { x, y },
            radius = 1.0,
        })
        for _, e in ipairs(found) do
            if e.valid and e.type ~= "character" and e.type ~= "resource" then
                -- Se preferie minar de verdad: devuelve el material al inventario.
                local p = actor()
                local mined = false
                if p then
                    local ok = pcall(function() p.mine_entity(e, true) end)
                    mined = ok
                end
                if not mined then
                    local ok2 = pcall(function() e.destroy() end)
                    if not ok2 then return false end
                end
                actualizar_barra_rapida()
                rcon.print("OK")
                return true
            end
        end
        rcon.print("ERR|no_entity")
        return false
    end,

    -- Recoge un item del suelo (la entidad "item-on-ground").
    --
    -- Es la version "de jugador" de minar: el juego lanza on_player_mined_entity
    -- cuando un JUGADOR recoge un item del suelo. Un personaje virtual no lo
    -- lanza nunca, asi que el item se recoge a mano y se anota, para que el
    -- escenario parcheado le pase a la historia ese mismo evento.
    pick_up_item = function(x, y)
        local surface = game.surfaces[1]
        if not surface then
            rcon.print("ERR|no_surface")
            return false
        end

        local e = buscar_item_del_suelo(surface, x, y)
        if not e then
            rcon.print("ERR|no_item")
            return false
        end

        -- El nombre y la posicion se copian ANTES de recogerlo.
        local nombre = e.name
        local pos = { x = e.position.x, y = e.position.y }

        if not recoger_item_del_suelo(e) then
            rcon.print("ERR|no_pick")
            return false
        end

        -- El juego solo lanza on_player_mined_entity con un JUGADOR REAL: se
        -- anota para que el escenario parcheado se lo pase a la historia.
        anotar_minado(nombre, pos)
        actualizar_barra_rapida()
        rcon.print("OK")
        return true
    end,

    -- Minado manual con pico sobre una veta. Devuelve el id del mineral.
    manual_mine = function(x, y)
        local surface = game.surfaces[1]
        if not surface then
            rcon.print("0")
            return 0
        end

        local found = surface.find_entities_filtered({
            position = { x, y },
            radius = 1.5,
            type = "resource",
        })
        for _, e in ipairs(found) do
            if e.valid then
                local ore = ITEM_IDS_MAP[e.name]
                if ore and ore <= 4 then
                    local amount = e.amount or 0
                    if amount > 0 then
                        local ok = pcall(function() e.amount = amount - 1 end)
                        if ok then
                            inv_add(e.name, 1)
                            actualizar_barra_rapida()
                            rcon.print(tostring(ore))
                            return ore
                        end
                    end
                end
            end
        end
        rcon.print("0")
        return 0
    end,

    -- Interactua con una maquina: recoge lo producido, aborta combustible y
    -- rota la receta si no habia nada que recoger.
    interact_machine = function(x, y)
        local surface = game.surfaces[1]
        if not surface then
            rcon.print("0")
            return 0
        end

        local found = surface.find_entities_filtered({
            position = { x, y },
            radius = 1.0,
        })

        -- La entidad mas cercana al punto (sin contar al propio actor).
        local e = entidad_mas_cercana(found, x, y, actor_unit_number(actor()))
        if e then
            local looted = 0

            -- 1. Recoger todo lo producido.
            local ok_out, out_inv = pcall(function() return e.get_output_inventory() end)
            if ok_out and out_inv then
                for _, stack in ipairs(inventory_contents(out_inv)) do
                    if stack.count and stack.count > 0 then
                        local taken = inv_add(stack.name, stack.count, stack.quality)
                        looted = looted + taken
                        if taken > 0 then
                            pcall(function()
                                out_inv.remove({ name = stack.name, count = taken })
                            end)
                        end
                    end
                end
            end

            -- 2. Abastecer de combustible (solo hornos de combustible).
            -- En VR el inventario no es interactivo: sin esta ayuda el nodo
            -- "put-coal" del tutorial no se resuelve nunca.
            abastecer_combustible(e)

            -- 3. Sin nada que recoger: la ensambladora rota de receta.
            if looted == 0 and e.type == "assembling-machine" then
                -- En 2.0 la receta actual se lee con `get_recipe()`.
                local current = nil
                pcall(function()
                    local r = e.get_recipe()
                    if r then current = r.name end
                end)
                local next_recipe = RECIPE_ROTATION[1]
                for i, r in ipairs(RECIPE_ROTATION) do
                    if r == current then
                        next_recipe = RECIPE_ROTATION[(i % #RECIPE_ROTATION) + 1]
                        break
                    end
                end
                pcall(function() e.set_recipe(next_recipe) end)
            end

            rcon.print(tostring(looted))
            return looted
        end

        rcon.print("0")
        return 0
    end,

    -- Mueve al JUGADOR (real o virtual) a una posicion.
    --
    -- En la campana el escenario exige un jugador (usa game.players[1]) y las
    -- ramas finales piden acciones reales: abrir el pecio, coger piezas. Desde
    -- VR el jugador se mueve con el control izquierdo, asi que el mod hace
    -- dePiernas: teleport() coloca al jugador donde toca.
    --
    -- Si no hay nadie conectado (servidor headless), mueve al PERSONAJE
    -- VIRTUAL, que es lo que permite jugar la campana sin cliente grafico.
    teleport_player = function(x, y)
        local p = actor()
        if not p then
            rcon.print("ERR|no_player")
            return false
        end
        local ok, err = pcall(function()
            p.teleport({ x = x, y = y })
        end)
        if not ok then
            rcon.print("ERR|teleport")
            return false
        end
        rcon.print("OK")
        return true
    end,

    -- Interactua con una entidad: abre su GUI (abrir el pecio), le coje
    -- piezas, la activa. Es lo que usa el jugador con el raton.
    --
    -- Si no hay jugador real, interactua con el PERSONAJE VIRTUAL.
    interact_entity = function(x, y)
        local surface = game.surfaces[1]
        local p = actor()
        if not surface or not p then
            rcon.print("ERR|no_player")
            return false
        end

        local found = surface.find_entities_filtered({
            position = { x, y },
            radius = 1.5,
        })
        if #found == 0 then
            rcon.print("ERR|no_entity")
            return false
        end

        -- La entidad mas cercana al punto (sin contar al propio actor).
        local mejor = entidad_mas_cercana(found, x, y, actor_unit_number(p))
        if not mejor then
            rcon.print("ERR|no_entity")
            return false
        end

        -- Seleccionar la entidad: el nivel de campana mira
        -- `jugador().selected` para saber que ha señalado el jugador.
        pcall(function() p.selected = mejor end)

        -- `opened` es lo que el juego usa para "tener abierto" el GUI de una
        -- entidad. Un personaje sin controlador de GUI no puede abrir nada,
        -- pero eso no es un error: se sigue con el inventario.
        pcall(function()
            p.opened = mejor
        end)

        -- Si tiene inventario con contenido, se le coge al jugador.
        local cogido = 0
        local ok_out, out_inv = pcall(function() return mejor.get_output_inventory() end)
        if ok_out and out_inv then
            -- Maquinas: lo que ha producido.
            cogido = loot_inventory(out_inv)
        else
            -- Cajas y pecios (el del tutorial) no tienen inventario de salida:
            -- su contenido esta en el inventario principal.
            local ok_main, main_inv = pcall(function() return mejor.get_inventory(1) end)
            if ok_main and main_inv then
                cogido = loot_inventory(main_inv)
            end
        end

        -- Abastecer de combustible: en VR el inventario no es interactivo, asi
        -- que al interactuar con el horno se le mete el carbon que lleva el
        -- actor (nodo "put-coal" del tutorial).
        abastecer_combustible(mejor)

        actualizar_barra_rapida()
        rcon.print("OK|" .. tostring(cogido))
        return true
    end,

    -- Inventario completo del jugador en el formato compacto del protocolo.
    get_inventory = function()
        local items = {}
        local seen = {}

        local inv = actor_inventory()
        if inv then
            for _, stack in ipairs(inventory_contents(inv)) do
                if stack.count and stack.count > 0 then
                    local id = ITEM_IDS_MAP[stack.name]
                    if id and id > 0 and not seen[stack.name] then
                        seen[stack.name] = true
                        table.insert(items, string.format('{"id":%d,"count":%d}', id, stack.count))
                    end
                end
            end
        else
            for name, amount in pairs(virtual_inventory()) do
                if type(amount) == "number" and amount > 0 then
                    local id = ITEM_IDS_MAP[name]
                    if id and id > 0 and not seen[name] then
                        seen[name] = true
                        table.insert(items, string.format('{"id":%d,"count":%d}', id, amount))
                    end
                end
            end
        end

        local out = "[" .. table.concat(items, ",") .. "]"
        rcon.print(out)
        return out
    end,

    -- Fabrica una receta usando las recetas reales del juego.
    craft = function(recipe_name, count)
        count = count or 1
        -- En Factorio 2.0 `game.recipe_prototypes` ya no existe: las recetas
        -- viven en `force.recipes`, indexadas por nombre.
        local f = active_force()
        local rp = f and f.recipes and f.recipes[recipe_name] or nil
        if not rp then
            rcon.print("ERR|unknown_recipe")
            return false
        end

        -- Comprobacion previa de ingredientes.
        for _, ing in pairs(rp.ingredients) do
            local need = ing.amount * count
            if inv_get(ing.name) < need then
                rcon.print("ERR|missing_" .. ing.name)
                return false
            end
        end

        -- Si el actor tiene cola de fabricacion se encola la receta: asi el
        -- juego la fabrica de verdad y el nivel de campana ve
        -- "crafting_queue_size > 0". Solo se acepta si la cola ha crecido:
        -- un personaje sin controlador no puede fabricar en cola.
        local a = actor()
        if a then
            -- El jugador virtual "abre" la GUI de fabricacion y deja el item
            -- en la cola: el nivel de campana lo espera asi.
            local gui = virtual_gui()
            gui.type = GUI_CONTROLLER
            gui.crafts = count
            gui.caduca = game.tick + 120
        end

        -- Si el actor es un JUGADOR DE VERDAD (tiene controlador), se encola
        -- la receta: el juego la fabrica sola y el nivel ve la cola.
        --
        -- OJO: con un personaje virtual (sin controlador) NO se puede encolar:
        -- `begin_crafting` se come los ingredientes y la cola no crece, asi
        -- que los items se perdian. Con un personaje la fabricacion es
        -- instantanea (la de abajo).
        if a and real_player() then
            local before = 0
            pcall(function() before = tonumber(a.crafting_queue_size) or 0 end)
            local ok, began = pcall(function()
                return a.begin_crafting(count, recipe_name)
            end)
            if not (ok and began) then
                -- Otra firma: {count=, recipe=}.
                pcall(function()
                    return a.begin_crafting{ count = count, recipe = recipe_name }
                end)
            end
            local after = 0
            pcall(function() after = tonumber(a.crafting_queue_size) or 0 end)
            if after > before then
                actualizar_barra_rapida()
                rcon.print("OK")
                return true
            end
        end

        for _, ing in pairs(rp.ingredients) do
            inv_remove(ing.name, ing.amount * count)
        end

        -- En Factorio 2.x `force.recipes[nombre]` devuelve un LuaRecipe (el
        -- objeto de recipes de la fuerza), NO un LuaRecipePrototype: LuaRecipe
        -- no tiene `main_product`, solo `products` (array de Product). El
        -- prototipo sigue disponible en `rp.prototype`, pero `products` es
        -- valido en ambos casos.
        local product = nil
        local best = -1
        for _, p in pairs(rp.products or {}) do
            if p and p.name and p.type ~= "fluid" then
                local amount = p.amount or 1
                if amount > best then
                    best = amount
                    product = p
                end
            end
        end
        if product and product.name then
            inv_add(product.name, (product.amount or 1) * count)
        end

        actualizar_barra_rapida()
        rcon.print("OK")
        return true
    end,

    -- Fija la receta de una ensambladora concreta.
    set_recipe = function(x, y, recipe_name)
        local surface = game.surfaces[1]
        if not surface then
            rcon.print("ERR|no_surface")
            return false
        end

        local found = surface.find_entities_filtered({
            position = { x, y },
            radius = 1.0,
            type = "assembling-machine",
        })
        for _, e in ipairs(found) do
            if e.valid then
                local ok = pcall(function() e.set_recipe(recipe_name) end)
                if ok then
                    rcon.print("OK")
                    return true
                end
            end
        end

        rcon.print("ERR|no_machine")
        return false
    end,

    -- ---------------------------------------------------------------
    -- Jugador virtual
    --
    -- El escenario de campana necesita saber quien es "el jugador" cuando
    -- no hay nadie conectado. Este remoto se lo dice: devuelve los datos
    -- del personaje virtual (o del jugador real, si lo hay).
    --
    -- OJO: por el remoto NO se puede devolver una entidad (los objetos Lua
    -- no cruzan la frontera del remoto), asi que se devuelve su
    -- unit_number y el escenario lo localiza con
    -- game.get_entity_by_unit_number (el prototipo "character" lleva el flag
    -- "get-by-unit-number").
    -- ---------------------------------------------------------------
    get_virtual_player = function()
        local p = real_player()
        if p then
            local pos = p.position or { x = 0, y = 0 }
            local out = {
                found = true,
                real = true,
                name = p.name or "VR",
                tag = p.tag or VIRTUAL_TAG,
                index = p.index,
                x = pos.x,
                y = pos.y,
                unit_number = nil,
                surface_index = p.surface and p.surface.index or 1,
                crafting_queue_size = p.crafting_queue_size or 0,
            }
            rcon.print("VR|real|" .. tostring(out.name))
            return out
        end

        local c = ensure_virtual_character()
        if not c then
            local out = { found = false, real = false, name = VIRTUAL_NAME, tag = VIRTUAL_TAG }
            rcon.print("VR|none")
            return out
        end

        local pos = c.position or { x = 0, y = 0 }
        local gui = virtual_gui()
        local out = {
            found = true,
            real = false,
            name = VIRTUAL_NAME,
            tag = VIRTUAL_TAG,
            unit_number = c.unit_number,
            surface_index = c.surface and c.surface.index or 1,
            x = pos.x,
            y = pos.y,
            -- La GUI virtual: el personaje no tiene GUI, pero el nivel de
            -- campana la mira.
            gui_type = gui.type or GUI_NONE,
            crafting_queue_size = math.max(tonumber(c.crafting_queue_size) or 0,
                                           tonumber(gui.crafts) or 0),
            -- La barra rapida virtual (el personaje no tiene ninguna).
            quick_bar = state().xfactorvr_bar or {},
        }
        rcon.print("VR|virtual|" .. tostring(out.name))
        return out
    end,

    -- Asigna un item a un hueco de la barra rapida del jugador virtual.
    -- Un personaje no tiene barra rapida (es cosa del jugador con raton),
    -- pero el nivel de campana mira si el jugador tiene la perforadora
    -- asignada a un hueco antes de dejarle colocarla.
    --
    -- OJO: una ranura puesta aqui es "del jugador": el mod automatico no la
    -- pisa nunca (ver actualizar_barra_rapida).
    set_quick_bar = function(page, slot, item_name)
        page = page or 1
        slot = slot or 1
        local bar, manual = barra_rapida()
        local clave = tostring(page) .. "_" .. tostring(slot)
        if item_name and item_name ~= "" then
            bar[clave] = item_name
            manual[clave] = true
        else
            bar[clave] = nil
            manual[clave] = nil
        end
        rcon.print("OK")
        return true
    end,

    -- Contenido de la barra rapida virtual: "1_1=iron-ore;1_2=coal".
    -- El "!" detrás del item marca una ranura puesta a mano.
    -- Lo usa el banco de pruebas y el visor.
    get_quick_bar = function()
        local bar, manual = barra_rapida()
        local partes = {}
        for clave, nombre in pairs(bar) do
            partes[#partes + 1] = clave .. "=" .. nombre ..
                (manual[clave] and "!" or "")
        end
        table.sort(partes)
        local out = table.concat(partes, ";")
        rcon.print(out)
        return out
    end,

    -- Dice si el actor ha recogido un item del suelo desde la ultima vez que
    -- se pregunto (y lo consume).
    --
    -- El juego solo lanza on_player_mined_entity cuando hay un JUGADOR REAL
    -- (con su controlador); un personaje virtual no lo lanza nunca. El
    -- escenario parcheado usa este remoto para darle a la historia el mismo
    -- evento que habria lanzado el juego (nodo "show-miner-blockage").
    get_mined_item = function()
        local st = state()
        local info = st.xfactorvr_minado
        st.xfactorvr_minado = nil
        if not info then return nil end
        rcon.print("MINED|" .. tostring(info.name))
        return info
    end,

    -- Mete items en el inventario de una maquina (al horno, al quemador).
    -- El indice de inventario importa: en un horno el 1 es el combustible y
    -- el 2 los ingredientes.
    insert_item = function(x, y, item_name, count, inv_index)
        count = count or 1
        inv_index = inv_index or 1
        local surface = game.surfaces[1]
        if not surface or not item_name then
            rcon.print("ERR|no_surface")
            return false
        end

        local found = surface.find_entities_filtered({
            position = { x, y },
            radius = 1.5,
        })
        -- Sin contar al propio actor: si el jugador virtual esta junto a la
        -- maquina, el item se metia en su inventario y no en el de la maquina.
        local e = entidad_mas_cercana(found, x, y, actor_unit_number(actor()))
        if e then
            local ok_inv, inv = pcall(function() return e.get_inventory(inv_index) end)
            if ok_inv and inv then
                local ok, inserted = pcall(function()
                    return inv.insert({ name = item_name, count = count })
                end)
                if ok and inserted and inserted > 0 then
                    actualizar_barra_rapida()
                    rcon.print("OK|" .. tostring(inserted))
                    return true
                end
            end
        end

        rcon.print("ERR|no_machine")
        return false
    end
})

script.on_init(function()
    virtual_inventory()
    -- En un servidor headless no hay jugador: se intenta crear el personaje
    -- virtual para que el nivel de campana pueda arrancar (ver docs/ADR-007).
    -- Puede que todavia no haya superficie (se crea mas tarde, cuando el
    -- escenario lo pide por el remoto).
    ensure_virtual_character()
    -- La version real del motor se lee del mod "core"; no hay una API directa
    -- y "2.0" a pelo mentia en cuanto el visor corre Factorio 2.1.
    local version = "?"
    local ok, core = pcall(function() return mods["core"] end)
    if ok and core and core.version then version = tostring(core.version) end
    log("[XFactorVR] Bridge mod initialized. Factorio " .. version ..
        " con " .. tostring(EAST_STEP) .. " como paso de direccion este.")
end)

-- El personaje virtual se recrea si muere (por ejemplo por un biter).
-- Se registra con cuidado porque el banco unitario de pruebas no expone
-- on_event.
local event_tick = defines.events and defines.events.on_tick
if event_tick and script.on_event then
    script.on_event(event_tick, function(event)
        local st = state()

        -- La GUI virtual caduca: la cola de fabricacion se vacia sola.
        local gui = st.xfactorvr_gui
        if gui and (gui.crafts or 0) > 0 and event.tick >= (gui.caduca or 0) then
            gui.crafts = 0
        end

        st.xfactorvr_tick = (st.xfactorvr_tick or 0) + 1

        -- La barra rapida virtual se refresca dos veces por segundo: puede
        -- que el actor coja items sin pasar por un remoto del mod (por
        -- ejemplo si el juego le da algo solo).
        if (st.xfactorvr_tick % 30) == 0 then
            actualizar_barra_rapida()
        end

        -- Cada 5 segundos se comprueba que el personaje sigue en pie.
        if (st.xfactorvr_tick % 300) ~= 0 then return end
        if real_player() then return end
        if virtual_character() then return end
        local c = ensure_virtual_character()
        if c then
            log("[XFactorVR] Jugador virtual recreado.")
        end
    end)
end
