// game_bridge.rs — Puente bidireccional con el motor real de Factorio
//
// Dos caminos de juego:
//
// 1. MODO STANDALONE (el objetivo): el visor (Pico 4 Ultra) habla por RCON con
//    un servidor Factorio headless ARM64 que corre dentro del propio visor.
//    El mod `xfactorvr_bridge` serializa el estado real de la fabrica y el
//    puente lo refleja en la mesa holografica. Cuando la conexion esta viva,
//    Factorio es la unica fuente de verdad.
//
// 2. MODO RESPALDO (sin servidor): una simulacion local reproduce el circuito
//    basico de Factorio (perforar, fundir, fabricar, cintar, insertar) para
//    que el juego siga siendo jugable si el servidor no arranca.
//
// Direcciones: Factorio 2.0 usa 16 direcciones (0 = norte, 4 = este) y los
// radianes son dir * TAU/16. Ver factorio-3d-mod/src/entities.rs (dir_to_yaw).

use crate::tabletop::LiveEntity;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

// --- Items (deben coincidir con ITEM_IDS_MAP del mod Lua) -------------------
pub const ITEM_IRON_ORE: u32 = 1;
pub const ITEM_COPPER_ORE: u32 = 2;
pub const ITEM_COAL: u32 = 3;
pub const ITEM_STONE: u32 = 4;
pub const ITEM_IRON_PLATE: u32 = 5;
pub const ITEM_COPPER_PLATE: u32 = 6;
pub const ITEM_IRON_GEAR: u32 = 7;
pub const ITEM_ELECTRONIC_CIRCUIT: u32 = 8;
pub const ITEM_ASSEMBLING_MACHINE: u32 = 9;
pub const ITEM_MINING_DRILL: u32 = 10;
pub const ITEM_FURNACE: u32 = 11;
pub const ITEM_TRANSPORT_BELT: u32 = 12;
pub const ITEM_INSERTER: u32 = 13;
pub const ITEM_POWER_POLE: u32 = 14;
pub const ITEM_TURRET: u32 = 15;
pub const ITEM_RADAR: u32 = 16;
pub const ITEM_ROBOPORT: u32 = 17;
pub const ITEM_CHEST: u32 = 18;

// --- Ritmos de la simulacion local (ciclos por segundo) --------------------
const DRILL_CYCLES_PER_SEC: f32 = 0.5;
const FURNACE_CYCLES_PER_SEC: f32 = 0.3125;
const ASSEMBLER_CYCLES_PER_SEC: f32 = 0.7;
const BELT_CYCLES_PER_SEC: f32 = 1.8;
const INSERTER_CYCLES_PER_SEC: f32 = 1.5;

// --- Parametros del puente RCON --------------------------------------------
const RCON_DEFAULT_HOST: &str = "127.0.0.1";
const RCON_DEFAULT_PORT: u16 = 27015;
const RCON_PASSWORD: &str = "xfactorvr";
const RCON_HOST_FILE: &str = "/sdcard/XFactorVR/rcon_host.txt";
const RCON_RECONNECT_INTERVAL: f32 = 3.0;
const RCON_POLL_INTERVAL: f32 = 0.12;
const RCON_SNAPSHOT_RADIUS: f32 = 32.0;
const RCON_CONNECT_TIMEOUT_MS: u64 = 120;
const RCON_READ_TIMEOUT_MS: u64 = 400;
/// Tope del cuerpo de un paquete RCON. Un snapshot completo con miles de
/// entidades puede pesar cientos de KB, asi que el limite debe ser holgado.
const RCON_MAX_BODY: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ResourcePatch {
    pub item_type: u32,
    pub tile_x: f32,
    pub tile_y: f32,
    pub radius: f32,
    pub remaining: u32,
}

#[derive(Clone, Debug)]
pub struct MachineState {
    pub input_buffer: HashMap<u32, u32>,
    pub output_buffer: HashMap<u32, u32>,
    pub fuel_remaining: f32,
    pub recipe_id: u32,
    pub work_progress: f32,
}

impl Default for MachineState {
    fn default() -> Self {
        Self {
            input_buffer: HashMap::new(),
            output_buffer: HashMap::new(),
            fuel_remaining: 100.0,
            recipe_id: 1, // engranaje de hierro
            work_progress: 0.0,
        }
    }
}

/// Receta activa de una ensambladora en la simulacion local.
fn assembler_recipe(recipe_id: u32) -> (&'static [(u32, u32)], u32, u32) {
    match recipe_id {
        // 1 plancha de hierro + 2 de cobre -> 1 circuito electronico
        2 => (&[(ITEM_IRON_PLATE, 1), (ITEM_COPPER_PLATE, 2)], ITEM_ELECTRONIC_CIRCUIT, 1),
        // 1 plancha de hierro + 1 engranaje -> 2 cintas transportadoras
        3 => (&[(ITEM_IRON_PLATE, 1), (ITEM_IRON_GEAR, 1)], ITEM_TRANSPORT_BELT, 2),
        // 2 planchas de hierro -> 1 engranaje
        _ => (&[(ITEM_IRON_PLATE, 2)], ITEM_IRON_GEAR, 1),
    }
}

/// Direccion frontal de una entidad expresada en pasos de casilla.
/// Con la convencion de Factorio 2.0: norte = (0,-1), este = (1,0).
fn facing_delta(orientation: f32) -> (i32, i32) {
    let dx = orientation.sin().round() as i32;
    let dy = (-orientation.cos()).round() as i32;
    (dx, dy)
}

/// Tipo de entidad segun la tabla compartida con el mod Lua.
fn is_machine_type(entity_type: u32) -> bool {
    matches!(
        entity_type,
        crate::tabletop::ENTITY_ASSEMBLING_MACHINE
            | crate::tabletop::ENTITY_MINING_DRILL
            | crate::tabletop::ENTITY_FURNACE
            | crate::tabletop::ENTITY_CHEST
            | crate::tabletop::ENTITY_TRANSPORT_BELT
            | crate::tabletop::ENTITY_INSERTER
    )
}

pub struct SimulationWorld {
    pub patches: Vec<ResourcePatch>,
    /// Terreno real del area visible (lo que se pinta en el suelo del diorama).
    pub terrain: TerrainSnapshot,
    pub machine_states: HashMap<(i32, i32), MachineState>,
    pub player_inventory: HashMap<u32, u32>,
    pub is_connected_to_rcon: bool,
    /// Centro de la vista actual en casillas: define que area pide al servidor.
    pub view_center: (f32, f32),
}

