-- scripts/tests/bridge_control_test.lua
--
-- Banco de pruebas del mod puente: simula la API de Factorio 2.0 y verifica el
-- comportamiento de mods/xfactorvr_bridge/control.lua sin necesidad de un
-- servidor real. Uso:  lua scripts/tests/bridge_control_test.lua

package.path = "mods/xfactorvr_bridge/?.lua;" .. package.path

-- ---------------------------------------------------------------------------
-- Dobles de prueba de la API de Factorio 2.0
-- ---------------------------------------------------------------------------

defines = {
    direction = { north = 0, northeast = 1, east = 4, southeast = 6, south = 8, southwest = 10, west = 12, northwest = 14 },
    entity_status = {
        working = 1,
        low_power = 2,
        no_fuel = 3,
        no_ingredients = 4,
        disabled_by_control_behavior = 5,
    },
}

local printed = {}
rcon = {
    print = function(s) table.insert(printed, s) end,
    print_error = function(s) table.insert(printed, "ERR:" .. s) end,
}

-- Tabla global de Factorio (persiste entre guardados).
-- Factorio 2.0 elimino la tabla `global` en favor de `storage`.
-- El mock replica la API real: si el mod vuelve a usar `global`, este banco falla.
storage = {}

local interface = nil
remote = {
    add_interface = function(name, tbl)
        assert(name == "xfactorvr", "nombre de interfaz inesperado: " .. tostring(name))
        interface = tbl
    end,
}

local init_callback = nil
script = {
    on_init = function(fn) init_callback = fn end,
    on_load = function() end,
}

-- log() de Factorio
log = function() end

-- --- Entidades simuladas ---------------------------------------------------

local function make_entity(spec)
    local e = {
        name = spec.name,
        type = spec.type,
        position = { x = spec.x, y = spec.y },
        direction = spec.direction,
        status = spec.status,
        health = spec.health or 100,
        -- En Factorio 2.0 el maximo de vida es `max_health` (no `health_max`).
        max_health = spec.max_health or 100,
        amount = spec.amount,
        crafting_progress = spec.crafting_progress,
        held_stack_position = spec.held_stack_position,
        held_stack = spec.held_stack,
        current_recipe = spec.recipe,
        valid = true,
    }
    e.get_health_ratio = function()
        if not e.max_health or e.max_health <= 0 then return 1.0 end
        return e.health / e.max_health
    end
    e.get_output_inventory = function()
        if spec.type ~= "assembling-machine" and spec.type ~= "furnace" then
            error("no output inventory")
        end
        local inv = spec.output_inventory or {}
        return {
            get_contents = function() return spec.output_contents or inv end,
            clear = function() spec.output_contents = {} end,
        }
    end
    e.get_fuel_inventory = function()
        if spec.type ~= "furnace" then error("no fuel inventory") end
        return {
            get_item_count = function(name) return (spec.fuel_contents or {})[name] or 0 end,
            insert = function(item) spec.fuel_contents = spec.fuel_contents or {}; spec.fuel_contents[item.name] = (spec.fuel_contents[item.name] or 0) + item.count end,
        }
    end
    -- En Factorio los metodos de LuaEntity se invocan con notacion de punto:
    -- e.set_recipe("iron-gear-wheel").
    -- En 2.0 la receta actual se lee con get_recipe(), que devuelve un
    -- prototipo con campo `name`.
    e.set_recipe = function(r) e.current_recipe = r end
    e.get_recipe = function()
        if not e.current_recipe then return nil end
        return { name = e.current_recipe }
    end
    e.destroy = function() e.valid = false end
    return e
end

local entities = {}

local surface = {
    find_entities = function(area)
        local out = {}
        for _, e in ipairs(entities) do
            if e.valid then table.insert(out, e) end
        end
        return out
    end,
    find_entities_filtered = function(filter)
        local out = {}
        -- Factorio admite Position como {x=,y=} o como {x, y}; el mod usa la 2a forma.
        local px, py
        if filter.position.x ~= nil then
            px, py = filter.position.x, filter.position.y
        else
            px, py = filter.position[1], filter.position[2]
        end
        local radius = filter.radius or 0
        for _, e in ipairs(entities) do
            if e.valid then
                local dx = e.position.x - px
                local dy = e.position.y - py
                local in_range = (dx * dx + dy * dy) <= radius * radius
                local type_ok = (not filter.type) or e.type == filter.type
                if in_range and type_ok then table.insert(out, e) end
            end
        end
        return out
    end,
    create_entity = function(opts)
        if opts.name == "unknown-entity" then return nil end
        local e = make_entity({
            name = opts.name,
            type = opts.name:find("machine") and "assembling-machine" or "entity",
            x = opts.position[1],
            y = opts.position[2],
            direction = opts.direction,
        })
        e.created_with = opts
        table.insert(entities, e)
        return e
    end,
}

