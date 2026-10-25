// lib.rs — Punto de entrada para XFactorVR (libxfactorvr.so)
//
// Inyectado en Factorio vía LD_PRELOAD para proporcionar la experiencia
// Tabletop Mixed Reality en Pico 4 Ultra Standalone / OpenXR.

pub mod assets;
pub mod game_bridge;
pub mod modelos3d;
pub mod offsets;
pub mod openxr;
pub mod renderer;
pub mod symbols;
pub mod tabletop;
pub mod terreno;
// El HUD de VR: tabla de controles unica, ventanas espaciales y estados.
// Ver src/ui/README.md para la API.
pub mod ui;
mod gnu_unwind;

use anyhow::Result;
use std::sync::{OnceLock, RwLock};

static TABLETOP: OnceLock<tabletop::TabletopManager> = OnceLock::new();
static LIVE_ENTITIES: OnceLock<RwLock<Vec<tabletop::LiveEntity>>> = OnceLock::new();

pub fn get_live_entities_lock() -> &'static RwLock<Vec<tabletop::LiveEntity>> {
    LIVE_ENTITIES.get_or_init(|| {
        RwLock::new(tabletop::init_default_factory_entities())
    })
}

#[cfg(target_os = "android")]
struct AndroidLogger;

#[cfg(target_os = "android")]
impl log::Log for AndroidLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            use std::ffi::CString;
            let tag = CString::new("libxfactorvr").unwrap_or_default();
            let msg = CString::new(format!("{}", record.args()))
                .unwrap_or_else(|_| CString::new("<invalid log string>").unwrap_or_default());
            let priority = match record.level() {
                log::Level::Error => 6, // ANDROID_LOG_ERROR
                log::Level::Warn => 5,  // ANDROID_LOG_WARN
                log::Level::Info => 4,  // ANDROID_LOG_INFO
                log::Level::Debug => 3, // ANDROID_LOG_DEBUG
                log::Level::Trace => 2, // ANDROID_LOG_VERBOSE
            };
            unsafe {
                __android_log_print(priority, tag.as_ptr(), c"%s".as_ptr(), msg.as_ptr());
            }
        }
    }

    fn flush(&self) {}
}

#[cfg(target_os = "android")]
#[link(name = "log")]
extern "C" {
    fn __android_log_print(
        prio: i32,
        tag: *const std::os::raw::c_char,
        fmt: *const std::os::raw::c_char,
        ...
    ) -> i32;
}

static INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Inicializa el mod al cargarse la librería en el proceso de Factorio
pub fn initialize() -> Result<()> {
    if INITIALIZED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }

    #[cfg(target_os = "android")]
    {
        let _ = log::set_boxed_logger(Box::new(AndroidLogger));
        log::set_max_level(log::LevelFilter::Info);
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).try_init();
    }

    log::info!("=== XFactorVR Tabletop OpenXR inicializado ===");

    // 1. Inicializar gestor de mesa física
    let config = tabletop::TabletopConfig::default();
    TABLETOP.set(tabletop::TabletopManager::new(config)).ok();
    log::info!("Tabletop Manager configurado");

    // 2. Escaneo de símbolos ELF en hilo secundario para NO bloquear el hilo principal de Android
    std::thread::Builder::new()
        .name("xfactorvr_syms".to_string())
        .spawn(|| {
            if let Ok(exe_path) = symbols::find_game_executable() {
                log::info!("Escaneando símbolos en segundo plano: {}", exe_path.display());
                if let Ok(all_syms) = symbols::scan_elf_symbols(&exe_path) {
                    let target_names: Vec<&str> = offsets::HOOK_TARGETS.iter().map(|t| t.symbol_pattern).collect();
                    let resolved = symbols::resolve_symbols(&all_syms, &target_names);
                    log::info!("Resueltos {}/{} símbolos objetivo", resolved.len(), target_names.len());
                }
            }
        })
        .ok();

    Ok(())
}

/// Función C exportada para llamada explícita tras dlopen
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_init() {
    let _ = initialize();
}

/// C FFI para cargar un archivo GLB del mod 3D y convertirlo en un array de DioramaVertex
/// Retorna true si tuvo éxito, false si falló.
/// El llamador debe liberar la memoria con xfactorvr_free_glb.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_load_glb(
    path: *const std::os::raw::c_char,
    target_width: f32,
    out_verts: *mut *mut tabletop::DioramaVertex,
    out_count: *mut i32,
) -> bool {
    if path.is_null() || out_verts.is_null() || out_count.is_null() {
        return false;
    }
    let c_str = match std::ffi::CStr::from_ptr(path).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    match tabletop::load_glb_as_vertices(c_str, target_width) {
        Ok(mut verts) => {
            verts.shrink_to_fit();
            let count = verts.len() as i32;
            let ptr = verts.as_mut_ptr();
            std::mem::forget(verts);
            *out_verts = ptr;
            *out_count = count;
            true
        }
        Err(e) => {
            log::error!("Error cargando GLB {}: {:?}", c_str, e);
            *out_verts = std::ptr::null_mut();
            *out_count = 0;
            false
        }
    }
}

/// Libera la memoria alocada por xfactorvr_load_glb
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_free_glb(
    verts: *mut tabletop::DioramaVertex,
    count: i32,
) {
    if !verts.is_null() && count > 0 {
        let _ = Vec::from_raw_parts(verts, count as usize, count as usize);
    }
}

/// Obtiene una copia de las entidades vivas actuales para renderizarlas en OpenXR
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_live_entities(
    out_entities: *mut tabletop::LiveEntity,
    max_count: i32,
    out_count: *mut i32,
) -> bool {
    if out_entities.is_null() || out_count.is_null() || max_count <= 0 {
        return false;
    }
    let lock = get_live_entities_lock();
    if let Ok(entities) = lock.read() {
        let count = entities.len().min(max_count as usize);
        std::ptr::copy_nonoverlapping(entities.as_ptr(), out_entities, count);
        *out_count = count as i32;
        true
    } else {
        *out_count = 0;
        false
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct PlayerItemFfi {
    pub item_id: u32,
    pub count: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct ResourcePatchFfi {
    pub item_type: u32,
    pub tile_x: f32,
    pub tile_y: f32,
    pub radius: f32,
    pub remaining: u32,
}

fn entity_type_to_item_id(entity_type: u32) -> u32 {
    match entity_type {
        tabletop::ENTITY_ASSEMBLING_MACHINE => game_bridge::ITEM_ASSEMBLING_MACHINE,
        tabletop::ENTITY_MINING_DRILL => game_bridge::ITEM_MINING_DRILL,
        tabletop::ENTITY_FURNACE => game_bridge::ITEM_FURNACE,
        tabletop::ENTITY_GUN_TURRET => game_bridge::ITEM_TURRET,
        tabletop::ENTITY_RADAR => game_bridge::ITEM_RADAR,
        tabletop::ENTITY_ROBOPORT => game_bridge::ITEM_ROBOPORT,
        tabletop::ENTITY_CHEST => game_bridge::ITEM_CHEST,
        tabletop::ENTITY_ELECTRIC_POLE => game_bridge::ITEM_POWER_POLE,
        tabletop::ENTITY_TRANSPORT_BELT => game_bridge::ITEM_TRANSPORT_BELT,
        tabletop::ENTITY_INSERTER => game_bridge::ITEM_INSERTER,
        _ => 0,
    }
}

/// Registra o actualiza una entidad viva (llamado desde hooks o UI de colocación 6DoF)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_place_or_update_entity(
    entity_type: u32,
    tile_x: f32,
    tile_y: f32,
    orientation: f32,
) -> bool {
    let item_id = entity_type_to_item_id(entity_type);
    if item_id > 0 {
        if let Ok(mut world) = game_bridge::get_simulation_world().write() {
            if let Some(cnt) = world.player_inventory.get_mut(&item_id) {
                if *cnt > 0 {
                    *cnt -= 1;
                }
            }
        }
    }

    // Si hay conexión RCON con Factorio, emitir comando a la simulación nativa.
    // La orientación se envía en radianes: el mod la convierte a las 16
    // direcciones de Factorio 2.0. Antes se enviaba fija en 0 y todo salía
    // mirando al norte, que rompía las cadenas de cintas.
    let name_str = match entity_type {
        tabletop::ENTITY_ASSEMBLING_MACHINE => "assembling-machine-1",
        tabletop::ENTITY_MINING_DRILL => "electric-mining-drill",
        tabletop::ENTITY_FURNACE => "stone-furnace",
        tabletop::ENTITY_GUN_TURRET => "gun-turret",
        tabletop::ENTITY_RADAR => "radar",
        tabletop::ENTITY_ROBOPORT => "roboport",
        tabletop::ENTITY_CHEST => "wooden-chest",
        tabletop::ENTITY_ELECTRIC_POLE => "small-electric-pole",
        tabletop::ENTITY_TRANSPORT_BELT => "transport-belt",
        tabletop::ENTITY_INSERTER => "inserter",
        _ => "",
    };
    if !name_str.is_empty() {
        let cmd = format!(
            "/silent-command remote.call('xfactorvr', 'place_entity', '{}', {:.2}, {:.2}, {:.3})",
            name_str, tile_x, tile_y, orientation
        );
        game_bridge::enviar_orden(&cmd);
    }

    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        if let Some(existing) = entities.iter_mut().find(|e| (e.tile_x - tile_x).abs() < 0.5 && (e.tile_y - tile_y).abs() < 0.5) {
            existing.entity_type = entity_type;
            existing.orientation = orientation;
        } else {
            entities.push(tabletop::LiveEntity {
                entity_type,
                tile_x,
                tile_y,
                orientation,
                status: 1,
                animation_progress: 0.0,
                health_ratio: 1.0,
                arm_angle: 0.0,
                sprite_id: 0,
                anim_frame: 0, model_id: 0,
            });
        }
        true
    } else {
        false
    }
}

/// Elimina / mina una entidad en las coordenadas de tile dadas (Mining / Deconstrucción 6DoF)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_mine_entity(
    tile_x: f32,
    tile_y: f32,
) -> bool {
    let mut removed_type = 0;
    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        if let Some(pos) = entities.iter().position(|e| (e.tile_x - tile_x).abs() < 1.0 && (e.tile_y - tile_y).abs() < 1.0) {
            removed_type = entities[pos].entity_type;
            entities.remove(pos);
        }
    }

    if removed_type > 0 {
        let item_id = entity_type_to_item_id(removed_type);
        if item_id > 0 {
            if let Ok(mut world) = game_bridge::get_simulation_world().write() {
                let cnt = world.player_inventory.entry(item_id).or_insert(0);
                *cnt += 1;
            }
        }

        // Si hay conexión RCON con Factorio, emitir comando de minado a la simulación nativa
        let cmd = format!("/c remote.call('xfactorvr', 'mine_entity', {}, {})", tile_x, tile_y);
        game_bridge::enviar_orden(&cmd);
        true
    } else {
        false
    }
}