impl SimulationWorld {
    pub fn new() -> Self {
        let mut inv = HashMap::new();
        // Pack inicial autentico de Factorio (Crash Site / Level 1 start)
        inv.insert(ITEM_IRON_PLATE, 8);
        inv.insert(ITEM_MINING_DRILL, 1);
        inv.insert(ITEM_FURNACE, 1);
        inv.insert(ITEM_COAL, 1);

        let patches = vec![
            ResourcePatch {
                item_type: ITEM_IRON_ORE,
                tile_x: -6.0,
                tile_y: 4.0,
                radius: 3.0,
                remaining: 50000,
            },
            ResourcePatch {
                item_type: ITEM_COPPER_ORE,
                tile_x: 6.0,
                tile_y: 4.0,
                radius: 2.5,
                remaining: 35000,
            },
            ResourcePatch {
                item_type: ITEM_COAL,
                tile_x: 0.0,
                tile_y: -5.0,
                radius: 2.5,
                remaining: 40000,
            },
            ResourcePatch {
                item_type: ITEM_STONE,
                tile_x: -6.0,
                tile_y: -4.0,
                radius: 2.0,
                remaining: 20000,
            },
        ];

        Self {
            patches,
            terrain: TerrainSnapshot::default(),
            machine_states: HashMap::new(),
            player_inventory: inv,
            is_connected_to_rcon: false,
            view_center: (0.0, 0.0),
        }
    }

    /// Reinicia el mundo de simulacion al estado inicial de nivel
    pub fn reset_world(&mut self) {
        let mut inv = HashMap::new();
        inv.insert(ITEM_IRON_PLATE, 8);
        inv.insert(ITEM_MINING_DRILL, 1);
        inv.insert(ITEM_FURNACE, 1);
        inv.insert(ITEM_COAL, 1);

        self.player_inventory = inv;
        self.machine_states.clear();
        self.view_center = (0.0, 0.0);
    }

    /// Minado manual con pico sobre yacimientos de mineral o rocas
    pub fn manual_mine(&mut self, tile_x: f32, tile_y: f32) -> u32 {
        if let Some(ore_type) = Self::get_resource_in_patches(&self.patches, tile_x, tile_y) {
            for patch in self.patches.iter_mut() {
                let dx = tile_x - patch.tile_x;
                let dy = tile_y - patch.tile_y;
                if dx * dx + dy * dy <= patch.radius * patch.radius && patch.remaining > 0 {
                    patch.remaining = patch.remaining.saturating_sub(1);
                    break;
                }
            }
            let cnt = self.player_inventory.entry(ore_type).or_insert(0);
            *cnt += 1;
            ore_type
        } else {
            0
        }
    }

    /// Interactua con una maquina (recoge productos y suministra combustible/mineral)
    pub fn interact_machine(&mut self, tile_x: f32, tile_y: f32) -> u32 {
        let key = (tile_x.round() as i32, tile_y.round() as i32);
        let mut total_looted = 0;
        if let Some(state) = self.machine_states.get_mut(&key) {
            // 1. Recoger productos terminados
            let produced: Vec<(u32, u32)> = state
                .output_buffer
                .iter()
                .filter(|(_, &q)| q > 0)
                .map(|(&id, &q)| (id, q))
                .collect();
            for (item_id, qty) in produced {
                *self.player_inventory.entry(item_id).or_insert(0) += qty;
                total_looted += qty;
            }
            state.output_buffer.clear();

            // 2. Abastecer de carbon si esta bajo de combustible
            if state.fuel_remaining < 50.0 {
                if let Some(coal_cnt) = self.player_inventory.get_mut(&ITEM_COAL) {
                    if *coal_cnt > 0 {
                        let transfer = (*coal_cnt).min(5);
                        *coal_cnt -= transfer;
                        state.fuel_remaining += transfer as f32 * 100.0;
                    }
                }
            }

            // 3. Cargar mineral en el horno (no en las ensambladoras)
            let wants_ore = state.recipe_id == 0 || state.recipe_id == 1;
            if wants_ore {
                for ore in [ITEM_IRON_ORE, ITEM_COPPER_ORE] {
                    if let Some(cnt) = self.player_inventory.get_mut(&ore) {
                        if *cnt > 0 {
                            let transfer = (*cnt).min(5);
                            *cnt -= transfer;
                            *state.input_buffer.entry(ore).or_insert(0) += transfer;
                        }
                    }
                }
            }
        }
        total_looted
    }

    /// Obtiene informacion de estado de una maquina para el HUD / GUI
    pub fn get_machine_info(&self, tile_x: f32, tile_y: f32) -> Option<(f32, f32, u32, u32)> {        let key = (tile_x.round() as i32, tile_y.round() as i32);
        self.machine_states.get(&key).map(|s| {
            let input_ore = *s.input_buffer.get(&ITEM_IRON_ORE).unwrap_or(&0) +
                            *s.input_buffer.get(&ITEM_COPPER_ORE).unwrap_or(&0);
            let output_plates = *s.output_buffer.get(&ITEM_IRON_PLATE).unwrap_or(&0) +
                                *s.output_buffer.get(&ITEM_COPPER_PLATE).unwrap_or(&0);
            (s.fuel_remaining, s.work_progress, input_ore, output_plates)
        })
    }