-- En Factorio 2.0 las recetas NO cuelgan de `game.recipe_prototypes`: viven en
-- `force.recipes`, indexadas por nombre. El mock replica esa forma real.
-- IMPORTANTE: un LuaRecipe NO tiene `main_product` (eso pertenece al
-- LuaRecipePrototype). El motor real falla con "LuaRecipe doesn't contain key
-- main_product" si el mod lo usa, asi que el mock tampoco lo expone: si el mod
-- vuelve a depender de `main_product`, este banco lo detecta.
local RECIPES = {
    ["iron-gear-wheel"] = {
        name = "iron-gear-wheel",
        ingredients = { { type = "item", name = "iron-plate", amount = 2 } },
        products = { { type = "item", name = "iron-gear-wheel", amount = 1 } },
    },
    ["transport-belt"] = {
        name = "transport-belt",
        ingredients = { { type = "item", name = "iron-plate", amount = 1 },
                        { type = "item", name = "iron-gear-wheel", amount = 1 } },
        products = { { type = "item", name = "transport-belt", amount = 2 } },
    },
    ["inserter"] = {
        name = "inserter",
        ingredients = { { type = "item", name = "iron-plate", amount = 1 },
                        { type = "item", name = "iron-gear-wheel", amount = 1 },
                        { type = "item", name = "electronic-circuit", amount = 1 } },
        products = { { type = "item", name = "inserter", amount = 1 } },
    },
}

game = {
    surfaces = { surface },
    players = {}, -- servidor headless: sin jugador conectado
    forces = {
        {
            name = "player",
            recipes = RECIPES,
        },
    },
    tick = 1,
}

-- ---------------------------------------------------------------------------
-- Carga del modulo bajo prueba
-- ---------------------------------------------------------------------------

dofile("mods/xfactorvr_bridge/control.lua")
assert(init_callback, "script.on_init no fue registrado")
init_callback()
assert(interface, "remote.add_interface no fue llamado")

-- ---------------------------------------------------------------------------
-- Mini framework
-- ---------------------------------------------------------------------------

local passed, failed = 0, 0
local function check(name, cond, detail)
    if cond then
        passed = passed + 1
        print("  ok   - " .. name)
    else
        failed = failed + 1
        print("  FAIL - " .. name .. (detail and ("  [" .. tostring(detail) .. "]") or ""))
    end
end