/// Mueve al JUGADOR REAL del juego a una posicion.
///
/// En la campana el escenario exige un jugador (usa game.players[1]) y las
/// ramas finales piden acciones reales (abrir el pecio, coger piezas). Desde
/// VR el jugador se mueve con el control izquierdo: esto es lo que le lleva
/// al sitio que se ha点了.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_teleport_player(tile_x: f32, tile_y: f32) -> bool {
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'teleport_player', {:.2}, {:.2})",
        tile_x, tile_y
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// Interactua con una entidad: abre su GUI y le recoge el contenido.
///
/// Es el equivalente a hacer clic con el raton: abrir el pecio del nivel de
/// campana, coger las piezas, activar una maquina.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_interact_entity(tile_x: f32, tile_y: f32) -> bool {
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'interact_entity', {:.2}, {:.2})",
        tile_x, tile_y
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// Actualiza el ciclo de simulación y animaciones continuas de las entidades vivas
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_update_simulation(delta_time: f32) {
    // 1) Sondeo RCON. La E/S de red se hace FUERA de los locks del mundo y de
    //    las entidades: el servidor puede tardar y el render de la mesa no
    //    puede congelarse por eso.
    let view_center = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.view_center,
        Err(_) => (0.0, 0.0),
    };
    let poll = game_bridge::poll_rcon_sync(delta_time, view_center);

    // 2) Aplicar el estado real y avanzar la simulación.
    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        if let Ok(mut world) = game_bridge::get_simulation_world().write() {
            world.is_connected_to_rcon = poll.connected;

            if poll.connected {
                if let Some(ref raw) = poll.snapshot {
                    world.apply_snapshot(&mut entities, raw);
                }
                if let Some(ref raw) = poll.inventory {
                    world.apply_inventory(raw);
                }
            } else {
                // Sin motor real conectado: la simulación local lleva la partida.
                world.tick(&mut entities, delta_time);
            }
        }
    }
}

/// Desplaza el centro de la vista sobre la mesa (pan de la ventana de mapa).
/// Permite explorar la fabrica mas alla de las casillas iniciales.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_pan_view(delta_x: f32, delta_y: f32) -> bool {
    if let Ok(mut world) = game_bridge::get_simulation_world().write() {
        world.view_center.0 += delta_x;
        world.view_center.1 += delta_y;
        true
    } else {
        false
    }
}

/// Obtiene el centro actual de la vista en casillas.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_view_center(out_x: *mut f32, out_y: *mut f32) -> bool {
    if out_x.is_null() || out_y.is_null() {
        return false;
    }
    if let Ok(world) = game_bridge::get_simulation_world().read() {
        *out_x = world.view_center.0;
        *out_y = world.view_center.1;
        true
    } else {
        false
    }
}

/// Obtiene los ítems del inventario del jugador
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_player_inventory(
    out_items: *mut PlayerItemFfi,
    max_items: u32,
    out_count: *mut u32,
) -> bool {
    if out_items.is_null() || out_count.is_null() || max_items == 0 {
        return false;
    }

    if let Ok(world) = game_bridge::get_simulation_world().read() {
        let mut count = 0;
        for (&item_id, &qty) in &world.player_inventory {
            if count >= max_items as usize {
                break;
            }
            *out_items.add(count) = PlayerItemFfi { item_id, count: qty };
            count += 1;
        }
        *out_count = count as u32;
        true
    } else {
        *out_count = 0;
        false
    }
}

/// Fabrica un ítem consumiendo los recursos necesarios del inventario
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_craft_item(
    item_id: u32,
    count: u32,
) -> bool {
    if count == 0 {
        return false;
    }

    if let Ok(mut world) = game_bridge::get_simulation_world().write() {
        match item_id {
            // Horno de piedra: 5 piedras
            game_bridge::ITEM_FURNACE => {
                let required = count * 5;
                let stone = world.player_inventory.entry(game_bridge::ITEM_STONE).or_insert(0);
                if *stone >= required {
                    *stone -= required;
                    let furnaces = world.player_inventory.entry(game_bridge::ITEM_FURNACE).or_insert(0);
                    *furnaces += count;
                    return true;
                }
            }
            // Engranaje de hierro: cuesta 2 planchas de hierro
            game_bridge::ITEM_IRON_GEAR => {
                let required = count * 2;
                let plates = world.player_inventory.entry(game_bridge::ITEM_IRON_PLATE).or_insert(0);
                if *plates >= required {
                    *plates -= required;
                    let gears = world.player_inventory.entry(game_bridge::ITEM_IRON_GEAR).or_insert(0);
                    *gears += count;
                    return true;
                }
            }
            // Circuito electrónico: 1 plancha de hierro + 3 planchas de cobre
            game_bridge::ITEM_ELECTRONIC_CIRCUIT => {
                let required_iron = count;
                let required_copper = count * 3;
                let iron = *world.player_inventory.get(&game_bridge::ITEM_IRON_PLATE).unwrap_or(&0);
                let copper = *world.player_inventory.get(&game_bridge::ITEM_COPPER_PLATE).unwrap_or(&0);
                if iron >= required_iron && copper >= required_copper {
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_PLATE).unwrap() -= required_iron;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_COPPER_PLATE).unwrap() -= required_copper;
                    let circuits = world.player_inventory.entry(game_bridge::ITEM_ELECTRONIC_CIRCUIT).or_insert(0);
                    *circuits += count;
                    return true;
                }
            }
            // Cinta transportadora: 1 plancha de hierro + 1 engranaje -> 2 cintas
            game_bridge::ITEM_TRANSPORT_BELT => {
                let required_plates = count;
                let required_gears = count;
                let plates = *world.player_inventory.get(&game_bridge::ITEM_IRON_PLATE).unwrap_or(&0);
                let gears = *world.player_inventory.get(&game_bridge::ITEM_IRON_GEAR).unwrap_or(&0);
                if plates >= required_plates && gears >= required_gears {
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_PLATE).unwrap() -= required_plates;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_GEAR).unwrap() -= required_gears;
                    let belts = world.player_inventory.entry(game_bridge::ITEM_TRANSPORT_BELT).or_insert(0);
                    *belts += count * 2;
                    return true;
                }
            }
            // Minera eléctrica / quemador: 9 planchas + 5 engranajes + 5 piedra
            game_bridge::ITEM_MINING_DRILL => {
                let required_plates = count * 9;
                let required_gears = count * 5;
                let required_stone = count * 5;
                let plates = *world.player_inventory.get(&game_bridge::ITEM_IRON_PLATE).unwrap_or(&0);
                let gears = *world.player_inventory.get(&game_bridge::ITEM_IRON_GEAR).unwrap_or(&0);
                let stone = *world.player_inventory.get(&game_bridge::ITEM_STONE).unwrap_or(&0);
                if plates >= required_plates && gears >= required_gears && stone >= required_stone {
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_PLATE).unwrap() -= required_plates;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_GEAR).unwrap() -= required_gears;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_STONE).unwrap() -= required_stone;
                    let drills = world.player_inventory.entry(game_bridge::ITEM_MINING_DRILL).or_insert(0);
                    *drills += count;
                    return true;
                }
            }
            // Insertador robótico: 1 plancha de hierro + 1 engranaje + 1 circuito
            game_bridge::ITEM_INSERTER => {
                let required_plates = count;
                let required_gears = count;
                let required_circuits = count;
                let plates = *world.player_inventory.get(&game_bridge::ITEM_IRON_PLATE).unwrap_or(&0);
                let gears = *world.player_inventory.get(&game_bridge::ITEM_IRON_GEAR).unwrap_or(&0);
                let circuits = *world.player_inventory.get(&game_bridge::ITEM_ELECTRONIC_CIRCUIT).unwrap_or(&0);
                if plates >= required_plates && gears >= required_gears && circuits >= required_circuits {
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_PLATE).unwrap() -= required_plates;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_GEAR).unwrap() -= required_gears;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_ELECTRONIC_CIRCUIT).unwrap() -= required_circuits;
                    let inserters = world.player_inventory.entry(game_bridge::ITEM_INSERTER).or_insert(0);
                    *inserters += count;
                    return true;
                }
            }
            // Máquina de ensamblaje: 9 planchas + 5 engranajes + 3 circuitos
            game_bridge::ITEM_ASSEMBLING_MACHINE => {
                let required_plates = count * 9;
                let required_gears = count * 5;
                let required_circuits = count * 3;
                let plates = *world.player_inventory.get(&game_bridge::ITEM_IRON_PLATE).unwrap_or(&0);
                let gears = *world.player_inventory.get(&game_bridge::ITEM_IRON_GEAR).unwrap_or(&0);
                let circuits = *world.player_inventory.get(&game_bridge::ITEM_ELECTRONIC_CIRCUIT).unwrap_or(&0);
                if plates >= required_plates && gears >= required_gears && circuits >= required_circuits {
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_PLATE).unwrap() -= required_plates;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_GEAR).unwrap() -= required_gears;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_ELECTRONIC_CIRCUIT).unwrap() -= required_circuits;
                    let machines = world.player_inventory.entry(game_bridge::ITEM_ASSEMBLING_MACHINE).or_insert(0);
                    *machines += count;
                    return true;
                }
            }
            _ => {}
        }
    }

    // Si el motor real de Factorio esta conectado, la fabricacion se pide al
    // juego de verdad usando sus recetas oficiales.
    //
    // OJO: el lock del mundo se suelta ANTES de la llamada de red (si el
    // `.read()` sigue vivo, el bucle de render se queda esperando el
    // `.write()` del mundo mientras el RCON tarda hasta 3 s).
    let recipe_name = match item_id {
        game_bridge::ITEM_FURNACE => "stone-furnace",
        game_bridge::ITEM_IRON_GEAR => "iron-gear-wheel",
        game_bridge::ITEM_ELECTRONIC_CIRCUIT => "electronic-circuit",
        game_bridge::ITEM_TRANSPORT_BELT => "transport-belt",
        game_bridge::ITEM_MINING_DRILL => "electric-mining-drill",
        game_bridge::ITEM_INSERTER => "inserter",
        game_bridge::ITEM_ASSEMBLING_MACHINE => "assembling-machine-1",
        _ => "",
    };
    if recipe_name.is_empty() {
        return false;
    }
    let conectado = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.is_connected_to_rcon,
        Err(_) => false,
    };
    if !conectado {
        return false;
    }
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'craft', '{}', {})",
        recipe_name, count
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// Minado manual con pico sobre yacimientos de mineral o rocas
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_manual_mine(tile_x: f32, tile_y: f32) -> u32 {
    if let Ok(mut world) = game_bridge::get_simulation_world().write() {
        world.manual_mine(tile_x, tile_y)
    } else {
        0
    }
}