    /// Aplica una instantanea enviada por el motor real de Factorio.
    /// La lista de entidades de la mesa se reemplaza por completo: Factorio es
    /// la unica fuente de verdad cuando el puente esta conectado.
    pub fn apply_snapshot(&mut self, entities: &mut Vec<LiveEntity>, raw: &str) {
        let snap: Snapshot = match serde_json::from_str(raw) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Instantanea RCON ilegible: {}", e);
                return;
            }
        };

        entities.clear();
        entities.reserve(snap.entities.len());
        for e in snap.entities {
            // El mod manda el nombre del prototipo; el id numerico solo
            // aparece con despliegues antiguos del mod.
            let (type_id, type_name) = match &e.entity_type {
                SnapshotType::Name(n) => (entity_type_from_name(n), n.clone()),
                SnapshotType::Id(i) => (*i, String::new()),
            };
            if type_id == 0 {
                continue;
            }
            // Id del sprite real en el atlas (0 si el manifiesto no esta
            // desplegado o el tipo no tiene sprite).
            let sprite_id = if type_name.is_empty() {
                0
            } else {
                crate::assets::sprite_id_for_type(&type_name)
            };
            entities.push(LiveEntity {
                entity_type: type_id,
                tile_x: e.x,
                tile_y: e.y,
                orientation: e.orientation,
                status: e.status,
                animation_progress: e.progress,
                health_ratio: e.health,
                arm_angle: e.arm_angle,
                sprite_id,
                anim_frame: e.anim_frame,
            });
        }

        self.patches = snap
            .patches
            .into_iter()
            .map(|p| ResourcePatch {
                item_type: p.item_type,
                tile_x: p.x,
                tile_y: p.y,
                radius: p.radius,
                remaining: p.remaining,
            })
            .collect();
        self.terrain = snap.terrain;
    }

    /// Aplica el inventario real del jugador.
    pub fn apply_inventory(&mut self, raw: &str) {
        let items: Vec<InventoryItem> = match serde_json::from_str(raw) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Inventario RCON ilegible: {}", e);
                return;
            }
        };
        self.player_inventory.clear();
        for it in items {
            if it.id > 0 && it.count > 0 {
                self.player_inventory.insert(it.id, it.count);
            }
        }
    }

    /// Devuelve el item que viaja sobre una cinta o que sostiene un insertador.
    /// Se usa para dibujar las piezas que circulan por la fabrica en tiempo real.
    pub fn get_carried_item(&self, tile_x: f32, tile_y: f32) -> Option<(u32, u32)> {
        let key = (tile_x.round() as i32, tile_y.round() as i32);
        let state = self.machine_states.get(&key)?;
        let mut best: Option<(u32, u32)> = None;
        for (&id, &qty) in state.output_buffer.iter() {
            if qty > 0 && best.map(|(_, b)| qty > b).unwrap_or(true) {
                best = Some((id, qty));
            }
        }
        best
    }

    /// Comprueba si una posicion esta sobre un yacimiento de mineral
    pub fn get_resource_in_patches(patches: &[ResourcePatch], x: f32, y: f32) -> Option<u32> {
        for patch in patches {
            let dx = x - patch.tile_x;
            let dy = y - patch.tile_y;
            if dx * dx + dy * dy <= patch.radius * patch.radius && patch.remaining > 0 {
                return Some(patch.item_type);
            }
        }
        None
    }

    pub fn get_resource_at(&self, x: f32, y: f32) -> Option<u32> {
        Self::get_resource_in_patches(&self.patches, x, y)
    }

    /// Avanza la simulacion local por pasos discretos estables.
    /// Solo se usa cuando no hay motor de Factorio conectado.
    pub fn tick(&mut self, entities: &mut [LiveEntity], dt: f32) {
        let mut rem = dt;
        while rem > 0.0001 {
            let step = rem.min(0.2);
            self.tick_step(entities, step);
            rem -= step;
        }
    }

    /// Resuelve que maquina ocupa (o cubre) una casilla. Las maquinas de 2x2 y
    /// 3x3 ocupan casillas alrededor de su origen, asi que se busca con radio 1.
    fn resolve_machine(
        entities: &[LiveEntity],
        states: &HashMap<(i32, i32), MachineState>,
        tile: (i32, i32),
    ) -> Option<(i32, i32)> {
        if states.contains_key(&tile) {
            return Some(tile);
        }
        let mut best: Option<(i32, i32)> = None;
        let mut best_dist = i32::MAX;
        for e in entities.iter() {
            if !is_machine_type(e.entity_type) {
                continue;
            }
            let k = (e.tile_x.round() as i32, e.tile_y.round() as i32);
            if (k.0 - tile.0).abs() <= 1 && (k.1 - tile.1).abs() <= 1 {
                let d = (k.0 - tile.0).abs() + (k.1 - tile.1).abs();
                if d < best_dist && states.contains_key(&k) {
                    best_dist = d;
                    best = Some(k);
                }
            }
        }
        best
    }

    fn tick_step(&mut self, entities: &mut [LiveEntity], dt: f32) {
        // El mapa de estados se saca de self para poder leer self.patches
        // (inmutable) mientras se modifica (mutable) sin conflicto de prestamos.
        let mut states = std::mem::take(&mut self.machine_states);

        // --- Fase 1: cada entidad avanza su propio ciclo -------------------
        // Los destinos se resuelven despues del recorrido: mantener a la vez
        // un prestamo mutable del mapa de estados y otro de las entidades es
        // incompatible en Rust.
        let mut pending_drops: Vec<((i32, i32), (i32, i32), u32)> = Vec::new();

        for i in 0..entities.len() {
            let (etype, tile_x, tile_y, orientation) = {
                let e = &entities[i];
                (e.entity_type, e.tile_x, e.tile_y, e.orientation)
            };
            let key = (tile_x.round() as i32, tile_y.round() as i32);
            let ore_at_loc = Self::get_resource_in_patches(&self.patches, tile_x, tile_y);
            let (dx, dy) = facing_delta(orientation);
            let state = states.entry(key).or_default();

            match etype {
                // Perforadora: extrae mineral y lo empuja a la casilla frontal.
                crate::tabletop::ENTITY_MINING_DRILL => {
                    if let Some(ore_type) = ore_at_loc {
                        entities[i].status = 1; // trabajando
                        state.work_progress += dt * DRILL_CYCLES_PER_SEC;
                        if state.work_progress >= 1.0 {
                            state.work_progress -= 1.0;
                            pending_drops.push(((key.0 + dx, key.1 + dy), key, ore_type));
                        }
                        entities[i].animation_progress = state.work_progress;
                    } else {
                        entities[i].status = 4; // sin recursos
                        entities[i].animation_progress = 0.0;
                    }
                }

                // Horno: funde mineral con carbon.
                crate::tabletop::ENTITY_FURNACE => {
                    let iron_ore_count = *state.input_buffer.get(&ITEM_IRON_ORE).unwrap_or(&0);
                    let copper_ore_count = *state.input_buffer.get(&ITEM_COPPER_ORE).unwrap_or(&0);
                    let has_ore = iron_ore_count > 0 || copper_ore_count > 0;

                    if state.fuel_remaining <= 0.0 {
                        if let Some(coal_cnt) = state.input_buffer.get_mut(&ITEM_COAL) {
                            if *coal_cnt > 0 {
                                *coal_cnt -= 1;
                                state.fuel_remaining += 100.0;
                            }
                        }
                    }

                    if has_ore && state.fuel_remaining > 0.0 {
                        entities[i].status = 1; // ardiendo
                        state.fuel_remaining = (state.fuel_remaining - dt * 1.5).max(0.0);
                        state.work_progress += dt * FURNACE_CYCLES_PER_SEC;
                        if state.work_progress >= 1.0 {
                            state.work_progress -= 1.0;
                            if iron_ore_count > 0 {
                                if let Some(cnt) = state.input_buffer.get_mut(&ITEM_IRON_ORE) {
                                    *cnt -= 1;
                                }
                                *state.output_buffer.entry(ITEM_IRON_PLATE).or_insert(0) += 1;
                            } else {
                                if let Some(cnt) = state.input_buffer.get_mut(&ITEM_COPPER_ORE) {
                                    *cnt -= 1;
                                }
                                *state.output_buffer.entry(ITEM_COPPER_PLATE).or_insert(0) += 1;
                            }
                        }
                        entities[i].animation_progress = state.work_progress;
                    } else if has_ore {
                        entities[i].status = 3; // sin combustible
                        entities[i].animation_progress = 0.0;
                    } else {
                        entities[i].status = 0; // inactivo
                        entities[i].animation_progress = 0.0;
                    }
                }

                // Ensambladora: consume insumos y produce el resultado de su receta.
                crate::tabletop::ENTITY_ASSEMBLING_MACHINE => {
                    let (ingredients, out_item, out_count) = assembler_recipe(state.recipe_id);
                    let can_craft = ingredients
                        .iter()
                        .all(|(item, needed)| *state.input_buffer.get(item).unwrap_or(&0) >= *needed);

                    if can_craft {
                        entities[i].status = 1; // fabricando
                        state.work_progress += dt * ASSEMBLER_CYCLES_PER_SEC;
                        if state.work_progress >= 1.0 {
                            state.work_progress -= 1.0;
                            for (item, needed) in ingredients {
                                if let Some(cnt) = state.input_buffer.get_mut(item) {
                                    *cnt = cnt.saturating_sub(*needed);
                                }
                            }
                            *state.output_buffer.entry(out_item).or_insert(0) += out_count;
                        }
                        entities[i].animation_progress = state.work_progress;
                    } else {
                        entities[i].status = 4; // sin ingredientes
                        entities[i].animation_progress = 0.0;
                    }
                }

                // Radar: rotacion continua.
                crate::tabletop::ENTITY_RADAR => {
                    entities[i].status = 1;
                    entities[i].animation_progress =
                        (entities[i].animation_progress + dt * 0.25).rem_euclid(1.0);
                }

                // Torreta: barrido defensivo.
                crate::tabletop::ENTITY_GUN_TURRET => {
                    entities[i].status = 1;
                    entities[i].animation_progress =
                        (entities[i].animation_progress + dt * 0.4).rem_euclid(1.0);
                }

                // Cofre: solo almacena.
                crate::tabletop::ENTITY_CHEST => {
                    entities[i].status = 1;
                }

                _ => {
                    entities[i].status = 1;
                    entities[i].animation_progress =
                        (entities[i].animation_progress + dt * 0.2).rem_euclid(1.0);
                }
            }
        }

        // Entrega del mineral extraido por las perforadoras.
        for (target, drill_key, ore_type) in pending_drops {
            match Self::resolve_machine(&entities[..], &states, target) {
                Some(dst) => {
                    *states
                        .entry(dst)
                        .or_default()
                        .input_buffer
                        .entry(ore_type)
                        .or_insert(0) += 1;
                }
                None => {
                    // Sin receptor delante: el mineral se queda en la salida de
                    // la propia perforadora (de ahi lo tomara un insertador).
                    if let Some(drill) = states.get_mut(&drill_key) {
                        *drill.output_buffer.entry(ore_type).or_insert(0) += 1;
                    }
                }
            }
        }

        // --- Fase 2: cintas transportadoras ---------------------------------
        // La cinta toma lo que espera en su entrada, lo lleva encima y lo
        // entrega a la casilla frontal.
        for i in 0..entities.len() {
            let (etype, tile_x, tile_y, orientation) = {
                let e = &entities[i];
                (e.entity_type, e.tile_x, e.tile_y, e.orientation)
            };
            if etype != crate::tabletop::ENTITY_TRANSPORT_BELT {
                continue;
            }
            let key = (tile_x.round() as i32, tile_y.round() as i32);
            let (dx, dy) = facing_delta(orientation);

            // 1. Avanza el reloj de la cinta y carga lo que esperaba.
            let ready = {
                let state = states.entry(key).or_default();
                state.work_progress += dt * BELT_CYCLES_PER_SEC;
                if state.work_progress < 1.0 {
                    entities[i].animation_progress = state.work_progress;
                    false
                } else {
                    state.work_progress -= 1.0;
                    entities[i].animation_progress = state.work_progress;
                    // Subir a la cinta lo que esperaba en la entrada.
                    let waiting: Vec<(u32, u32)> = state
                        .input_buffer
                        .iter()
                        .filter(|(_, &q)| q > 0)
                        .map(|(&id, &q)| (id, q))
                        .collect();
                    for (id, qty) in waiting {
                        *state.output_buffer.entry(id).or_insert(0) += qty;
                        state.input_buffer.insert(id, 0);
                    }
                    true
                }
            };
            if !ready {
                continue;
            }

            // 2. Entregar a la casilla frontal (otra cinta, cofre o maquina).
            let front = (key.0 + dx, key.1 + dy);
            let dst = Self::resolve_machine(&entities[..], &states, front);
            let carried: Vec<(u32, u32)> = {
                let state = states.entry(key).or_default();
                state
                    .output_buffer
                    .iter()
                    .filter(|(_, &q)| q > 0)
                    .map(|(&id, &q)| (id, q))
                    .collect()
            };
            for (id, qty) in carried {
                let delivered = match dst {
                    Some(d) => {
                        let target = states.entry(d).or_default();
                        // Una maquina de 2x2/3x3 acepta un maximo razonable.
                        let cap: u32 = 200;
                        let current = *target.input_buffer.get(&id).unwrap_or(&0);
                        let room = cap.saturating_sub(current);
                        let moved = qty.min(room);
                        if moved > 0 {
                            *target.input_buffer.entry(id).or_insert(0) += moved;
                            let belt = states.entry(key).or_default();
                            if let Some(cnt) = belt.output_buffer.get_mut(&id) {
                                *cnt -= moved;
                            }
                        }
                        moved
                    }
                    None => 0,
                };
                if delivered == 0 {
                    // Sin destino: la carga se queda en la cinta.
                    break;
                }
            }
        }

        // --- Fase 3: insertadores roboticos --------------------------------
        // El insertador toma de la casilla trasera y deja en la frontal.
        // (En Factorio el insertador mira hacia donde suelta y recoge por detras.)
        for i in 0..entities.len() {
            let (etype, tile_x, tile_y, orientation) = {
                let e = &entities[i];
                (e.entity_type, e.tile_x, e.tile_y, e.orientation)
            };
            if etype != crate::tabletop::ENTITY_INSERTER {
                continue;
            }
            let key = (tile_x.round() as i32, tile_y.round() as i32);
            let (dx, dy) = facing_delta(orientation);

            let ready = {
                let state = states.entry(key).or_default();
                state.work_progress += dt * INSERTER_CYCLES_PER_SEC;
                entities[i].animation_progress = state.work_progress.rem_euclid(1.0);
                entities[i].status = 1;
                if state.work_progress < 1.0 {
                    false
                } else {
                    state.work_progress -= 1.0;
                    true
                }
            };
            if !ready {
                continue;
            }

            let back = (key.0 - dx, key.1 - dy);
            let front = (key.0 + dx, key.1 + dy);
            let src = Self::resolve_machine(&entities[..], &states, back);
            let dst = Self::resolve_machine(&entities[..], &states, front);
            let (src, dst) = match (src, dst) {
                (Some(s), Some(d)) => (s, d),
                _ => continue,
            };

            // Elegir el item: primero la salida de la maquina y, para cintas o
            // cofres, tambien su entrada.
            let mut picked: Option<u32> = None;
            if let Some(s) = states.get_mut(&src) {
                let produced: Vec<u32> = s.output_buffer.keys().copied().collect();
                for id in produced {
                    if *s.output_buffer.get(&id).unwrap_or(&0) > 0 {
                        picked = Some(id);
                        break;
                    }
                }
                if picked.is_none() {
                    let pending: Vec<u32> = s.input_buffer.keys().copied().collect();
                    for id in pending {
                        if *s.input_buffer.get(&id).unwrap_or(&0) > 0 {
                            picked = Some(id);
                            break;
                        }
                    }
                }
            }

            if let Some(item) = picked {
                // Retirar una unidad del origen (de la salida o de la entrada).
                if let Some(s) = states.get_mut(&src) {
                    if *s.output_buffer.get(&item).unwrap_or(&0) > 0 {
                        if let Some(cnt) = s.output_buffer.get_mut(&item) {
                            *cnt = cnt.saturating_sub(1);
                        }
                    } else if let Some(cnt) = s.input_buffer.get_mut(&item) {
                        *cnt = cnt.saturating_sub(1);
                    }
                }
                if let Some(target) = states.get_mut(&dst) {
                    *target.input_buffer.entry(item).or_insert(0) += 1;
                }
            }
        }

        self.machine_states = states;
    }
}

