-- xfactorvr_bridge control.lua
-- Bridges Factorio simulation state with XFactorVR OpenXR Tabletop engine

local ENTITY_TYPES_MAP = {
    ["assembling-machine-1"] = 1,
    ["assembling-machine-2"] = 1,
    ["assembling-machine-3"] = 1,
    ["electric-mining-drill"] = 2,
    ["burner-mining-drill"] = 2,
    ["stone-furnace"] = 3,
    ["steel-furnace"] = 3,
    ["electric-furnace"] = 3,
    ["gun-turret"] = 4,
    ["laser-turret"] = 4,
    ["radar"] = 5,
    ["roboport"] = 6,
    ["wooden-chest"] = 7,
    ["iron-chest"] = 7,
    ["steel-chest"] = 7,
    ["small-electric-pole"] = 8,
    ["medium-electric-pole"] = 8,
    ["big-electric-pole"] = 8,
    ["substation"] = 8,
    ["transport-belt"] = 9,
    ["fast-transport-belt"] = 9,
    ["express-transport-belt"] = 9,
    ["inserter"] = 10,
    ["fast-inserter"] = 10,
    ["long-handed-inserter"] = 10,
    ["iron-ore"] = 11,
    ["copper-ore"] = 12,
    ["coal"] = 13,
    ["stone"] = 14
}

local function serialize_entity(e)
    if not (e and e.valid) then return nil end
    local type_id = ENTITY_TYPES_MAP[e.name] or 0
    local status_val = 0
    if e.status then
        if e.status == defines.entity_status.working then
            status_val = 1
        elseif e.status == defines.entity_status.low_power then
            status_val = 2
        elseif e.status == defines.entity_status.no_fuel then
            status_val = 3
        elseif e.status == defines.entity_status.no_ingredients then
            status_val = 4
        else
            status_val = 1
        end
    else
        status_val = 1
    end

    local prog = 0.0
    if e.crafting_progress then
        prog = e.crafting_progress
    end

    -- Direction in radians
    local dir_rad = 0.0
    if e.direction then
        -- 0 = North, 2 = East, 4 = South, 6 = West
        dir_rad = (e.direction / 8.0) * (2.0 * math.pi)
    end

    return {
        id = e.unit_number or 0,
        type = type_id,
        name = e.name,
        x = e.position.x,
        y = e.position.y,
        orientation = dir_rad,
        status = status_val,
        progress = prog
    }
end

remote.add_interface("xfactorvr", {
    get_entities = function(radius, cx, cy)
        radius = radius or 30.0
        cx = cx or 0.0
        cy = cy or 0.0
        local surface = game.surfaces[1]
        if not surface then return "[]" end

        local area = {{cx - radius, cy - radius}, {cx + radius, cy + radius}}
        local found = surface.find_entities(area)
        local results = {}
        for _, e in ipairs(found) do
            local s = serialize_entity(e)
            if s and s.type > 0 then
                table.insert(results, string.format(
                    '{"t":%d,"x":%.2f,"y":%.2f,"o":%.2f,"s":%d,"p":%.2f}',
                    s.type, s.x, s.y, s.orientation, s.status, s.progress
                ))
            end
        end

        return "[" .. table.concat(results, ",") .. "]"
    end,

    place_entity = function(type_name, x, y, direction)
        local surface = game.surfaces[1]
        if not surface then return false end
        direction = direction or defines.direction.north

        local created = surface.create_entity{
            name = type_name,
            position = {x, y},
            direction = direction,
            force = "player",
            fast_replace = true,
            spill = false
        }
        return created ~= nil
    end,

    mine_entity = function(x, y)
        local surface = game.surfaces[1]
        if not surface then return false end
        local found = surface.find_entities_filtered{
            position = {x, y},
            radius = 1.0
        }
        for _, e in ipairs(found) do
            if e.valid and e.type ~= "character" and e.type ~= "resource" then
                e.destroy()
                return true
            end
        end
        return false
    end,

    get_inventory = function()
        local player = game.players[1]
        if not (player and player.valid) then return "[]" end
        local inv = player.get_main_inventory()
        if not inv then return "[]" end

        local items = {}
        for name, count in pairs(inv.get_contents()) do
            table.insert(items, string.format('{"name":"%s","count":%d}', name, count))
        end
        return "[" .. table.concat(items, ",") .. "]"
    end,

    craft = function(recipe_name, count)
        local player = game.players[1]
        if not (player and player.valid) then return false end
        count = count or 1
        player.begin_crafting{
            recipe = recipe_name,
            count = count
        }
        return true
    end,

    set_recipe = function(x, y, recipe_name)
        local surface = game.surfaces[1]
        if not surface then return false end
        local found = surface.find_entities_filtered{
            position = {x, y},
            radius = 1.0,
            type = "assembling-machine"
        }
        if found[1] and found[1].valid then
            found[1].set_recipe(recipe_name)
            return true
        end
        return false
    end
})

script.on_init(function()
    log("[XFactorVR] Bridge mod initialized on game world.")
end)