/// Interactúa con una máquina (recoge planchas/mineral y suministra combustible/mineral)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_interact_machine(tile_x: f32, tile_y: f32) -> u32 {
    if let Ok(mut world) = game_bridge::get_simulation_world().write() {
        world.interact_machine(tile_x, tile_y)
    } else {
        0
    }
}

/// Reinicia el mundo de simulación al estado inicial auténtico
/// `fresh_start = true` deja la mesa vacía; `false` restaura la fábrica inicial.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_reset_world(fresh_start: bool) -> bool {
    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        if fresh_start {
            entities.clear();
        } else {
            entities.clear();
            entities.extend(tabletop::init_default_factory_entities());
        }
    }
    if let Ok(mut world) = game_bridge::get_simulation_world().write() {
        world.reset_world();
        true
    } else {
        false
    }
}

/// Obtiene información de estado de una máquina para el HUD / GUI
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_machine_info(
    tile_x: f32,
    tile_y: f32,
    out_fuel: *mut f32,
    out_progress: *mut f32,
    out_input_ore: *mut u32,
    out_output_plates: *mut u32,
) -> bool {
    if out_fuel.is_null() || out_progress.is_null() || out_input_ore.is_null() || out_output_plates.is_null() {
        return false;
    }
    if let Ok(world) = game_bridge::get_simulation_world().read() {
        if let Some((fuel, prog, in_ore, out_plates)) = world.get_machine_info(tile_x, tile_y) {
            *out_fuel = fuel;
            *out_progress = prog;
            *out_input_ore = in_ore;
            *out_output_plates = out_plates;
            return true;
        }
    }
    false
}

/// Devuelve el item que viaja sobre una cinta o sostiene un insertador.
/// Permite dibujar en la mesa las piezas que circulan por la fabrica.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_carried_item(
    tile_x: f32,
    tile_y: f32,
    out_item_id: *mut u32,
    out_count: *mut u32,
) -> bool {
    if out_item_id.is_null() || out_count.is_null() {
        return false;
    }
    if let Ok(world) = game_bridge::get_simulation_world().read() {
        if let Some((id, count)) = world.get_carried_item(tile_x, tile_y) {
            *out_item_id = id;
            *out_count = count;
            return true;
        }
    }
    *out_item_id = 0;
    *out_count = 0;
    false
}

/// Obtiene los yacimientos de recursos del mapa para su renderizado en la mesa
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_resource_patches(
    out_patches: *mut ResourcePatchFfi,
    max_patches: u32,
    out_count: *mut u32,
) -> bool {
    if out_patches.is_null() || out_count.is_null() || max_patches == 0 {
        return false;
    }

    if let Ok(world) = game_bridge::get_simulation_world().read() {
        let count = world.patches.len().min(max_patches as usize);
        for i in 0..count {
            let p = &world.patches[i];
            *out_patches.add(i) = ResourcePatchFfi {
                item_type: p.item_type,
                tile_x: p.tile_x,
                tile_y: p.tile_y,
                radius: p.radius,
                remaining: p.remaining,
            };
        }
        *out_count = count as u32;
        true
    } else {
        *out_count = 0;
        false
    }
}

/// Conecta con el servidor RCON de Factorio
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_connect_rcon(
    host_ip: *const std::os::raw::c_char,
    port: u16,
    password: *const std::os::raw::c_char,
) -> bool {
    if host_ip.is_null() || password.is_null() {
        return false;
    }
    let c_host = std::ffi::CStr::from_ptr(host_ip).to_string_lossy();
    let c_pwd = std::ffi::CStr::from_ptr(password).to_string_lossy();
    let addr = format!("{}:{}", c_host, port);

    // Guardar la configuracion para el sondeo automatico posterior.
    if let Ok(mut cfg) = game_bridge::get_rcon_config().lock() {
        cfg.host = c_host.to_string();
        cfg.port = port;
        cfg.password = c_pwd.to_string();
    }

    // El lock del cliente RCON se libera ANTES de tomar el del mundo: el
    // sondeo periodico toma mundo -> rcon, y al revés se produciria un
    // interbloqueo clasico (ABBA).
    let success = if let Ok(mut rcon) = game_bridge::get_rcon_client().lock() {
        rcon.connect(&addr, &c_pwd)
    } else {
        false
    };

    if let Ok(mut world) = game_bridge::get_simulation_world().write() {
        world.is_connected_to_rcon = success;
    }
    success
}

/// Consulta si el puente RCON con Factorio está activo
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_is_rcon_connected() -> bool {
    if let Ok(world) = game_bridge::get_simulation_world().read() {
        world.is_connected_to_rcon
    } else {
        false
    }
}

/// Numero de paginas del atlas de sprites. 0 = no hay atlas.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_atlas_page_count() -> i32 {
    assets::atlas_page_count() as i32
}