// --- Cliente RCON ----------------------------------------------------------

pub struct RconClient {
    stream: Option<TcpStream>,
    request_id: i32,
}

impl RconClient {
    pub fn new() -> Self {
        Self {
            stream: None,
            request_id: 1,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    /// Conecta y autentica. La respuesta de autenticacion DEBE consumirse: si
    /// se deja en el buffer, el siguiente execute() la leeria en lugar de la
    /// respuesta del comando y el puente se desincroniza.
    pub fn connect(&mut self, addr: &str, password: &str) -> bool {
        let target: std::net::SocketAddr = match addr.parse() {
            Ok(a) => a,
            Err(_) => format!("{}:{}", RCON_DEFAULT_HOST, RCON_DEFAULT_PORT)
                .parse()
                .unwrap_or_else(|_| {
                    std::net::SocketAddr::from(([127, 0, 0, 1], RCON_DEFAULT_PORT))
                }),
        };

        let stream = match TcpStream::connect_timeout(&target, Duration::from_millis(RCON_CONNECT_TIMEOUT_MS)) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(RCON_READ_TIMEOUT_MS)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(RCON_READ_TIMEOUT_MS)));
        self.stream = Some(stream);
        self.request_id = 1;

        // SERVERDATA_AUTH (tipo 3)
        if !self.send_packet(3, password) {
            self.stream = None;
            return false;
        }

        // El servidor responde con SERVERDATA_AUTH_RESPONSE (tipo 2). Algunas
        // implementaciones envian antes un paquete vacio (tipo 0).
        let mut authenticated = false;
        for _ in 0..2 {
            match self.read_packet() {
                Some((2, _)) => {
                    authenticated = true;
                    break;
                }
                Some((0, _)) => continue, // paquete vacio previo
                _ => break,
            }
        }
        if !authenticated {
            self.stream = None;
            return false;
        }
        true
    }

