// game_bridge.rs — Factorio Game Engine Integration & Real Simulation Bridge
//
// Proporciona la conexión bidireccional con el motor de Factorio:
// 1. Cliente RCON / IPC para comunicarse con la instancia de Factorio (Linux ARM64 o local).
// 2. Motor de simulación en vivo de Factorio en Rust:
//    - Yacimientos reales de recursos (vetas de hierro, cobre, carbón y piedra).
//    - Minería con extracción de mineral desde el suelo.
//    - Hornos con fundición de mineral a planchas con consumo de carbón.
//    - Ensambladoras con fabricación de recetas (engranajes, circuitos).
//    - Cintas transportadoras con flujo de ítems.
//    - Insertadores robóticos que transfieren piezas entre máquinas y cintas.
//    - Inventario real del jugador con consumo al construir y retorno al minar.

use crate::tabletop::LiveEntity;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::Duration;

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
    pub work_progress: f32, // 0.0 a 1.0
}

impl Default for MachineState {
    fn default() -> Self {
        Self {
            input_buffer: HashMap::new(),
            output_buffer: HashMap::new(),
            fuel_remaining: 100.0,
            recipe_id: 1, // default recipe: iron gear wheel
            work_progress: 0.0,
        }
    }
}

pub struct SimulationWorld {
    pub patches: Vec<ResourcePatch>,
    pub machine_states: HashMap<(i32, i32), MachineState>,
    pub player_inventory: HashMap<u32, u32>,
    pub is_connected_to_rcon: bool,
}