/// Tamano de una pagina del atlas (ancho y alto en pixeles).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_atlas_page_size(page: u32, out_w: *mut i32, out_h: *mut i32) -> bool {
    if out_w.is_null() || out_h.is_null() {
        return false;
    }
    match assets::atlas_page_pixels(page) {
        Some((_, w, h)) => {
            *out_w = *w as i32;
            *out_h = *h as i32;
            true
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// Atlas del suelo
// ---------------------------------------------------------------------------

/// Numero de paginas del atlas de suelo.
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_page_count() -> i32 {
    assets::terrain_page_count() as i32
}

/// Tamano de una pagina del atlas de suelo.
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_page_size(
    page: u32,
    out_w: *mut i32,
    out_h: *mut i32,
) -> bool {
    if out_w.is_null() || out_h.is_null() {
        return false;
    }
    match assets::terrain_page_pixels(page) {
        Some((_, w, h)) => {
            *out_w = *w as i32;
            *out_h = *h as i32;
            true
        }
        None => false,
    }
}

/// Pixeles de una pagina del atlas de suelo (RGBA8 crudo).
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_page_pixels(
    page: u32,
    out: *mut u8,
    max_len: i32,
) -> i32 {
    if out.is_null() || max_len <= 0 {
        return 0;
    }
    match assets::terrain_page_pixels(page) {
        Some((data, _, _)) => {
            if (data.len() as i64) > max_len as i64 {
                return 0;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len());
            data.len() as i32
        }
        None => 0,
    }
}

/// Lado de una celda de terreno en pixeles (32 = una casilla del juego).
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_tile_size() -> i32 {
    assets::terrain_atlas().map(|t| t.tile_size as i32).unwrap_or(0)
}

/// Numero de variantes guardadas de un tipo de terreno.
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_variant_count(nombre: *const std::os::raw::c_char) -> i32 {
    if nombre.is_null() {
        return 0;
    }
    let n = match std::ffi::CStr::from_ptr(nombre).to_str() {
        Ok(s) => s,
        Err(_) => return 0,
    };
    assets::terrain_variant_count(n) as i32
}

/// Pone un item en la barra rapida del jugador (virtual o real).
///
/// El tutorial de la campana tiene un nodo que pide filtrar un mineral: para
/// eso el item tiene que estar en la barra rapida.  Con el jugador real lo
/// haria la GUI; con el jugador virtual (sin GUI) lo hace el mod.
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_set_quick_bar(
    page: u32,
    slot: u32,
    item_name: *const std::os::raw::c_char,
) -> bool {
    if item_name.is_null() {
        return false;
    }
    let nombre = match std::ffi::CStr::from_ptr(item_name).to_str() {
        Ok(s) if !s.is_empty() => s.to_string(),
        _ => return false,
    };
    // OJO: el lock del mundo se suelta ANTES de la llamada de red (si el
    // `.read()` sigue vivo, el bucle de render se queda esperando el
    // `.write()` del mundo mientras el RCON tarda hasta 3 s).
    let conectado = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.is_connected_to_rcon,
        Err(_) => false,
    };
    if !conectado {
        return false;
    }
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'set_quick_bar', {}, {}, '{}')",
        page.max(1),
        slot.max(1),
        nombre
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// Pone un item en un inventario concreto (la del horno, por ejemplo).
///
/// `inv_index` es el indice del inventario (1 = principal, 2 = ...) y el
/// nombre del item, como lo espera el mod.
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_insert_item(
    x: f32,
    y: f32,
    item_name: *const std::os::raw::c_char,
    count: u32,
    inv_index: u32,
) -> bool {
    if item_name.is_null() {
        return false;
    }
    let nombre = match std::ffi::CStr::from_ptr(item_name).to_str() {
        Ok(s) if !s.is_empty() => s.to_string(),
        _ => return false,
    };
    // OJO: el lock del mundo se suelta ANTES de la llamada de red (si el
    // `.read()` sigue vivo, el bucle de render se queda esperando el
    // `.write()` del mundo mientras el RCON tarda hasta 3 s).
    let conectado = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.is_connected_to_rcon,
        Err(_) => false,
    };
    if !conectado {
        return false;
    }
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'insert_item', {:.2}, {:.2}, '{}', {}, {})",
        x, y, nombre, count.max(1), inv_index.max(1)
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// Rectangulo de una variante del suelo dentro del atlas.
///
/// out recibe: x, y, ancho, alto y pagina (en ese orden).
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_tile(
    nombre: *const std::os::raw::c_char,
    variant: u32,
    out: *mut f32,
) -> bool {
    if nombre.is_null() || out.is_null() {
        return false;
    }
    let n = match std::ffi::CStr::from_ptr(nombre).to_str() {
        Ok(s) => s,
        Err(_) => return false,
    };
    match assets::terrain_tile(n, variant) {
        Some(r) => {
            *out = r.x;
            *out.add(1) = r.y;
            *out.add(2) = r.w;
            *out.add(3) = r.h;
            *out.add(4) = r.page as f32;
            true
        }
        None => false,
    }
}

/// La malla del suelo, ya con las UV puestas.
///
/// Cada vertice: tile_x, tile_y, u0, v0, u1, v1, pagina.  La geometria se
/// calcula en Rust (asi se puede probar sin el visor) y el motor C++ solo
/// la pinta.
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_mesh(out: *mut f32, max_verts: i32) -> i32 {
    if out.is_null() || max_verts <= 0 {
        return 0;
    }
    let atlas = match assets::terrain_atlas() {
        Some(a) => a,
        None => return 0,
    };
    let mundo = match game_bridge::get_simulation_world().read() {
        Ok(m) => m,
        Err(_) => return 0,
    };
    let quads = terreno::terrain_quads(&mundo.terrain, atlas.page_size);
    let n = quads.len().min(max_verts as usize / 7);
    for (i, q) in quads.iter().take(n).enumerate() {
        let base = i * 7;
        *out.add(base) = q.tile_x as f32;
        *out.add(base + 1) = q.tile_y as f32;
        *out.add(base + 2) = q.u0;
        *out.add(base + 3) = q.v0;
        *out.add(base + 4) = q.u1;
        *out.add(base + 5) = q.v1;
        *out.add(base + 6) = q.page as f32;
    }
    n as i32
}

/// Area de terreno visible: casilla origen y tamaño.
///
/// Devuelve false si todavia no hay terreno (el mundo aun no ha llegado).
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_area(
    out_x0: *mut i32,
    out_y0: *mut i32,
    out_w: *mut i32,
    out_h: *mut i32,
) -> bool {
    if out_x0.is_null() || out_y0.is_null() || out_w.is_null() || out_h.is_null() {
        return false;
    }
    let mundo = match game_bridge::get_simulation_world().read() {
        Ok(m) => m,
        Err(_) => return false,
    };
    let t = &mundo.terrain;
    if t.is_empty() {
        return false;
    }
    *out_x0 = t.x0;
    *out_y0 = t.y0;
    *out_w = t.w as i32;
    *out_h = t.h as i32;
    true
}

/// Nombre del terreno en una casilla del area visible.
///
/// Las coordenadas son relativas al area (0..w, 0..h).  Escribe el nombre
/// en `out_name` (como mucho `out_len` bytes, con terminador) y devuelve
/// false si la casilla no tiene terreno o el nombre no cabe.
#[no_mangle]
pub unsafe extern "C" fn xfactorvr_get_terrain_at(
    rel_x: i32,
    rel_y: i32,
    out_name: *mut std::os::raw::c_char,
    out_len: i32,
) -> bool {
    if out_name.is_null() || out_len <= 1 {
        return false;
    }
    let mundo = match game_bridge::get_simulation_world().read() {
        Ok(m) => m,
        Err(_) => return false,
    };
    let nombre = match mundo.terrain.tile_at(rel_x, rel_y) {
        Some(n) => n,
        None => return false,
    };
    let bytes = nombre.as_bytes();
    if bytes.len() + 1 > out_len as usize {
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), out_name as *mut u8, bytes.len());
    *out_name.add(bytes.len()) = 0;
    true
}

/// Pixeles de una pagina del atlas en RGBA8 crudo, listos para subir a la GPU.
///
/// El atlas se parte en paginas porque una textura de mas de 4096 px de alto no
/// se puede subir en la GPU movil. Devuelve el numero de bytes escritos, o 0
/// si no hay esa pagina. Android no trae decodificador de PNG, por eso el
/// extractor escribe tambien el atlas en crudo.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_atlas_page_pixels(page: u32, out: *mut u8, max_len: i32) -> i32 {
    if out.is_null() || max_len <= 0 {
        return 0;
    }
    match assets::atlas_page_pixels(page) {
        Some((data, _, _)) => {
            if (data.len() as i64) > max_len as i64 {
                return 0;
            }
            std::ptr::copy_nonoverlapping(data.as_ptr(), out, data.len());
            data.len() as i32
        }
        None => 0,
    }
}


/// Rectangulo del frame de un sprite real de Factorio dentro del atlas.
///
/// Lo consume el motor C++ para dibujar el sprite en vez de una primitiva.
/// `out` recibe 10 floats: x, y, w, h (pixeles de la pagina del atlas), el
/// ancho y alto logicos del frame, el numero de direcciones de la capa, los
/// frames de animacion, la escala del sprite y el indice de pagina. Devuelve false si no hay sprite (se dibuja la primitiva de
/// siempre).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_get_sprite_frame(
    sprite_id: u32,
    anim_frame: u32,
    orientation: f32,
    out: *mut f32,
) -> bool {
    if out.is_null() || sprite_id == 0 {
        return false;
    }
    // La direccion de Factorio (0..15) se deduce de la orientacion en
    // radianes: dir = orientacion / TAU * 16.
    let dir = ((orientation / (2.0 * std::f32::consts::PI) * 16.0).round() as i32) & 15;
    match assets::sprite_frame(sprite_id, anim_frame, dir as u8) {
        Some(f) => {
            *out = f.x;
            *out.add(1) = f.y;
            *out.add(2) = f.w;
            *out.add(3) = f.h;
            *out.add(4) = f.frame_width;
            *out.add(5) = f.frame_height;
            *out.add(6) = f.direction_count as f32;
            *out.add(7) = f.animation_frames as f32;
            *out.add(8) = f.scale;
            *out.add(9) = f.page as f32;
            true
        }
        None => false,
    }
}