local function last_print()
    return printed[#printed]
end

-- ---------------------------------------------------------------------------
-- Pruebas
-- ---------------------------------------------------------------------------

print("\n== Direcciones (16 pasos, este == 4) ==")

-- El snapshot serializa la orientacion como direccion * TAU/16.
entities = {}
table.insert(entities, make_entity({ name = "transport-belt", type = "entity", x = 1, y = 2, direction = 0 }))
printed = {}
interface.get_world_snapshot(32, 0, 0)
local json = last_print()
check("norte (dir 0) -> o=0.000", json:find('"o":0%.000') ~= nil, json)
check("posicion x/y serializada", json:find('"x":1%.00,"y":2%.00') ~= nil, json)

entities = {}
table.insert(entities, make_entity({ name = "transport-belt", type = "entity", x = 1, y = 2, direction = 4 }))
printed = {}
interface.get_world_snapshot(32, 0, 0)
json = last_print()
-- este = 4 -> 4 * TAU/16 = TAU/4 = 1.5708 rad
check("este (dir 4) -> o=1.571", json:find('"o":1%.571') ~= nil, json)

entities = {}
table.insert(entities, make_entity({ name = "transport-belt", type = "entity", x = 1, y = 2, direction = 8 }))
printed = {}
interface.get_world_snapshot(32, 0, 0)
json = last_print()
check("sur (dir 8) -> o=3.142", json:find('"o":3%.142') ~= nil, json)

entities = {}
table.insert(entities, make_entity({ name = "transport-belt", type = "entity", x = 1, y = 2, direction = 12 }))
printed = {}
interface.get_world_snapshot(32, 0, 0)
json = last_print()
check("oeste (dir 12) -> o=4.712", json:find('"o":4%.712') ~= nil, json)

print("\n== Tipos de entidad ==")
entities = {}
table.insert(entities, make_entity({ name = "assembling-machine-1", type = "assembling-machine", x = 0, y = 0, direction = 4, status = defines.entity_status.working }))
table.insert(entities, make_entity({ name = "gun-turret", type = "entity", x = 5, y = 0, direction = 0 }))
table.insert(entities, make_entity({ name = "radar", type = "entity", x = 6, y = 0, direction = 0 }))
table.insert(entities, make_entity({ name = "roboport", type = "entity", x = 7, y = 0, direction = 0 }))
table.insert(entities, make_entity({ name = "inserter", type = "entity", x = 8, y = 0, direction = 4 }))
table.insert(entities, make_entity({ name = "tree", type = "tree", x = 9, y = 0 }))
table.insert(entities, make_entity({ name = "rock", type = "simple-entity", x = 10, y = 0 }))
table.insert(entities, make_entity({ name = "small-biter", type = "unit", x = 11, y = 0 }))
printed = {}
interface.get_world_snapshot(32, 0, 0)
json = last_print()
check("ensambladora -> t=1", json:find('"t":1') ~= nil, json)
check("torreta -> t=4", json:find('"t":4') ~= nil, json)
check("radar -> t=5", json:find('"t":5') ~= nil, json)
check("robopuerto -> t=6", json:find('"t":6') ~= nil, json)
check("insertador -> t=10", json:find('"t":10') ~= nil, json)
check("arbol -> t=12", json:find('"t":12') ~= nil, json)
check("roca -> t=12", json:find('"t":12') ~= nil, json)
check("bitero -> t=11", json:find('"t":11') ~= nil, json)

print("\n== Yacimientos agrupados en parches de 4x4 ==")
entities = {}
-- Tres vetas de hierro dentro de la misma celda 4x4 (0..4, 0..4)
table.insert(entities, make_entity({ name = "iron-ore", type = "resource", x = 1.0, y = 1.0, amount = 500 }))
table.insert(entities, make_entity({ name = "iron-ore", type = "resource", x = 2.0, y = 3.0, amount = 300 }))
table.insert(entities, make_entity({ name = "iron-ore", type = "resource", x = 0.5, y = 0.5, amount = 200 }))
-- Veta de cobre en otra celda
table.insert(entities, make_entity({ name = "copper-ore", type = "resource", x = 10.0, y = 10.0, amount = 400 }))
printed = {}
interface.get_world_snapshot(32, 0, 0)
json = last_print()
check("parche de hierro agrupado (t=1)", json:find('"t":1,"x":1%.17,"y":1%.50,"r":2%.4,"rem":1000') ~= nil, json)
check("parche de cobre separado (t=2)", json:find('"t":2,"x":10%.00,"y":10%.00') ~= nil, json)
check("dos parches en total", select(2, json:gsub('"t":%d,"x":%d+%.%d+,"y":%d+%.%d+,"r"', '')) == 2, json)

print("\n== Colocacion de entidades con orientacion ==")
entities = {}
storage.xfactorvr_inventory["transport-belt"] = 5
printed = {}
local ok = interface.place_entity("transport-belt", 3.0, 4.0, math.pi * 0.5) -- este
check("place_entity devuelve true", ok == true)
check("respuesta OK", last_print() == "OK", last_print())
local created = entities[1]
check("entidad creada", created ~= nil)
check("direccion este (4)", created.created_with.direction == 4, created.created_with.direction)
check("posicion correcta", created.created_with.position[1] == 3.0 and created.created_with.position[2] == 4.0)
check("fuerza player", created.created_with.force == "player")
check("raise_built activo", created.created_with.raise_built == true)
check("consumo 1 del inventario", storage.xfactorvr_inventory["transport-belt"] == 4,
    storage.xfactorvr_inventory["transport-belt"])

-- Rotacion: 180 grados -> sur (8)
entities = {}
storage.xfactorvr_inventory["transport-belt"] = 5
interface.place_entity("transport-belt", 0.0, 0.0, math.pi)
check("rotacion 180 -> sur (8)", entities[1].created_with.direction == 8, entities[1].created_with.direction)

-- Sin item en inventario: cae a la alternativa (quemador)
entities = {}
storage.xfactorvr_inventory["electric-mining-drill"] = nil
storage.xfactorvr_inventory["burner-mining-drill"] = 1
interface.place_entity("electric-mining-drill", 1.0, 1.0, 0)
check("perforadora electrica cae a quemador", entities[1].created_with.name == "burner-mining-drill",
    entities[1].created_with.name)

print("\n== Minado manual ==")
entities = {}
table.insert(entities, make_entity({ name = "iron-ore", type = "resource", x = 2.0, y = 2.0, amount = 10 }))
printed = {}
local ore = interface.manual_mine(2.0, 2.0)
check("minado devuelve id de mineral", ore == 1, ore)
check("veta reducida", entities[1].amount == 9, entities[1].amount)
check("mineral al inventario", storage.xfactorvr_inventory["iron-ore"] == 1,
    storage.xfactorvr_inventory["iron-ore"])
check("respuesta numerica", last_print() == "1", last_print())

print("\n== Interaccion con maquinas ==")
entities = {}
table.insert(entities, make_entity({
    name = "assembling-machine-1", type = "assembling-machine", x = 0, y = 0,
    recipe = "iron-gear-wheel",
    output_contents = { ["iron-gear-wheel"] = 3 },
}))
printed = {}
local looted = interface.interact_machine(0, 0)
check("recoge 3 engranajes", looted == 3, looted)
check("engranajes al inventario", storage.xfactorvr_inventory["iron-gear-wheel"] == 3)

-- Sin nada que recoger -> rota la receta
entities = {}
table.insert(entities, make_entity({
    name = "assembling-machine-1", type = "assembling-machine", x = 0, y = 0,
    recipe = "iron-gear-wheel", output_contents = {},
}))
interface.interact_machine(0, 0)
check("rota a la siguiente receta", entities[1].current_recipe == "electronic-circuit", entities[1].current_recipe)

print("\n== Fabricacion con recetas reales ==")
storage.xfactorvr_inventory["iron-plate"] = 10
storage.xfactorvr_inventory["iron-gear-wheel"] = 0
storage.xfactorvr_inventory["electronic-circuit"] = 0
printed = {}
ok = interface.craft("iron-gear-wheel", 1)
check("craft devuelve true", ok == true)
check("consume 2 planchas", storage.xfactorvr_inventory["iron-plate"] == 8, storage.xfactorvr_inventory["iron-plate"])
check("produce 1 engranaje", storage.xfactorvr_inventory["iron-gear-wheel"] == 1)

-- Falta un ingrediente -> no fabrica y no consume
storage.xfactorvr_inventory["iron-plate"] = 0
storage.xfactorvr_inventory["iron-gear-wheel"] = 1
ok = interface.craft("iron-gear-wheel", 1)
check("craft sin ingredientes falla", ok == false)
check("no altera el inventario", storage.xfactorvr_inventory["iron-gear-wheel"] == 1)

print("\n== Inventario ==")
storage.xfactorvr_inventory = { ["iron-plate"] = 5, ["coal"] = 2, ["desconocido"] = 9 }
printed = {}
local inv_json = interface.get_inventory()
check("devuelve JSON de array", inv_json:sub(1, 1) == "[", inv_json)
check("planchas de hierro id=5", inv_json:find('{"id":5,"count":5}') ~= nil, inv_json)
check("carbon id=3", inv_json:find('{"id":3,"count":2}') ~= nil, inv_json)
check("items desconocidos omitidos", inv_json:find("desconocido") == nil, inv_json)

-- ---------------------------------------------------------------------------
-- Inventario de un jugador real conectado.
--
-- Este camino es el que estaba roto: el mod llamaba a `inv.remove_item`, que
-- no existe en Factorio 2.0. El pcall lo silenciaba y los ingredientes nunca
-- se consumian, con lo que se podian fabricar items sin limite.
-- ---------------------------------------------------------------------------

print("\n== Fabricacion con jugador conectado ==")
local player_items = { ["iron-plate"] = 5, ["iron-gear-wheel"] = 0 }
local mock_player = {
    valid = true,
    force = game.forces[1],
    get_main_inventory = function()
        return {
            get_item_count = function(name) return player_items[name] or 0 end,
            get_contents = function() return player_items end,
            insert = function(item)
                player_items[item.name] = (player_items[item.name] or 0) + item.count
                return item.count
            end,
            -- Factorio 2.0: remove(nombre, cantidad). Devuelve lo quitado.
            remove = function(name, count)
                local have = player_items[name] or 0
                local take = math.min(have, count)
                player_items[name] = have - take
                return take
            end,
        }
    end,
    mine_entity = function(e, _force) e.destroy() end,
}
game.players = { mock_player }

ok = interface.craft("iron-gear-wheel", 1)
check("craft con jugador devuelve true", ok == true)
check("consume las planchas del jugador", player_items["iron-plate"] == 3, player_items["iron-plate"])
check("entrega el engranaje al jugador", player_items["iron-gear-wheel"] == 1, player_items["iron-gear-wheel"])

-- Sin ingredientes suficientes no se fabrica ni se consume nada.
player_items["iron-plate"] = 0
ok = interface.craft("iron-gear-wheel", 1)
check("craft sin planchas falla", ok == false)
check("no toca el inventario del jugador", player_items["iron-gear-wheel"] == 1, player_items["iron-gear-wheel"])

-- Volvemos al modo servidor headless (sin jugador) para el resto de pruebas.
game.players = {}

print("\n== set_recipe ==")
entities = {}
table.insert(entities, make_entity({ name = "assembling-machine-1", type = "assembling-machine", x = 0, y = 0 }))
ok = interface.set_recipe(0, 0, "transport-belt")
check("set_recipe devuelve true", ok == true)
check("receta aplicada", entities[1].current_recipe == "transport-belt", entities[1].current_recipe)

-- ---------------------------------------------------------------------------

print(string.format("\n=== %d correctas, %d fallidas ===", passed, failed))
os.exit(failed == 0 and 0 or 1)