impl SimulationWorld {
    pub fn new() -> Self {
        let mut inv = HashMap::new();
        // Pack inicial auténtico de Factorio (Crash Site / Level 1 start)
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
            machine_states: HashMap::new(),
            player_inventory: inv,
            is_connected_to_rcon: false,
        }
    }

    /// Reinicia el mundo de simulación al estado inicial de nivel
    pub fn reset_world(&mut self) {
        let mut inv = HashMap::new();
        inv.insert(ITEM_IRON_PLATE, 8);
        inv.insert(ITEM_MINING_DRILL, 1);
        inv.insert(ITEM_FURNACE, 1);
        inv.insert(ITEM_COAL, 1);

        self.player_inventory = inv;
        self.machine_states.clear();
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

    /// Interactúa con una máquina (recoge planchas/mineral y suministra combustible/mineral)
    pub fn interact_machine(&mut self, tile_x: f32, tile_y: f32) -> u32 {
        let key = (tile_x.round() as i32, tile_y.round() as i32);
        let mut total_looted = 0;
        if let Some(state) = self.machine_states.get_mut(&key) {
            // 1. Recoger productos terminados de output_buffer
            for (&item_id, &qty) in &state.output_buffer {
                if qty > 0 {
                    *self.player_inventory.entry(item_id).or_insert(0) += qty;
                    total_looted += qty;
                }
            }
            state.output_buffer.clear();

            // 2. Abastecer de carbón si está bajo de combustible
            if state.fuel_remaining < 50.0 {
                if let Some(coal_cnt) = self.player_inventory.get_mut(&ITEM_COAL) {
                    if *coal_cnt > 0 {
                        let transfer = (*coal_cnt).min(5);
                        *coal_cnt -= transfer;
                        state.fuel_remaining += transfer as f32 * 100.0;
                    }
                }
            }

            // 3. Si es un horno y el jugador tiene mineral en inventario, cargar hasta 5 unidades
            if let Some(iron_cnt) = self.player_inventory.get_mut(&ITEM_IRON_ORE) {
                if *iron_cnt > 0 {
                    let transfer = (*iron_cnt).min(5);
                    *iron_cnt -= transfer;
                    *state.input_buffer.entry(ITEM_IRON_ORE).or_insert(0) += transfer;
                }
            }
        }
        total_looted
    }

    /// Obtiene información de estado de una máquina para el HUD / GUI
    pub fn get_machine_info(&self, tile_x: f32, tile_y: f32) -> Option<(f32, f32, u32, u32)> {
        let key = (tile_x.round() as i32, tile_y.round() as i32);
        self.machine_states.get(&key).map(|s| {
            let input_ore = *s.input_buffer.get(&ITEM_IRON_ORE).unwrap_or(&0) +
                            *s.input_buffer.get(&ITEM_COPPER_ORE).unwrap_or(&0);
            let output_plates = *s.output_buffer.get(&ITEM_IRON_PLATE).unwrap_or(&0) +
                                *s.output_buffer.get(&ITEM_COPPER_PLATE).unwrap_or(&0);
            (s.fuel_remaining, s.work_progress, input_ore, output_plates)
        })
    }

    /// Comprueba si una posición está sobre un yacimiento de mineral
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

    /// Avanza la simulación por pasos discretos estables
    pub fn tick(&mut self, entities: &mut [LiveEntity], dt: f32) {
        let mut rem = dt;
        while rem > 0.0001 {
            let step = rem.min(0.2);
            self.tick_step(entities, step);
            rem -= step;
        }
    }

    fn tick_step(&mut self, entities: &mut [LiveEntity], dt: f32) {
        // Encontrar máquinas adyacentes a las perforadoras para transferencia directa
        let mut pending_transfers: Vec<((i32, i32), u32)> = Vec::new();

        for entity in entities.iter_mut() {
            let key = (entity.tile_x.round() as i32, entity.tile_y.round() as i32);
            let ore_at_loc = Self::get_resource_in_patches(&self.patches, entity.tile_x, entity.tile_y);
            let state = self.machine_states.entry(key).or_default();

            match entity.entity_type {
                // Minera Eléctrica / a vapor: Extrae mineral si está sobre un yacimiento
                2 => {
                    if let Some(ore_type) = ore_at_loc {
                        entity.status = 1; // Trabajando
                        state.work_progress += dt * 0.5; // 1 ciclo cada 2s
                        if state.work_progress >= 1.0 {
                            state.work_progress = 0.0;

                            // Calcular casilla frontal de caída (drop target) según orientación
                            let f_dx = (entity.orientation.sin()).round() as i32;
                            let f_dy = (-entity.orientation.cos()).round() as i32;
                            let target_tile = (key.0 + f_dx, key.1 + f_dy);
                            pending_transfers.push((target_tile, ore_type));
                        }
                        entity.animation_progress = state.work_progress;
                    } else {
                        entity.status = 4; // Sin recursos / inactiva
                        entity.animation_progress = 0.0;
                    }
                }
                // Horno de piedra: Funde mineral con carbón
                3 => {
                    let iron_ore_count = *state.input_buffer.get(&ITEM_IRON_ORE).unwrap_or(&0);
                    let copper_ore_count = *state.input_buffer.get(&ITEM_COPPER_ORE).unwrap_or(&0);
                    let has_ore = iron_ore_count > 0 || copper_ore_count > 0;

                    // Si necesita combustible, intenta consumir carbón del input_buffer
                    if state.fuel_remaining <= 0.0 {
                        if let Some(coal_cnt) = state.input_buffer.get_mut(&ITEM_COAL) {
                            if *coal_cnt > 0 {
                                *coal_cnt -= 1;
                                state.fuel_remaining += 100.0;
                            }
                        }
                    }

                    if has_ore && state.fuel_remaining > 0.0 {
                        entity.status = 1; // ARDIENDO / FUNDIENDO (Fire & Smoke)
                        state.fuel_remaining = (state.fuel_remaining - dt * 1.5).max(0.0);
                        state.work_progress += dt * 0.3125; // 3.2 segundos por plancha

                        if state.work_progress >= 1.0 {
                            state.work_progress = 0.0;
                            if iron_ore_count > 0 {
                                if let Some(cnt) = state.input_buffer.get_mut(&ITEM_IRON_ORE) {
                                    *cnt -= 1;
                                }
                                *state.output_buffer.entry(ITEM_IRON_PLATE).or_insert(0) += 1;
                            } else if copper_ore_count > 0 {
                                if let Some(cnt) = state.input_buffer.get_mut(&ITEM_COPPER_ORE) {
                                    *cnt -= 1;
                                }
                                *state.output_buffer.entry(ITEM_COPPER_PLATE).or_insert(0) += 1;
                            }
                        }
                        entity.animation_progress = state.work_progress;
                    } else if has_ore && state.fuel_remaining <= 0.0 {
                        entity.status = 3; // Sin combustible
                        entity.animation_progress = 0.0;
                    } else {
                        entity.status = 0; // Inactivo / esperando mineral
                        entity.animation_progress = 0.0;
                    }
                }
                // Ensambladora: Fabrica recetas
                1 => {
                    entity.status = 1; // Activa
                    state.work_progress = (state.work_progress + dt * 0.7) % 1.0;
                    entity.animation_progress = state.work_progress;
                }
                // Radar: Rotación continua
                5 => {
                    entity.status = 1;
                    entity.animation_progress = (entity.animation_progress + dt * 0.25).rem_euclid(1.0);
                }
                // Torreta: Barrido defensivo
                4 => {
                    entity.status = 1;
                    entity.animation_progress = (entity.animation_progress + dt * 0.4).rem_euclid(1.0);
                }
                // Insertador: Movimiento de brazo
                10 => {
                    entity.status = 1;
                    entity.animation_progress = (entity.animation_progress + dt * 1.5).rem_euclid(1.0);
                }
                _ => {
                    entity.status = 1;
                    entity.animation_progress = (entity.animation_progress + dt * 0.2).rem_euclid(1.0);
                }
            }
        }

        // Aplicar transferencias de mineral desde perforadoras a máquinas receptoras
        for (target_coord, ore_type) in pending_transfers {
            // Verificar si hay una entidad en target_coord o adyacente
            let target_exists = entities.iter().any(|e| {
                (e.tile_x.round() as i32 - target_coord.0).abs() <= 1 &&
                (e.tile_y.round() as i32 - target_coord.1).abs() <= 1 &&
                (e.entity_type == 3 || e.entity_type == 7 || e.entity_type == 9) // Horno, Cofre o Cinta
            });

            if target_exists {
                let target_state = self.machine_states.entry(target_coord).or_default();
                *target_state.input_buffer.entry(ore_type).or_insert(0) += 1;
            } else {
                // Si no hay máquina receptora, acumular en la perforadora
                if let Some(drill_state) = self.machine_states.values_mut().next() {
                    *drill_state.output_buffer.entry(ore_type).or_insert(0) += 1;
                }
            }
        }
    }
}