/// Hook invocado cuando una entidad de Factorio se dibuja en el motor original (ELF Hook bridge)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_on_entity_draw(
    entity_type: u32,
    x_fp: i32,
    y_fp: i32,
    orientation: f32,
    status: u32,
) {
    let tile_x = x_fp as f32 / offsets::RECT_FP;
    let tile_y = y_fp as f32 / offsets::RECT_FP;
    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        if let Some(existing) = entities.iter_mut().find(|e| (e.tile_x - tile_x).abs() < 0.5 && (e.tile_y - tile_y).abs() < 0.5) {
            existing.entity_type = entity_type;
            existing.orientation = orientation;
            existing.status = status;
        } else {
            entities.push(tabletop::LiveEntity {
                entity_type,
                tile_x,
                tile_y,
                orientation,
                status,
                animation_progress: 0.0,
                health_ratio: 1.0,
                arm_angle: 0.0,
                sprite_id: 0,
                anim_frame: 0, model_id: 0,
            });
        }
    }
}

/// Coge un item del suelo con la mano de VR.
///
/// El remoto del mod ("pick_up_item") coge el item que hay en esa casilla y lo
/// mete en el inventario del actor (el jugador real o el personaje virtual).
/// Si no hay ningun item ahi, el mod responde "ERR|no_item" y no pasa nada.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_pick_up_item(tile_x: f32, tile_y: f32) -> bool {
    // OJO: el lock del mundo se suelta ANTES de la llamada de red.
    //
    // Si el `.read()` del mundo sigue vivo durante el `rcon.execute`, el
    // bucle de render (que necesita un `.write()` del mundo para aplicar el
    // snapshot) se queda ESPERANDO: el RwLock no deja pasar las escrituras
    // mientras hay una lectura viva. Y la llamada al RCON puede tardar hasta
    // 3 s (el timeout de lectura): en la Pico eso son tres segundos de
    // visor congelado cada vez que la mano coge un item.
    let conectado = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.is_connected_to_rcon,
        Err(_) => false,
    };
    if !conectado {
        return false;
    }
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'pick_up_item', {:.2}, {:.2})",
        tile_x, tile_y
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// Saca un item de una caja (o de los ingredientes de un horno).
///
/// CASO RARO: `item_name` puede ser NULL o "", y entonces el mod coge el
/// PRIMER item que haya en la caja. En VR todavia no hay HUD de inventario, asi
/// que no se puede elegir QUE item se coge: se coge el primero que haya.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_take_from_container(
    tile_x: f32,
    tile_y: f32,
    item_name: *const std::os::raw::c_char,
    count: u32,
) -> bool {
    // Un nombre vacio (o NULL) significa "el primer item de la caja".
    let nombre = if item_name.is_null() {
        String::new()
    } else {
        match std::ffi::CStr::from_ptr(item_name).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return false,
        }
    };
    // OJO: el lock del mundo se suelta ANTES de la llamada de red (el
    // `.read()` vivo bloquea al bucle de render, que necesita un `.write()`).
    let conectado = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.is_connected_to_rcon,
        Err(_) => false,
    };
    if !conectado {
        return false;
    }
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'take_from_container', {:.2}, {:.2}, '{}', {})",
        tile_x,
        tile_y,
        nombre,
        count.max(1)
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// Mete en una maquina o en una caja el item que lleva el actor.
///
/// Es un TRASPASO real: el item sale del bolsillo del actor y aparece en la
/// maquina. El remoto "insert_item" de antes NO quitaba nada del actor (sirve
/// para el guion de la campana, que mete items que el actor no tiene), asi que
/// para las manos de VR hace falta este.
///
/// CASO RARO: `item_name` NULL o "" = el mod mete el PRIMER item que lleve el
/// actor (el que estaria "en la mano"). Y `inv_index` es el indice del
/// inventario: 1 = el contenido de una caja, 2 = los ingredientes de un horno
/// (en un horno el 1 es el COMBUSTIBLE, no el contenido).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_insert_carried_item(
    tile_x: f32,
    tile_y: f32,
    item_name: *const std::os::raw::c_char,
    count: u32,
    inv_index: u32,
) -> bool {
    // Un nombre vacio (o NULL) significa "el item que lleva en la mano".
    let nombre = if item_name.is_null() {
        String::new()
    } else {
        match std::ffi::CStr::from_ptr(item_name).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return false,
        }
    };
    // OJO: el lock del mundo se suelta ANTES de la llamada de red (el
    // `.read()` vivo bloquea al bucle de render, que necesita un `.write()`).
    let conectado = match game_bridge::get_simulation_world().read() {
        Ok(w) => w.is_connected_to_rcon,
        Err(_) => false,
    };
    if !conectado {
        return false;
    }
    let cmd = format!(
        "/silent-command remote.call('xfactorvr', 'insert_carried_item', {:.2}, {:.2}, '{}', {}, {})",
        tile_x,
        tile_y,
        nombre,
        count.max(1),
        inv_index.max(1)
    );
    game_bridge::enviar_orden(&cmd);
    true
}

/// La accion que una mano ha ARMADO, esperando a que el dedo se suelte.
///
/// Es el gesto diferido de Generals-Zero-Hour-XR: al apretar el gatillo NO se
/// emite la orden, se arma; la orden sale cuando el dedo se levanta.  Asi un
/// roce, un movimiento de la mano o una perdida de tracking no ejecutan nada
/// por accidente, y el usuario ve que la accion esta preparada antes de que
/// ocurra.
#[derive(Clone, Debug)]
pub struct ManoArmada {
    pub mano: openxr::actions::VrHand,
    pub accion: openxr::actions::VrAction,
}

/// La mano que tiene una accion armada (si hay alguna).
static MANO_ARMADA: std::sync::Mutex<Option<ManoArmada>> = std::sync::Mutex::new(None);

/// Ejecuta la accion de VR contra el juego de verdad.
fn ejecutar_accion_vr(accion: &openxr::actions::VrAction) -> bool {
    match accion {
        openxr::actions::VrAction::PickUpItem { tile_x, tile_y } => {
            unsafe { xfactorvr_pick_up_item(*tile_x, *tile_y) }
        }
        openxr::actions::VrAction::TakeFromContainer {
            tile_x,
            tile_y,
            item_name,
            count,
        } => {
            let nombre = std::ffi::CString::new(item_name.clone()).unwrap_or_default();
            unsafe { xfactorvr_take_from_container(*tile_x, *tile_y, nombre.as_ptr(), *count) }
        }
        openxr::actions::VrAction::InsertItem {
            tile_x,
            tile_y,
            item_name,
            count,
            inv_index,
        } => {
            let nombre = std::ffi::CString::new(item_name.clone()).unwrap_or_default();
            unsafe {
                xfactorvr_insert_carried_item(
                    *tile_x,
                    *tile_y,
                    nombre.as_ptr(),
                    *count,
                    *inv_index,
                )
            }
        }
    }
}

/// ARMA la accion de la mano: el gatillo se ha apretado pero todavia no se
/// ejecuta nada.
///
/// Devuelve true si hay algo interactuable bajo el rayo (o sea, si la mano
/// "ha cogido" algo y el motor de VR debe dejar de tratar el gatillo como un
/// clic de raton).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_vr_hand_action(tile_x: f32, tile_y: f32, hand: u32) -> bool {
    // Sin motor real conectado no se arma NADA: el gatillo tiene que seguir
    // siendo el raton de siempre.  (Con el gesto diferido, armar es todavia
    // una promesa de accion, y una promesa sin motor no vale nada.)
    let hay_motor = game_bridge::get_simulation_world()
        .read()
        .map(|w| w.is_connected_to_rcon)
        .unwrap_or(false);
    if !hay_motor {
        return false;
    }
    let live = get_live_entities_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();
    let loose = game_bridge::get_simulation_world()
        .read()
        .map(|w| w.loose_items.clone())
        .unwrap_or_default();
    let target = tabletop::classify_target(&live, &loose, tile_x, tile_y);
    let mano = if hand == 0 {
        openxr::actions::VrHand::Trigger
    } else {
        openxr::actions::VrHand::Grip
    };
    match openxr::actions::decide_vr_action(&target, tile_x, tile_y, mano) {
        Some(accion) => {
            let mut estado = MANO_ARMADA.lock().unwrap_or_else(|e| e.into_inner());
            *estado = Some(ManoArmada { mano, accion });
            true
        }
        None => false,
    }
}