    pub fn disconnect(&mut self) {
        self.stream = None;
    }

    fn send_packet(&mut self, packet_type: i32, body: &str) -> bool {
        let stream = match self.stream.as_mut() {
            Some(s) => s,
            None => return false,
        };

        let body_bytes = body.as_bytes();
        let length = (body_bytes.len() + 10) as i32;
        let id = self.request_id;
        self.request_id += 1;

        let mut buf = Vec::with_capacity(length as usize + 4);
        buf.extend_from_slice(&length.to_le_bytes());
        buf.extend_from_slice(&id.to_le_bytes());
        buf.extend_from_slice(&packet_type.to_le_bytes());
        buf.extend_from_slice(body_bytes);
        buf.push(0); // termina cadena
        buf.push(0); // relleno

        stream.write_all(&buf).is_ok()
    }

    /// Lee un paquete RCON completo: (tipo, cuerpo).
    fn read_packet(&mut self) -> Option<(i32, String)> {
        let stream = self.stream.as_mut()?;
        let mut header = [0u8; 12];
        stream.read_exact(&mut header).ok()?;
        let length = i32::from_le_bytes(header[0..4].try_into().ok()?);
        if length < 10 || length as usize > RCON_MAX_BODY {
            return None;
        }
        let body_len = (length - 10) as usize;
        let mut body = vec![0u8; body_len];
        stream.read_exact(&mut body).ok()?;
        let ptype = i32::from_le_bytes(header[8..12].try_into().ok()?);
        if body.len() >= 2 {
            body.truncate(body.len() - 2);
        }
        Some((ptype, String::from_utf8_lossy(&body).to_string()))
    }

    /// Envia un comando y devuelve el cuerpo de la primera respuesta.
    pub fn execute(&mut self, command: &str) -> Option<String> {
        if !self.send_packet(2, command) {
            return None;
        }
        self.read_packet().map(|(_, body)| body)
    }

    /// Envia un comando y acumula las respuestas. Factorio puede repartir la
    /// salida entre varios paquetes, asi que se leen hasta `max_packets`.
    pub fn execute_collect(&mut self, command: &str, max_packets: usize) -> Option<String> {
        if !self.send_packet(2, command) {
            return None;
        }
        let mut out = String::new();
        for i in 0..max_packets.max(1) {
            match self.read_packet() {
                Some((_, body)) => {
                    if !body.is_empty() {
                        if !out.is_empty() {
                            out.push('\n');
                        }
                        out.push_str(&body);
                    }
                    // El primer paquete que ya trae el JSON basta.
                    if extract_json(&out).is_some() {
                        break;
                    }
                }
                None => break,
            }
            if i + 1 == max_packets {
                break;
            }
        }
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// Envia un comando y reintenta una vez si la respuesta llega vacia.
    ///
    /// Factorio rechaza el PRIMER comando de consola de cada sesion con el
    /// aviso "Using Lua console commands will disable achievements. Please
    /// repeat the command to proceed": la respuesta llega vacia y el comando NO
    /// se ejecuta. Hay que repetirlo con el mismo texto. Verificado contra el
    /// servidor real en scripts/local_rcon_check.sh.
    pub fn execute_collect_retry(&mut self, command: &str, max_packets: usize) -> Option<String> {
        let first = self.execute_collect(command, max_packets);
        if let Some(ref body) = first {
            if !body.trim().is_empty() {
                return first;
            }
        }
        self.execute_collect(command, max_packets)
    }

    /// Envia un comando y extrae el JSON de la respuesta.
    pub fn execute_json(&mut self, command: &str) -> Option<serde_json::Value> {
        let body = self.execute_collect(command, 3)?;
        extract_json(&body)
    }

    /// Envia un comando y extrae un entero de la respuesta.
    pub fn execute_number(&mut self, command: &str) -> Option<i64> {
        let body = self.execute_collect(command, 3)?;
        parse_leading_number(&body)
    }
}

/// Extrae el primer JSON valido de un texto que puede traer ruido alrededor.
pub fn extract_json(text: &str) -> Option<serde_json::Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        return Some(v);
    }
    let bytes = trimmed.as_bytes();
    for start in 0..bytes.len() {
        if bytes[start] != b'{' && bytes[start] != b'[' {
            continue;
        }
        if let Some(end) = find_matching_end(bytes, start) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed[start..=end]) {
                return Some(v);
            }
        }
    }
    None
}