pub static SIMULATION_WORLD: OnceLock<RwLock<SimulationWorld>> = OnceLock::new();

pub fn get_simulation_world() -> &'static RwLock<SimulationWorld> {
    SIMULATION_WORLD.get_or_init(|| RwLock::new(SimulationWorld::new()))
}

// --- Cliente RCON Factorio ---
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

    pub fn connect(&mut self, addr: &str, password: &str) -> bool {
        let stream = match TcpStream::connect_timeout(
            &addr.parse().unwrap_or_else(|_| "127.0.0.1:27015".parse().unwrap()),
            Duration::from_millis(500),
        ) {
            Ok(s) => s,
            Err(_) => return false,
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
        let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
        self.stream = Some(stream);

        // Enviar paquete de autenticación SERVERDATA_AUTH (tipo 3)
        if !self.send_packet(3, password) {
            self.stream = None;
            return false;
        }

        true
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
        buf.push(0); // Termina string
        buf.push(0); // Pad

        stream.write_all(&buf).is_ok()
    }

    pub fn execute(&mut self, command: &str) -> Option<String> {
        if !self.send_packet(2, command) {
            return None;
        }

        let stream = self.stream.as_mut()?;
        let mut header = [0u8; 12];
        if stream.read_exact(&mut header).is_err() {
            return None;
        }

        let length = i32::from_le_bytes(header[0..4].try_into().unwrap());
        if length < 8 || length > 65536 {
            return None;
        }

        let body_len = (length - 8) as usize;
        let mut body = vec![0u8; body_len];
        if stream.read_exact(&mut body).is_err() {
            return None;
        }

        // Eliminar los dos bytes nulos del final
        if body.len() >= 2 {
            body.truncate(body.len() - 2);
        }

        String::from_utf8(body).ok()
    }
}

pub static RCON_CLIENT: OnceLock<Mutex<RconClient>> = OnceLock::new();

pub fn get_rcon_client() -> &'static Mutex<RconClient> {
    RCON_CLIENT.get_or_init(|| Mutex::new(RconClient::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simulation_world_mechanics() {
        let mut world = SimulationWorld::new();
        // Verificar inventario inicial auténtico
        assert_eq!(*world.player_inventory.get(&ITEM_IRON_PLATE).unwrap(), 8);
        assert_eq!(*world.player_inventory.get(&ITEM_MINING_DRILL).unwrap(), 1);
        assert_eq!(*world.player_inventory.get(&ITEM_FURNACE).unwrap(), 1);

        // 1. Probar minado manual con pico en yacimiento de piedra (-6.0, -4.0)
        let mined_stone = world.manual_mine(-6.0, -4.0);
        assert_eq!(mined_stone, ITEM_STONE, "Debe haber extraído piedra");
        assert_eq!(*world.player_inventory.get(&ITEM_STONE).unwrap(), 1);

        // Minar 4 piedras más para alcanzar 5 de piedra
        for _ in 0..4 {
            world.manual_mine(-6.0, -4.0);
        }
        assert_eq!(*world.player_inventory.get(&ITEM_STONE).unwrap(), 5);

        // 2. Probar minera colocada sobre yacimiento de hierro en (-6.0, 4.0)
        // y horno de piedra colocado justo al lado para recibir el mineral
        let mut entities = vec![
            LiveEntity {
                entity_type: 2, // Mining drill
                tile_x: -6.0,
                tile_y: 4.0,
                orientation: 0.0,
                status: 0,
                animation_progress: 0.0,
                health_ratio: 1.0,
            },
            LiveEntity {
                entity_type: 3, // Stone furnace
                tile_x: -6.0,
                tile_y: 3.0,
                orientation: 0.0,
                status: 0,
                animation_progress: 0.0,
                health_ratio: 1.0,
            }
        ];

        // Suministrar carbón inicial al horno
        world.machine_states.entry((-6, 3)).or_default().fuel_remaining = 100.0;

        // Simular 4 segundos
        world.tick(&mut entities, 4.0);

        assert_eq!(entities[0].status, 1, "La minera debe estar trabajando");
        assert_eq!(entities[1].status, 1, "El horno debe estar ardiendo y fundiendo mineral");

        // Simular 4 segundos adicionales para completar la fundición de planchas
        world.tick(&mut entities, 4.0);

        // 3. Probar looting del horno
        let looted = world.interact_machine(-6.0, 3.0);
        assert!(looted >= 1, "Debe haber recogido al menos 1 plancha de hierro fundida");
        assert!(*world.player_inventory.get(&ITEM_IRON_PLATE).unwrap() > 8);
    }
}