/// SUELTA la mano: el dedo se levanta y la accion armada se ejecuta.
///
/// Es la segunda mitad del gesto diferido.  Si la accion no estaba armada (o
/// es de la otra mano) no hace nada.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_vr_hand_release(hand: u32) -> bool {
    let mano = if hand == 0 {
        openxr::actions::VrHand::Trigger
    } else {
        openxr::actions::VrHand::Grip
    };
    let armada = {
        let mut estado = MANO_ARMADA.lock().unwrap_or_else(|e| e.into_inner());
        match estado.take() {
            Some(a) if a.mano == mano => Some(a),
            otra => {
                // No es la mano armada: se devuelve al estado para que otro
                // dedo no la coja por error.
                *estado = otra;
                None
            }
        }
    };
    match armada {
        Some(a) => ejecutar_accion_vr(&a.accion),
        None => false,
    }
}

/// Dice si hay una accion ARMADA (para que el visor pueda dar el feedback).
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_vr_hand_armed() -> bool {
    MANO_ARMADA.lock().map(|e| e.is_some()).unwrap_or(false)
}

// ============================================================================
// HUD DE VR: DEL LOGO DE DISENO A GEOMETRIA
// ============================================================================
//
// El modulo `ui` (src/ui/) sabe COMO se ve el HUD: que controles hay, donde,
// con que estado, y incluso tiene la fuente bitmap 5x7 para el texto.  Pero el
// motor de VR (C++) solo sabe pintar triangulos.
//
// Estas funciones hacen de puente: el HUD se convierte en la misma geometria
// que el diorama (DioramaVertex), y el C++ la sube a su buffer dinamico y la
// dibuja con el shader que ya tiene.  El texto se compone de un quad por
// pixel encendido de la fuente, asi que no hace falta ninguna fuente del
// sistema.
//
// Las ventanas se colocan en el ESPACIO DE LA MESA (como el resto del
// diorama), que es donde el motor de VR ya sabe aplicar la matriz de la mesa.

static HUD: OnceLock<std::sync::Mutex<ui::Hud>> = OnceLock::new();

/// El HUD global. Se construye con el inventario real del jugador.
fn hud() -> &'static std::sync::Mutex<ui::Hud> {
    HUD.get_or_init(|| {
        let inventario = game_bridge::get_simulation_world()
            .read()
            .map(|w| w.player_inventory.clone())
            .unwrap_or_default();
        let items: Vec<ui::ItemResumen> = inventario
            .iter()
            .map(|(item_id, cantidad)| ui::ItemResumen {
                item_id: *item_id,
                nombre: String::new(),
                cantidad: *cantidad,
            })
            .collect();
        std::sync::Mutex::new(ui::Hud::nuevo(&items, &[]))
    })
}

/// Añade un quad (dos triángulos) con el color en los vertices.
fn push_quad(
    out: &mut Vec<tabletop::DioramaVertex>,
    centro: glam::Vec3,
    eje_x: glam::Vec3,
    eje_y: glam::Vec3,
    ancho: f32,
    alto: f32,
    color: ui::tabla::Color,
) {
    let hx = eje_x * (ancho * 0.5);
    let hy = eje_y * (alto * 0.5);
    let c = [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        color.a as f32 / 255.0,
    ];
    let normal = {
        let n = eje_x.cross(eje_y).normalize_or_zero();
        [n.x, n.y, n.z]
    };
    let puntos = [
        centro - hx - hy,
        centro + hx - hy,
        centro + hx + hy,
        centro - hx - hy,
        centro + hx + hy,
        centro - hx + hy,
    ];
    for p in puntos {
        out.push(tabletop::DioramaVertex {
            x: p.x,
            y: p.y,
            z: p.z,
            nx: normal[0],
            ny: normal[1],
            nz: normal[2],
            u: 0.0,
            v: 0.0,
            r: c[0],
            g: c[1],
            b: c[2],
            a: c[3],
        });
    }
}