fn find_matching_end(bytes: &[u8], start: usize) -> Option<usize> {
    let open = bytes[start];
    let close = if open == b'{' { b'}' } else { b']' };
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for i in start..bytes.len() {
        let c = bytes[i];
        if in_string {
            if escaped {
                escaped = false;
            } else if c == b'\\' {
                escaped = true;
            } else if c == b'"' {
                in_string = false;
            }
            continue;
        }
        if c == b'"' {
            in_string = true;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Extrae el primer entero de un texto (para respuestas como "1" o "OK 1").
fn parse_leading_number(text: &str) -> Option<i64> {
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        let mut end = 0;
        let bytes = t.as_bytes();
        if bytes[0] == b'-' {
            end = 1;
        }
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end > 0 && (end == 1 && bytes[0] == b'-' || t.as_bytes()[end - (end > 0) as usize].is_ascii_digit()) {
            if let Ok(v) = t[..end].parse::<i64>() {
                return Some(v);
            }
        }
    }
    None
}

// --- Estado global del puente RCON -----------------------------------------

pub struct RconConfig {
    pub host: String,
    pub port: u16,
    pub password: String,
    pub poll_timer: f32,
    pub connect_timer: f32,
    /// Cuando el servidor no devuelve la carga con /silent-command, se reintenta
    /// con el comando normal (que imprime el valor de retorno en la consola).
    pub verbose_fallback: bool,
    pub last_error: Option<String>,
}

static RCON_CONFIG: OnceLock<Mutex<RconConfig>> = OnceLock::new();

pub fn get_rcon_config() -> &'static Mutex<RconConfig> {
    RCON_CONFIG.get_or_init(|| {
        Mutex::new(RconConfig {
            host: RCON_DEFAULT_HOST.to_string(),
            port: RCON_DEFAULT_PORT,
            password: RCON_PASSWORD.to_string(),
            poll_timer: 0.0,
            connect_timer: RCON_RECONNECT_INTERVAL,
            verbose_fallback: false,
            last_error: None,
        })
    })
}

/// Resultado de un sondeo RCON.
pub struct RconPoll {
    pub connected: bool,
    /// JSON crudo devuelto por el mod (instantanea del mundo).
    pub snapshot: Option<String>,
    /// JSON crudo del inventario del jugador.
    pub inventory: Option<String>,
}

/// Lee el host de RCON configurado en el visor (si existe).
fn read_host_override() -> Option<String> {
    #[cfg(target_os = "android")]
    {
        std::fs::read_to_string(RCON_HOST_FILE)
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = RCON_HOST_FILE;
        None
    }
}

/// Sondea el servidor Factorio. La E/S de red ocurre SIN el lock del mundo
/// para no congelar el renderizado de la mesa holografica.
pub fn poll_rcon_sync(dt: f32, view_center: (f32, f32)) -> RconPoll {
    let (host, port, password, do_poll, do_connect, verbose) = {
        let mut cfg = match get_rcon_config().lock() {
            Ok(c) => c,
            Err(_) => {
                return RconPoll {
                    connected: false,
                    snapshot: None,
                    inventory: None,
                }
            }
        };
        cfg.poll_timer += dt;
        cfg.connect_timer += dt;

        let do_poll = cfg.poll_timer >= RCON_POLL_INTERVAL;
        if do_poll {
            cfg.poll_timer = 0.0;
        }
        let do_connect = cfg.connect_timer >= RCON_RECONNECT_INTERVAL;
        if do_connect {
            cfg.connect_timer = 0.0;
        }
        (
            cfg.host.clone(),
            cfg.port,
            cfg.password.clone(),
            do_poll,
            do_connect,
            cfg.verbose_fallback,
        )
    };

    let mut rcon = match get_rcon_client().lock() {
        Ok(r) => r,
        Err(_) => {
            return RconPoll {
                connected: false,
                snapshot: None,
                inventory: None,
            }
        }
    };

    // Intento de conexion.
    if do_connect || !rcon.is_connected() {
        if !rcon.is_connected() {
            let target_host = read_host_override().unwrap_or(host);
            let addr = format!("{}:{}", target_host, port);
            if rcon.connect(&addr, &password) {
                if let Ok(mut cfg) = get_rcon_config().lock() {
                    cfg.last_error = None;
                }
            } else {
                rcon.disconnect();
                if let Ok(mut cfg) = get_rcon_config().lock() {
                    cfg.last_error = Some(format!("no se pudo conectar a {}", addr));
                }
                return RconPoll {
                    connected: false,
                    snapshot: None,
                    inventory: None,
                };
            }
        }
    }

    if !do_poll {
        return RconPoll {
            connected: rcon.is_connected(),
            snapshot: None,
            inventory: None,
        };
    }

    // Pedir la instantanea del mundo alrededor del centro de la vista.
    let prefix = if verbose { "" } else { "/silent-command " };
    let snap_cmd = format!(
        "{}remote.call('xfactorvr', 'get_world_snapshot', {}, {:.1}, {:.1})",
        prefix, RCON_SNAPSHOT_RADIUS, view_center.0, view_center.1
    );
    let inv_cmd = format!("{}remote.call('xfactorvr', 'get_inventory')", prefix);

    // Factorio rechaza el primer comando de consola de la sesion (ver
    // execute_collect_retry), asi que ambos comandos se lanzan con reintento.
    let snapshot = rcon.execute_collect_retry(&snap_cmd, 3);
    let inventory = rcon.execute_collect_retry(&inv_cmd, 2);

    // Si el comando silencioso no trae carga util, se prueba con el comando
    // normal, que ademas imprime el valor de retorno en la consola.
    let mut snapshot = snapshot;
    let mut inventory = inventory;
    if !verbose && (extract_json(snapshot.as_deref().unwrap_or("")).is_none()) {
        if let Ok(mut cfg) = get_rcon_config().lock() {
            cfg.verbose_fallback = true;
        }
        let snap_cmd = format!(
            "remote.call('xfactorvr', 'get_world_snapshot', {}, {:.1}, {:.1})",
            RCON_SNAPSHOT_RADIUS, view_center.0, view_center.1
        );
        let inv_cmd = "remote.call('xfactorvr', 'get_inventory')";
        snapshot = rcon.execute_collect(&snap_cmd, 3);
        inventory = rcon.execute_collect(&inv_cmd, 2);
    }

    RconPoll {
        connected: rcon.is_connected(),
        snapshot,
        inventory,
    }
}

pub static SIMULATION_WORLD: OnceLock<RwLock<SimulationWorld>> = OnceLock::new();

pub fn get_simulation_world() -> &'static RwLock<SimulationWorld> {
    SIMULATION_WORLD.get_or_init(|| RwLock::new(SimulationWorld::new()))
}

pub static RCON_CLIENT: OnceLock<Mutex<RconClient>> = OnceLock::new();

pub fn get_rcon_client() -> &'static Mutex<RconClient> {
    RCON_CLIENT.get_or_init(|| Mutex::new(RconClient::new()))
}

// --- Formato de la instantanea que devuelve el mod -------------------------

/// Campo "t" de la instantanea. El mod envia el NOMBRE del prototipo
/// ("assembling-machine-1"), que es lo unico que permite elegir el sprite
/// real del juego; se acepta tambien el numero por compatibilidad con
/// despliegues antiguos del mod.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum SnapshotType {
    Name(String),
    Id(u32),
}

