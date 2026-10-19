// lib.rs — Punto de entrada para XFactorVR (libxfactorvr.so)
//
// Inyectado en Factorio vía LD_PRELOAD para proporcionar la experiencia
// Tabletop Mixed Reality en Pico 4 Ultra Standalone / OpenXR.

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

/// Registra o actualiza una entidad viva (llamado desde hooks o UI de colocación 6DoF)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_place_or_update_entity(
    entity_type: u32,
    tile_x: f32,
    tile_y: f32,
    orientation: f32,
) -> bool {
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
    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        if let Some(pos) = entities.iter().position(|e| (e.tile_x - tile_x).abs() < 1.0 && (e.tile_y - tile_y).abs() < 1.0) {
            entities.remove(pos);
            true
        } else {
            false
        }
    } else {
        false
    }
}

/// Actualiza el ciclo de simulación y animaciones continuas de las entidades vivas
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_update_simulation(delta_time: f32) {
    let lock = get_live_entities_lock();
    if let Ok(mut entities) = lock.write() {
        for entity in entities.iter_mut() {
            if entity.status == 1 {
                let speed = match entity.entity_type {
                    tabletop::ENTITY_ASSEMBLING_MACHINE => 1.2,
                    tabletop::ENTITY_MINING_DRILL => 0.8,
                    tabletop::ENTITY_RADAR => 0.5,
                    tabletop::ENTITY_GUN_TURRET => 0.4,
                    tabletop::ENTITY_INSERTER => 1.5,
                    _ => 0.2,
                };
                entity.animation_progress = (entity.animation_progress + delta_time * speed).rem_euclid(1.0);
            }
        }
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