/// Geometria del HUD lista para dibujar. Devuelve cuantos vertices escribio.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_ui_vertices(
    out: *mut tabletop::DioramaVertex,
    max: i32,
) -> i32 {
    if out.is_null() || max <= 0 {
        return 0;
    }
    let buffer = std::slice::from_raw_parts_mut(out, max as usize);
    let mut verts: Vec<tabletop::DioramaVertex> = Vec::new();

    {
        let h = hud().lock().unwrap_or_else(|e| e.into_inner());
        for ventana_id in ui::ventanas::IdVentana::TODAS {
            let ventana = match h.ventanas.ventana(ventana_id) {
                Some(v) => v,
                None => continue,
            };
            // Escala: de pixeles de la ventana a metros.
            let escala = if ventana.ancho_px > 0 {
                ventana.ancho_m / ventana.ancho_px as f32
            } else {
                0.0
            };
            if escala <= 0.0 {
                continue;
            }
            let rot = ventana.plano.rotacion;
            let centro = ventana.plano.posicion;
            // Ejes del plano de la ventana (la mira por su -Z local).
            let eje_x = rot * glam::Vec3::X;
            let eje_y = rot * glam::Vec3::Y;

            // --- El cristal de la ventana -----------------------------------
            let (ancho_px, alto_px) = (ventana.ancho_px, ventana.alto_px);
            push_quad(
                &mut verts,
                centro,
                eje_x,
                eje_y,
                ancho_px as f32 * escala,
                alto_px as f32 * escala,
                ui::tabla::Color::FONDO,
            );

            // --- Los controles ------------------------------------------------
            for prim in h.render_ventana(ventana_id) {
                match prim {
                    ui::tabla::Primitiva::Rect { x, y, w, h: alto, color } => {
                        let centro_px = (x as f32 + w as f32 * 0.5, y as f32 + alto as f32 * 0.5);
                        let centro_local = (centro_px.0 - ancho_px as f32 * 0.5) * escala;
                        let centro_local_y = (alto_px as f32 * 0.5 - centro_px.1) * escala;
                        let p = centro
                            + eje_x * centro_local
                            + eje_y * centro_local_y;
                        push_quad(
                            &mut verts,
                            p,
                            eje_x,
                            eje_y,
                            w as f32 * escala,
                            alto as f32 * escala,
                            color,
                        );
                    }
                    ui::tabla::Primitiva::Borde {
                        x,
                        y,
                        w,
                        h: alto,
                        color,
                        grosor,
                    } => {
                        // El borde se dibuja como cuatro tiras finas.
                        let g = grosor.max(1) as f32 * escala;
                        let lados = [
                            (x, y, w, grosor),
                            (x, y + alto - grosor, w, grosor),
                            (x, y, grosor, alto),
                            (x + w - grosor, y, grosor, alto),
                        ];
                        for (lx, ly, lw, lh) in lados {
                            let cx = lx as f32 + lw as f32 * 0.5;
                            let cy = ly as f32 + lh as f32 * 0.5;
                            let local_x = (cx - ancho_px as f32 * 0.5) * escala;
                            let local_y = (alto_px as f32 * 0.5 - cy) * escala;
                            let p = centro + eje_x * local_x + eje_y * local_y;
                            push_quad(&mut verts, p, eje_x, eje_y, lw as f32 * escala, lh as f32 * escala, color);
                        }
                    }
                    ui::tabla::Primitiva::Badge { .. } => {
                        // Los badges los dibuja el renderizador como rectangulo
                        // con su numero; aqui se ignoran (el numero va en el
                        // texto de la celda).
                    }
                    ui::tabla::Primitiva::Texto { celdas, escala: esc } => {
                        // Cada pixel ENCENDIDO de la fuente 5x7 es un quad.
                        let px = esc.max(1) as f32 * escala;
                        for celda in celdas {
                            for fila in 0..7usize {
                                let bits = celda.filas[fila];
                                for bit in 0..5usize {
                                    if bits & (1 << (4 - bit)) == 0 {
                                        continue;
                                    }
                                    let cx = celda.x as f32 + bit as f32 + 0.5;
                                    let cy = celda.y as f32 + fila as f32 + 0.5;
                                    let local_x = (cx - ancho_px as f32 * 0.5) * escala;
                                    let local_y = (alto_px as f32 * 0.5 - cy) * escala;
                                    let p = centro + eje_x * local_x + eje_y * local_y;
                                    push_quad(&mut verts, p, eje_x, eje_y, px, px, celda.color);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let n = verts.len().min(max as usize);
    buffer[..n].copy_from_slice(&verts[..n]);
    n as i32
}

/// Traduce un item del inventario al tipo de entidad que se coloca en la
/// mesa (el inverso de `entity_type_to_item_id`).
fn item_id_to_entity_type(item_id: u32) -> u32 {
    match item_id {
        game_bridge::ITEM_ASSEMBLING_MACHINE => tabletop::ENTITY_ASSEMBLING_MACHINE,
        game_bridge::ITEM_MINING_DRILL => tabletop::ENTITY_MINING_DRILL,
        game_bridge::ITEM_FURNACE => tabletop::ENTITY_FURNACE,
        game_bridge::ITEM_TURRET => tabletop::ENTITY_GUN_TURRET,
        game_bridge::ITEM_RADAR => tabletop::ENTITY_RADAR,
        game_bridge::ITEM_ROBOPORT => tabletop::ENTITY_ROBOPORT,
        game_bridge::ITEM_CHEST => tabletop::ENTITY_CHEST,
        game_bridge::ITEM_POWER_POLE => tabletop::ENTITY_ELECTRIC_POLE,
        game_bridge::ITEM_TRANSPORT_BELT => tabletop::ENTITY_TRANSPORT_BELT,
        game_bridge::ITEM_INSERTER => tabletop::ENTITY_INSERTER,
        _ => 0,
    }
}

/// Geometria de una ventana del HUD, para que el motor de VR pueda hacer el
/// hit-test con el rayo de la mano.
///
/// Escribe en `out` (12 floats): posicion xyz, rotacion (cuaternion xyzw),
/// escala, ancho_px, alto_px, ancho_m y alto_m.  Devuelve false si esa
/// ventana no existe.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_ui_ventana_geom(ventana: u32, out: *mut f32) -> bool {
    if out.is_null() {
        return false;
    }
    let h = hud().lock().unwrap_or_else(|e| e.into_inner());
    let id = match ventana {
        0 => ui::ventanas::IdVentana::Construir,
        1 => ui::ventanas::IdVentana::Inventario,
        2 => ui::ventanas::IdVentana::Fabricar,
        3 => ui::ventanas::IdVentana::Vista,
        _ => return false,
    };
    let Some(v) = h.ventanas.ventana(id) else {
        return false;
    };
    let (ancho_px, alto_px) = h.tamano_px(id);
    let q = v.plano.rotacion;
    let buf = [
        v.plano.posicion.x,
        v.plano.posicion.y,
        v.plano.posicion.z,
        q.x,
        q.y,
        q.z,
        q.w,
        v.plano.escala,
        ancho_px as f32,
        alto_px as f32,
        v.ancho_m,
        v.alto_m(),
    ];
    std::ptr::copy_nonoverlapping(buf.as_ptr(), out, buf.len());
    true
}

/// Pulsa un control del HUD y ejecuta lo que corresponda.
///
/// Devuelve el TIPO DE ENTIDAD a colocar en la mesa (0 = nada que colocar).
/// Para "Fabricar" la receta se fabrica aqui mismo contra el motor real.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_ui_pulsar(ventana: u32, id: u32) -> i32 {
    let id_ventana = match ventana {
        0 => ui::ventanas::IdVentana::Construir,
        1 => ui::ventanas::IdVentana::Inventario,
        2 => ui::ventanas::IdVentana::Fabricar,
        3 => ui::ventanas::IdVentana::Vista,
        _ => return 0,
    };
    let accion = {
        let mut h = hud().lock().unwrap_or_else(|e| e.into_inner());
        h.pulsar(id_ventana, id)
    };
    match accion {
        Some(ui::tabla::Accion::Construir(item_id)) => {
            // Elegir la maquina: el motor de VR la pondra como "herramienta de
            // colocacion" y el usuario la colocara pulsando sobre la mesa.
            let tipo = item_id_to_entity_type(item_id);
            let mut h = hud().lock().unwrap_or_else(|e| e.into_inner());
            h.estados.elegir_maquina(item_id, (id_ventana, id));
            h.sincronizar_estados();
            tipo as i32
        }
        Some(ui::tabla::Accion::Fabricar(item_id)) => {
            // Fabricar de verdad: el mod hace la receta en el Factorio real.
            xfactorvr_craft_item(item_id, 1);
            let mut h = hud().lock().unwrap_or_else(|e| e.into_inner());
            h.estados.elegir_receta(item_id, (id_ventana, id));
            h.sincronizar_estados();
            0
        }
        _ => 0,
    }
}

/// Hit-test del HUD: que control hay bajo el punto (u, v) de la ventana.
///
/// Devuelve el id del control o -1.
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_ui_hit(ventana: u32, u: f32, v: f32) -> i64 {
    let h = hud().lock().unwrap_or_else(|e| e.into_inner());
    let id_ventana = match ventana {
        0 => ui::ventanas::IdVentana::Construir,
        1 => ui::ventanas::IdVentana::Inventario,
        2 => ui::ventanas::IdVentana::Fabricar,
        3 => ui::ventanas::IdVentana::Vista,
        _ => return -1,
    };
    match h.hit_test(id_ventana, u, v) {
        Some(id) => id as i64,
        None => -1,
    }
}

/// Pista de texto del modo actual (lo que el usuario debe hacer ahora).
///
/// La cadena se copia al buffer del llamante; devuelve su longitud.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_ui_pista(out: *mut std::os::raw::c_char, max: i32) -> i32 {
    if out.is_null() || max <= 0 {
        return 0;
    }
    let h = hud().lock().unwrap_or_else(|e| e.into_inner());
    let pista = h.pista().as_bytes();
    let n = pista.len().min(max as usize - 1);
    std::ptr::copy_nonoverlapping(pista.as_ptr(), out as *mut u8, n);
    *out.add(n) = 0;
    n as i32
}

/// Ejecuta la accion de inventario que decide la mano de VR.
///
/// `hand`: 0 = gatillo (coger), 1 = gripo (meter). Decide lo MISMO que la
/// logica de las manos (src/openxr/actions.rs): si bajo el rayo hay un item
/// suelto, una caja o un horno, hace su accion; si no hay nada interactuable,
/// NO hace nada (no es una accion destructiva).
///
/// Es el gancho que el motor de VR llama con la casilla que apunta el rayo.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_vr_hand_action_directo(tile_x: f32, tile_y: f32, hand: u32) -> bool {
    let live = get_live_entities_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();
    let loose = game_bridge::get_simulation_world()
        .read()
        .map(|w| w.loose_items.clone())
        .unwrap_or_default();
    let target = tabletop::classify_target(&live, &loose, tile_x, tile_y);
    let mano = if hand == 0 {
        openxr::actions::VrHand::Trigger
    } else {
        openxr::actions::VrHand::Grip
    };
    match openxr::actions::decide_vr_action(&target, tile_x, tile_y, mano) {
        Some(accion) => ejecutar_accion_vr(&accion),
        None => false,
    }
}

// Constructor ejecutado automáticamente por el linker dinámico en Linux / Android
#[cfg(any(target_os = "linux", target_os = "android"))]
#[unsafe(export_name = "_init")]
pub extern "C" fn _init() {
    let _ = initialize();
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    /// Los tests comparten el estado global de la mesa (lista de entidades).
    /// Este cerrojo serializa los que lo modifican para que no se pisen.
    static TEST_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_test_state() -> std::sync::MutexGuard<'static, ()> {
        TEST_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_tabletop_raycast() {
        let mgr = tabletop::TabletopManager::new(tabletop::TabletopConfig::default());
        // Rayo apuntando directamente hacia abajo desde la posición de la cabeza (0, 0, 0) hacia la mesa (0, -0.3, -1.0)
        let ray_origin = Vec3::new(0.0, 0.0, -1.0);
        let ray_dir = Vec3::new(0.0, -1.0, 0.0);

        let hit = mgr.pick_tile_from_ray(ray_origin, ray_dir);
        assert!(hit.is_some(), "El rayo debe impactar en el centro de la mesa");
        let (tile_x, tile_y) = hit.unwrap();
        assert!((tile_x - 0.0).abs() < 1e-4);
        assert!((tile_y - 0.0).abs() < 1e-4);
    }

    #[test]
    fn test_live_entities_management() {
        let _guard = lock_test_state();
        // Restaurar la fabrica inicial para partir de un estado determinista.
        unsafe { xfactorvr_reset_world(false) };
        let mut buffer = [tabletop::LiveEntity {
            entity_type: 0,
            tile_x: 0.0,
            tile_y: 0.0,
            orientation: 0.0,
            status: 0,
            animation_progress: 0.0,
            health_ratio: 0.0,
            arm_angle: 0.0,
            sprite_id: 0,
            anim_frame: 0, model_id: 0 }; 64];
        let mut count = 0;

        // 1. Obtener entidades iniciales
        let ok = unsafe { xfactorvr_get_live_entities(buffer.as_mut_ptr(), 64, &mut count) };
        assert!(ok);
        assert!(count > 0, "Debe haber entidades iniciales en la fábrica");
        let initial_count = count;

        // 2. Colocar una nueva ensambladora en (15.0, 15.0)
        let placed = unsafe { xfactorvr_place_or_update_entity(tabletop::ENTITY_ASSEMBLING_MACHINE, 15.0, 15.0, 0.0) };
        assert!(placed);

        // 3. Verificar que la cuenta aumentó
        let ok2 = unsafe { xfactorvr_get_live_entities(buffer.as_mut_ptr(), 64, &mut count) };
        assert!(ok2);
        assert_eq!(count, initial_count + 1);

        // 4. Minar / deconstruir la entidad colocada
        let mined = unsafe { xfactorvr_mine_entity(15.0, 15.0) };
        assert!(mined);

        // 5. Verificar que regresó a la cuenta original
        let ok3 = unsafe { xfactorvr_get_live_entities(buffer.as_mut_ptr(), 64, &mut count) };
        assert!(ok3);
        assert_eq!(count, initial_count);
    }

    #[test]
    fn test_simulation_progression_and_hook() {
        let _guard = lock_test_state();
        let mut buffer = [tabletop::LiveEntity {
            entity_type: 0,
            tile_x: 0.0,
            tile_y: 0.0,
            orientation: 0.0,
            status: 0,
            animation_progress: 0.0,
            health_ratio: 0.0,
            arm_angle: 0.0,
            sprite_id: 0,
            anim_frame: 0, model_id: 0 }; 64];
        let mut count = 0;

        // Probar hook de Factorio C++
        unsafe {
            xfactorvr_on_entity_draw(tabletop::ENTITY_GUN_TURRET, (5.0 * 256.0) as i32, (5.0 * 256.0) as i32, 1.57, 1);
        }
        let ok = unsafe { xfactorvr_get_live_entities(buffer.as_mut_ptr(), 64, &mut count) };
        assert!(ok);
        let turret = buffer[..count as usize].iter().find(|e| (e.tile_x - 5.0).abs() < 0.1 && (e.tile_y - 5.0).abs() < 0.1);
        assert!(turret.is_some(), "La torreta agregada por el hook debe existir");
        let initial_progress = turret.unwrap().animation_progress;

        // Avanzar simulación
        unsafe {
            xfactorvr_update_simulation(0.1);
        }
        let ok2 = unsafe { xfactorvr_get_live_entities(buffer.as_mut_ptr(), 64, &mut count) };
        assert!(ok2);
        let updated_turret = buffer[..count as usize].iter().find(|e| (e.tile_x - 5.0).abs() < 0.1 && (e.tile_y - 5.0).abs() < 0.1).unwrap();
        assert!(updated_turret.animation_progress > initial_progress, "El progreso de animación debe avanzar");

        // Limpiar
        unsafe {
            xfactorvr_mine_entity(5.0, 5.0);
        }
    }

    /// La accion de la mano de VR no puede hacer nada tonto: si bajo el rayo
    /// no hay ninguna entidad interactuable, la accion devuelve false (y no
    /// manda ningun comando al juego).
    #[test]
    fn test_accion_vr_sin_entidad_no_hace_nada() {
        let _guard = lock_test_state();
        unsafe { xfactorvr_reset_world(false) };
        // Lejos de todo: no hay caja, ni horno, ni item suelto.
        assert!(!unsafe { xfactorvr_vr_hand_action(1000.0, 1000.0, 0) });
        assert!(!unsafe { xfactorvr_vr_hand_action(1000.0, 1000.0, 1) });
    }

    /// Sin conexion RCON las acciones de la mano no mandan nada (devuelven
    /// false): es la garantia de que el visor no hace inventos cuando el
    /// puente con Factorio no esta conectado.
    #[test]
    fn test_acciones_vr_sin_rcon_no_mandan_nada() {
        let _guard = lock_test_state();
        unsafe { xfactorvr_reset_world(false) };
        if let Ok(mut world) = game_bridge::get_simulation_world().write() {
            world.is_connected_to_rcon = false;
        }
        assert!(!unsafe { xfactorvr_pick_up_item(1.0, 1.0) });
        assert!(!unsafe { xfactorvr_take_from_container(1.0, 1.0, std::ptr::null(), 1) });
        assert!(!unsafe { xfactorvr_insert_carried_item(1.0, 1.0, std::ptr::null(), 1, 1) });
        // La caja de la fabrica de ejemplo esta en (-10.5, -1.5): aun asi no
        // se arma nada, porque no hay RCON (y sin motor el gatillo tiene que
        // seguir siendo el raton).
        assert!(!unsafe { xfactorvr_vr_hand_action(-10.5, -1.5, 0) });
        assert!(!unsafe { xfactorvr_vr_hand_armed() });
        // Y soltar tampoco manda nada.
        assert!(!unsafe { xfactorvr_vr_hand_release(0) });
    }

    #[test]
    /// La geometria que manda el motor (canal del ADR-021) se guarda tal cual
    /// y la tabla de nombres se separa bien.
    ///
    /// Es lo que permite colocar los modelos 3D en la mesa con la posicion
    /// EXACTA con la que los dibuja el motor, en vez de la estimada por el
    /// snapshot.
    #[test]
    fn test_la_geometria_del_motor_se_guarda() {
        // 6 floats por entidad: wx, wy, sx, sy, direccion, modelo.
        let datos: [f32; 12] = [
            10.5, -3.25, 2.0, 2.0, 4.0, 0.0,   // maquina en (10.5, -3.25), este
            -1.0, 7.5, 1.0, 1.0, 8.0, 1.0,      // cofre en (-1, 7.5), sur
        ];
        let nombres = c"assembling-machine-1\nwooden-chest";

        // SAFETY: se pasan datos validos y un puntero a una cadena terminada.
        let n = unsafe { renderer::xfactorvr_geometria(datos.as_ptr(), 2, nombres.as_ptr()) };
        assert_eq!(n, 2, "tiene que guardar las dos entidades");

        let geo = renderer::geometria();
        assert_eq!(geo.len(), 2);
        assert!((geo[0].wx - 10.5).abs() < 0.001);
        assert!((geo[0].wy + 3.25).abs() < 0.001, "wy = {}", geo[0].wy);
        assert_eq!(geo[0].direccion, 4);
        assert_eq!(geo[0].modelo, 0);
        assert!((geo[1].wx + 1.0).abs() < 0.001);
        assert_eq!(geo[1].direccion, 8);
        assert_eq!(geo[1].modelo, 1);
    }

    fn test_hud_genera_geometria() {
        // El HUD tiene que producir triangulos de verdad: sin esto, el motor
        // de VR se queda con el HUD viejo y no se ve nada de lo nuevo.
        let mut verts = vec![tabletop::DioramaVertex {
            x: 0.0, y: 0.0, z: 0.0,
            nx: 0.0, ny: 0.0, nz: 1.0,
            u: 0.0, v: 0.0,
            r: 0.0, g: 0.0, b: 0.0, a: 0.0,
        }; 24000];
        let n = unsafe { xfactorvr_ui_vertices(verts.as_mut_ptr(), verts.len() as i32) };
        assert!(n > 0, "el HUD tiene que generar geometria");
        assert!(n % 6 == 0, "la geometria va en triangulos: {n} vertices");
        assert!(
            n < verts.len() as i32,
            "el HUD no debe llenarse el buffer (quedaria truncado): {n}"
        );

        // Y la pista del modo tiene que decir algo util.
        let mut buffer = [0i8; 256];
        let len = unsafe {
            xfactorvr_ui_pista(buffer.as_mut_ptr() as *mut std::os::raw::c_char, buffer.len() as i32)
        };
        assert!(len > 0, "el HUD tiene que escribir la pista del modo");
        let pista = unsafe {
            std::ffi::CStr::from_ptr(buffer.as_ptr() as *const std::os::raw::c_char)
        };
        assert!(!pista.to_str().unwrap_or("").is_empty());
    }

    #[test]
    fn test_early_game_progression_ffi() {        let _guard = lock_test_state();
        unsafe {
            // 1. Resetear mundo
            assert!(xfactorvr_reset_world(true));

            let mut inv_buf = [PlayerItemFfi { item_id: 0, count: 0 }; 16];
            let mut inv_count = 0;
            assert!(xfactorvr_get_player_inventory(inv_buf.as_mut_ptr(), 16, &mut inv_count));
            assert!(inv_count >= 4);

            // 2. Minar manualmente 5 piedras
            for _ in 0..5 {
                let mined = xfactorvr_manual_mine(-6.0, -4.0);
                assert_eq!(mined, game_bridge::ITEM_STONE);
            }

            // 3. Fabricar el primer Horno de Piedra
            let crafted = xfactorvr_craft_item(game_bridge::ITEM_FURNACE, 1);
            assert!(crafted, "Debe poder fabricar el horno con las 5 piedras minadas");

            // 4. Colocar la perforadora sobre la veta de hierro y el horno justo enfrente
            assert!(xfactorvr_place_or_update_entity(tabletop::ENTITY_MINING_DRILL, -6.0, 4.0, 0.0));
            assert!(xfactorvr_place_or_update_entity(tabletop::ENTITY_FURNACE, -6.0, 3.0, 0.0));

            // Suministrar carbón inicial interactuando con el horno
            xfactorvr_interact_machine(-6.0, 3.0);

            // 5. Avanzar la simulación durante 8 segundos
            for _ in 0..16 {
                xfactorvr_update_simulation(0.5);
            }

            // 6. Consultar estado del horno y recoger las planchas de hierro fundidas
            let mut fuel = 0.0f32;
            let mut progress = 0.0f32;
            let mut in_ore = 0u32;
            let mut out_plates = 0u32;
            let has_info = xfactorvr_get_machine_info(-6.0, 3.0, &mut fuel, &mut progress, &mut in_ore, &mut out_plates);
            assert!(has_info);
            assert!(out_plates >= 1, "El horno debe haber producido al menos 1 plancha de hierro");

            let looted = xfactorvr_interact_machine(-6.0, 3.0);
            assert!(looted >= 1, "Debe recoger las planchas fundidas del horno");
        }
    }
}
