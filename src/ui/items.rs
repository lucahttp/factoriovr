//! items.rs — Ids de item del mod de Factorio, replicados para el HUD.
//!
//! Estos valores son los mismos que los de `game_bridge::ITEM_*` (el mod Lua
//! los llama por número), pero el módulo `ui` NO depende de `game_bridge`:
//! así el HUD se puede compilar y probar solo, sin el motor ni la red.
//!
//! Si cambian en `game_bridge`, hay que cambiarlos aquí también. Hay un test
//! que lo recuerda.

/// Mineral de hierro.
pub const ITEM_IRON_ORE: u32 = 1;
/// Mineral de cobre.
pub const ITEM_COPPER_ORE: u32 = 2;
/// Carbón.
pub const ITEM_COAL: u32 = 3;
/// Piedra.
pub const ITEM_STONE: u32 = 4;
/// Plancha de hierro.
pub const ITEM_IRON_PLATE: u32 = 5;
/// Plancha de cobre.
pub const ITEM_COPPER_PLATE: u32 = 6;
/// Engranaje de hierro.
pub const ITEM_IRON_GEAR: u32 = 7;
/// Circuito electrónico.
pub const ITEM_ELECTRONIC_CIRCUIT: u32 = 8;
/// Ensambladora.
pub const ITEM_ASSEMBLING_MACHINE: u32 = 9;
/// Minera eléctrica.
pub const ITEM_MINING_DRILL: u32 = 10;
/// Horno de piedra.
pub const ITEM_FURNACE: u32 = 11;
/// Cinta transportadora.
pub const ITEM_TRANSPORT_BELT: u32 = 12;
/// Insertador.
pub const ITEM_INSERTER: u32 = 13;
/// Poste eléctrico.
pub const ITEM_POWER_POLE: u32 = 14;
/// Torreta.
pub const ITEM_TURRET: u32 = 15;
/// Radar.
pub const ITEM_RADAR: u32 = 16;
/// Robopuerto.
pub const ITEM_ROBOPORT: u32 = 17;
/// Cofre.
pub const ITEM_CHEST: u32 = 18;

/// Nombre corto y en español de un item (para el HUD).
pub fn nombre_item(item_id: u32) -> &'static str {
    match item_id {
        ITEM_IRON_ORE => "MINERAL DE HIERRO",
        ITEM_COPPER_ORE => "MINERAL DE COBRE",
        ITEM_COAL => "CARBON",
        ITEM_STONE => "PIEDRA",
        ITEM_IRON_PLATE => "PLANCHA DE HIERRO",
        ITEM_COPPER_PLATE => "PLANCHA DE COBRE",
        ITEM_IRON_GEAR => "ENGRANAJE",
        ITEM_ELECTRONIC_CIRCUIT => "CIRCUITO",
        ITEM_ASSEMBLING_MACHINE => "ENSAMBLADORA",
        ITEM_MINING_DRILL => "MINERA",
        ITEM_FURNACE => "HORNO",
        ITEM_TRANSPORT_BELT => "CINTA",
        ITEM_INSERTER => "INSERTADOR",
        ITEM_POWER_POLE => "POSTE",
        ITEM_TURRET => "TORRETA",
        ITEM_RADAR => "RADAR",
        ITEM_ROBOPORT => "ROBOPUERTO",
        ITEM_CHEST => "COFRE",
        _ => "OBJETO",
    }
}