#[derive(Debug, Deserialize)]
pub struct SnapshotEntity {
    #[serde(rename = "t")]
    pub entity_type: SnapshotType,
    pub x: f32,
    pub y: f32,
    #[serde(rename = "o", default)]
    pub orientation: f32,
    #[serde(rename = "s", default)]
    pub status: u32,
    #[serde(rename = "p", default)]
    pub progress: f32,
    #[serde(rename = "h", default)]
    pub health: f32,
    #[serde(rename = "a", default)]
    pub arm_angle: f32,
    /// Frame de animacion (lo que usa el motor para elegir el frame del
    /// sprite). 0 si la entidad no se anima.
    #[serde(rename = "f", default)]
    pub anim_frame: u32,
    /// Direccion de Factorio (0..15). La orientacion en radianes ya viene
    /// en "o", pero la direccion cruda es la que indexa las filas del sprite.
    #[serde(rename = "d", default)]
    pub direction: u8,
}

/// Traduce el nombre del prototipo de Factorio al enumerado de entidad que
/// usa el render. Debe coincidir con la tabla ENTITY_TYPES_MAP del mod
/// (mods/xfactorvr_bridge/control.lua).
pub fn entity_type_from_name(name: &str) -> u32 {
    use crate::tabletop::*;
    match name {
        "assembling-machine-1" | "assembling-machine-2" | "assembling-machine-3" => {
            ENTITY_ASSEMBLING_MACHINE
        }
        "electric-mining-drill" | "burner-mining-drill" | "offshore-mining-drill" => {
            ENTITY_MINING_DRILL
        }
        "stone-furnace" | "steel-furnace" | "electric-furnace" | "campfire" => ENTITY_FURNACE,
        "gun-turret" | "laser-turret" => ENTITY_GUN_TURRET,
        "radar" => ENTITY_RADAR,
        "roboport" => ENTITY_ROBOPORT,
        "wooden-chest" | "iron-chest" | "steel-chest" | "transport-chest" => ENTITY_CHEST,
        "small-electric-pole" | "medium-electric-pole" | "big-electric-pole" | "substation" => {
            ENTITY_ELECTRIC_POLE
        }
        "transport-belt" | "fast-transport-belt" | "express-transport-belt" | "underground-belt"
        | "splitter" => ENTITY_TRANSPORT_BELT,
        "inserter" | "fast-inserter" | "long-handed-inserter" | "stack-inserter"
        | "stack-inserter-1" => ENTITY_INSERTER,
        "tree" | "simple-entity" => ENTITY_TREE,
        "unit" | "unit-spawner" | "small-biter" | "medium-biter" | "big-biter"
        | "small-spitter" | "medium-spitter" | "big-spitter" => ENTITY_BITER,
        "character" | "player" => ENTITY_PLAYER,
        _ => 0,
    }
}

#[derive(Debug, Deserialize)]
pub struct SnapshotPatch {
    #[serde(rename = "t")]
    pub item_type: u32,
    pub x: f32,
    pub y: f32,
    #[serde(rename = "r", default = "default_patch_radius")]
    pub radius: f32,
    #[serde(rename = "rem", default)]
    pub remaining: u32,
}

fn default_patch_radius() -> f32 {
    2.4
}

#[derive(Debug, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub entities: Vec<SnapshotEntity>,
    #[serde(default)]
    pub patches: Vec<SnapshotPatch>,
    /// Terreno real del mapa (hierba, tierra, arena, agua...).
    #[serde(default)]
    pub terrain: TerrainSnapshot,
}

/// Terreno del area visible.
///
/// El mod manda el tile DOMINANTE del area ("base") y solo los que difieren
/// de ese, en coordenadas relativas al origen. Es compacto y exacto.
#[derive(Debug, Default, Clone, Deserialize)]
pub struct TerrainSnapshot {
    /// Indice (en `names`) del tile dominante.
    #[serde(default)]
    pub base: u32,
    /// Nombres de los tiles (prototipos: "grass", "dirt", "water"...).
    #[serde(default)]
    pub names: Vec<String>,
    /// Casilla origen del area.
    #[serde(default)]
    pub x0: i32,
    #[serde(default)]
    pub y0: i32,
    /// Tamano del area en casillas.
    #[serde(default)]
    pub w: u32,
    #[serde(default)]
    pub h: u32,
    /// Celdas que difieren del dominante: [x_rel, y_rel, indice_de_nombre].
    #[serde(default)]
    pub tiles: Vec<[u32; 3]>,
}

impl TerrainSnapshot {
    /// Nombre del terreno en una casilla del area (o None si esta fuera).
    pub fn tile_at(&self, rel_x: i32, rel_y: i32) -> Option<&str> {
        if rel_x < 0 || rel_y < 0 || rel_x >= self.w as i32 || rel_y >= self.h as i32 {
            return None;
        }
        for t in &self.tiles {
            if t[0] as i32 == rel_x && t[1] as i32 == rel_y {
                return self.names.get(t[2] as usize).map(|s| s.as_str());
            }
        }
        self.names.get(self.base as usize).map(|s| s.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.names.is_empty() || self.w == 0 || self.h == 0
    }
}

#[derive(Debug, Deserialize)]
pub struct InventoryItem {
    pub id: u32,
    pub count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(t: u32, x: f32, y: f32, o: f32) -> LiveEntity {
        LiveEntity {
            entity_type: t,
            tile_x: x,
            tile_y: y,
            orientation: o,
            status: 1,
            animation_progress: 0.0,
            health_ratio: 1.0,
            arm_angle: 0.0,
            sprite_id: 0,
            anim_frame: 0,
        }
    }

    #[test]
    fn test_simulation_world_mechanics() {
        let mut world = SimulationWorld::new();
        // Verificar inventario inicial autentico
        assert_eq!(*world.player_inventory.get(&ITEM_IRON_PLATE).unwrap(), 8);
        assert_eq!(*world.player_inventory.get(&ITEM_MINING_DRILL).unwrap(), 1);
        assert_eq!(*world.player_inventory.get(&ITEM_FURNACE).unwrap(), 1);

        // 1. Probar minado manual con pico en yacimiento de piedra (-6.0, -4.0)
        let mined_stone = world.manual_mine(-6.0, -4.0);
        assert_eq!(mined_stone, ITEM_STONE, "Debe haber extraido piedra");
        assert_eq!(*world.player_inventory.get(&ITEM_STONE).unwrap(), 1);

        // Minar 4 piedras mas para alcanzar 5 de piedra
        for _ in 0..4 {
            world.manual_mine(-6.0, -4.0);
        }
        assert_eq!(*world.player_inventory.get(&ITEM_STONE).unwrap(), 5);

        // 2. Probar minera colocada sobre yacimiento de hierro en (-6.0, 4.0)
        // y horno de piedra colocado justo al lado para recibir el mineral
        let mut entities = vec![
            entity(crate::tabletop::ENTITY_MINING_DRILL, -6.0, 4.0, 0.0),
            entity(crate::tabletop::ENTITY_FURNACE, -6.0, 3.0, 0.0),
        ];

        // Suministrar carbon inicial al horno
        world
            .machine_states
            .entry((-6, 3))
            .or_default()
            .fuel_remaining = 100.0;

        // Simular 4 segundos
        world.tick(&mut entities, 4.0);

        assert_eq!(entities[0].status, 1, "La minera debe estar trabajando");
        assert_eq!(entities[1].status, 1, "El horno debe estar ardiendo y fundiendo mineral");

        // Simular 4 segundos adicionales para completar la fundicion de planchas
        world.tick(&mut entities, 4.0);

        let out = *world
            .machine_states
            .get(&(-6, 3))
            .unwrap()
            .output_buffer
            .get(&ITEM_IRON_PLATE)
            .unwrap_or(&0);
        assert!(out >= 1, "El horno debe haber producido al menos 1 plancha de hierro");
    }

