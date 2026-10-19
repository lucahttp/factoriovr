// lib.rs — Punto de entrada para XFactorVR (libxfactorvr.so)
//
// Inyectado en Factorio vía LD_PRELOAD para proporcionar la experiencia
// Tabletop Mixed Reality en Pico 4 Ultra Standalone / OpenXR.

pub mod game_bridge;
pub mod offsets;
pub mod openxr;
pub mod renderer;
pub mod symbols;
pub mod tabletop;

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

    // Si hay conexión RCON con Factorio, emitir comando a la simulación nativa
    if let Ok(mut rcon) = game_bridge::get_rcon_client().lock() {
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
                "/c remote.call('xfactorvr', 'place_entity', '{}', {}, {}, 0)",
                name_str, tile_x, tile_y
            );
            rcon.execute(&cmd);
        }
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
        if let Ok(mut rcon) = game_bridge::get_rcon_client().lock() {
            let cmd = format!("/c remote.call('xfactorvr', 'mine_entity', {}, {})", tile_x, tile_y);
            rcon.execute(&cmd);
        }
        true
    } else {
        false
    }
}

/// Actualiza el ciclo de simulación y animaciones continuas de las entidades vivas
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_update_simulation(delta_time: f32) {
    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        if let Ok(mut world) = game_bridge::get_simulation_world().write() {
            world.tick(&mut entities, delta_time);
        }
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
            // Minera eléctrica: 10 planchas + 5 engranajes + 3 circuitos
            game_bridge::ITEM_MINING_DRILL => {
                let required_plates = count * 10;
                let required_gears = count * 5;
                let plates = *world.player_inventory.get(&game_bridge::ITEM_IRON_PLATE).unwrap_or(&0);
                let gears = *world.player_inventory.get(&game_bridge::ITEM_IRON_GEAR).unwrap_or(&0);
                if plates >= required_plates && gears >= required_gears {
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_PLATE).unwrap() -= required_plates;
                    *world.player_inventory.get_mut(&game_bridge::ITEM_IRON_GEAR).unwrap() -= required_gears;
                    let drills = world.player_inventory.entry(game_bridge::ITEM_MINING_DRILL).or_insert(0);
                    *drills += count;
                    return true;
                }
            }
            _ => {}
        }
    }
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

    if let Ok(mut rcon) = game_bridge::get_rcon_client().lock() {
        let success = rcon.connect(&addr, &c_pwd);
        if let Ok(mut world) = game_bridge::get_simulation_world().write() {
            world.is_connected_to_rcon = success;
        }
        success
    } else {
        false
    }
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
            });
        }
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
        let mut buffer = [tabletop::LiveEntity {
            entity_type: 0,
            tile_x: 0.0,
            tile_y: 0.0,
            orientation: 0.0,
            status: 0,
            animation_progress: 0.0,
            health_ratio: 0.0,
        }; 64];
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
        let mut buffer = [tabletop::LiveEntity {
            entity_type: 0,
            tile_x: 0.0,
            tile_y: 0.0,
            orientation: 0.0,
            status: 0,
            animation_progress: 0.0,
            health_ratio: 0.0,
        }; 64];
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
}