    #[test]
    fn test_belt_and_inserter_automation_chain() {
        // Cofre -> Insertador -> Ensambladora.
        // El insertador en (0,1) mira al sur (orientacion = PI): recoge del
        // cofre que tiene atras (0,0) y deja en la ensambladora de delante (0,2).
        let mut world = SimulationWorld::new();
        let mut entities = vec![
            entity(crate::tabletop::ENTITY_CHEST, 0.0, 0.0, 0.0),
            entity(crate::tabletop::ENTITY_INSERTER, 0.0, 1.0, std::f32::consts::PI),
            entity(crate::tabletop::ENTITY_ASSEMBLING_MACHINE, 0.0, 2.0, 0.0),
        ];

        // El cofre tiene planchas de hierro listas para llevar.
        world
            .machine_states
            .entry((0, 0))
            .or_default()
            .output_buffer
            .insert(ITEM_IRON_PLATE, 4);

        // La ensambladora produce engranajes (2 planchas -> 1 engranaje).
        world.machine_states.entry((0, 2)).or_default().recipe_id = 1;

        // Simular 10 segundos: el insertador deberia mover las planchas y la
        // ensambladora fabricar engranajes sin intervencion del jugador.
        world.tick(&mut entities, 10.0);

        let assembler = world.machine_states.get(&(0, 2)).unwrap();
        let gears = *assembler.output_buffer.get(&ITEM_IRON_GEAR).unwrap_or(&0);
        assert!(
            gears >= 1,
            "La ensambladora deberia fabricar engranajes, produjo {}",
            gears
        );
    }

    #[test]
    fn test_drill_feeds_belt_to_machine() {
        // Perforadora -> Cinta -> Ensambladora: el mineral debe llegar solo.
        let mut world = SimulationWorld::new();
        // La veta de hierro esta en (-6, 4). La perforadora en (-6, 4) mirando
        // al sur entrega en (-6, 5), donde esta la cinta, que mira al sur y
        // entrega en (-6, 6), donde esta la ensambladora.
        let mut entities = vec![
            entity(crate::tabletop::ENTITY_MINING_DRILL, -6.0, 4.0, std::f32::consts::PI),
            entity(crate::tabletop::ENTITY_TRANSPORT_BELT, -6.0, 5.0, std::f32::consts::PI),
            entity(crate::tabletop::ENTITY_ASSEMBLING_MACHINE, -6.0, 6.0, 0.0),
        ];

        world.tick(&mut entities, 12.0);

        let assembler = world.machine_states.get(&(-6, 6)).unwrap();
        let ore = *assembler.input_buffer.get(&ITEM_IRON_ORE).unwrap_or(&0);
        assert!(ore >= 1, "La cinta deberia llevar mineral a la ensambladora, llego {}", ore);
    }

    #[test]
    fn test_terreno_de_la_instantanea() {
        // Formato que manda el mod: tile dominante + excepciones.
        let json = r#"{"entities":[],"patches":[],
            "terrain":{"base":0,"names":["grass","dirt","water"],"x0":-4,"y0":-4,"w":9,"h":9,
                       "tiles":[[0,0,1],[1,0,2],[2,2,1]]}}"#;
        let snap: Snapshot = serde_json::from_str(json).unwrap();
        let t = &snap.terrain;
        assert_eq!(t.names, vec!["grass", "dirt", "water"]);
        assert_eq!(t.w, 9);
        // El dominante se aplica a todo lo demas.
        assert_eq!(t.tile_at(5, 5), Some("grass"));
        // Las excepciones pisan al dominante.
        assert_eq!(t.tile_at(0, 0), Some("dirt"));
        assert_eq!(t.tile_at(1, 0), Some("water"));
        assert_eq!(t.tile_at(2, 2), Some("dirt"));
        // Fuera del area no hay terreno.
        assert_eq!(t.tile_at(-1, 0), None);
        assert_eq!(t.tile_at(9, 0), None);
    }

    #[test]
    fn test_instantanea_sin_terreno_no_rompe() {
        // Un mod antiguo (sin terreno) debe seguir siendo valido.
        let json = r#"{"entities":[{"t":"tree","x":1.0,"y":2.0}],"patches":[]}"#;
        let snap: Snapshot = serde_json::from_str(json).unwrap();
        assert!(snap.terrain.is_empty());
        assert_eq!(snap.entities.len(), 1);
    }

    #[test]
    fn test_snapshot_parsing() {
        let json = r#"{"entities":[{"t":"assembling-machine-1","x":1.5,"y":2.5,"o":1.571,"s":1,"p":0.25,"h":1.0,"f":7,"d":4}],
                      "patches":[{"t":1,"x":0.0,"y":0.0,"r":2.4,"rem":1000}]}"#;
        let snap: Snapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snap.entities.len(), 1);
        match &snap.entities[0].entity_type {
            SnapshotType::Name(n) => assert_eq!(n, "assembling-machine-1"),
            SnapshotType::Id(i) => panic!("se esperaba un nombre de prototipo, se obtuvo id {}", i),
        }
        assert_eq!(snap.entities[0].anim_frame, 7);
        assert_eq!(snap.entities[0].direction, 4);
        assert!((snap.entities[0].orientation - 1.571).abs() < 0.01);
        assert_eq!(snap.patches.len(), 1);
        assert_eq!(snap.patches[0].remaining, 1000);
    }

    #[test]
    fn test_extract_json_from_noisy_output() {
        // La respuesta de RCON puede venir con texto antes o despues del JSON.
        let text = "some log line\n{\"entities\":[],\"patches\":[]}\ntrailing";
        let v = extract_json(text).expect("deberia extraer el JSON");
        assert!(v.get("entities").is_some());
    }

    #[test]
    fn test_parse_leading_number() {
        assert_eq!(parse_leading_number("1"), Some(1));
        assert_eq!(parse_leading_number("OK\n3"), Some(3));
        assert_eq!(parse_leading_number("ERR|no_entity"), None);
    }

    #[test]
    fn test_facing_delta_matches_factorio_directions() {
        // Factorio 2.0: 16 direcciones, este = 4 -> PI/2 radianes.
        assert_eq!(facing_delta(0.0), (0, -1), "norte");
        assert_eq!(facing_delta(std::f32::consts::FRAC_PI_2), (1, 0), "este");
        assert_eq!(facing_delta(std::f32::consts::PI), (0, 1), "sur");
        assert_eq!(facing_delta(3.0 * std::f32::consts::FRAC_PI_2), (-1, 0), "oeste");
    }
}
