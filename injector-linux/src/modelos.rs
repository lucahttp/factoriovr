//! Donde estan los modelos 3D y que dice el manifiesto de ellos.
//!
//! # Variable de entorno
//!
//! `XFACTORVR_MODELOS_DIR` = ruta **absoluta** al directorio que contiene el
//! `manifest.json` y los `.glb` (los mismos ficheros que se empaquetan en
//! `android/src/main/assets/models/`).  Ejemplos:
//!
//! ```text
//! XFACTORVR_MODELOS_DIR=/sdcard/XFactorVR/assets/models
//! XFACTORVR_MODELOS_DIR=/factorio/assets/models
//! ```
//!
//! Si no esta puesta, se buscan los sitios de siempre (los del APK en el
//! Android y el contenedor), en este orden:
//!
//!   1. `/sdcard/Android/data/com.xfactorvr.tabletop/files/models`
//!   2. `/storage/emulated/0/Android/data/com.xfactorvr.tabletop/files/models`
//!   3. `/data/data/com.xfactorvr.tabletop/files/models`
//!   4. `/data/user/0/com.xfactorvr.tabletop/files/models`
//!   5. `/sdcard/XFactorVR/assets/models`
//!   6. `/storage/emulated/0/XFactorVR/assets/models`
//!   7. `/data/local/tmp/XFactorVR/assets/models`
//!   8. `/factorio/assets/models` (el contenedor del juego)
//!   9. `models` (relativo al directorio de trabajo)
//!
//! Un directorio cuenta como valido si tiene `manifest.json`.
//!
//! # Manifiesto
//!
//! Lo escribe `src/bin/optimizar_glb.rs` (el generador del APK) y es el
//! mismo fichero que lee el visor (`src/modelos3d.rs`).  Las claves de
//! `modelos` son **nombres de prototipo** del Factorio ("assembling-machine-1",
//! "stone-furnace"...), asi que el inyector puede mapear directamente de la
//! entidad a su modelo.

use std::path::{Path, PathBuf};

use crate::json::Json;

/// Variable de entorno con la ruta de los modelos (la que hay que documentar).
pub const VAR_MODELOS: &str = "XFACTORVR_MODELOS_DIR";

/// Variable de entorno alternativa (la misma que usa el crate del visor).
pub const VAR_MODELOS_ALT: &str = "XFACTORVR_MODELS_DIR";

/// Como gira una parte del modelo respecto a la entidad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Giro {
    /// Sigue la direccion de la entidad (ensambladora, horno, poste...).
    Direccion,
    /// No gira (la base de la torreta).
    Fijo,
    /// Gira con la orientacion "suave" (la cabeza de la torreta).
    Orientacion,
    /// Gira solo (el plato del radar).
    Giro,
}

impl Giro {
    /// Parsea el texto del manifiesto.
    pub fn de_texto(s: &str) -> Giro {
        match s {
            "fijo" => Giro::Fijo,
            "orientacion" => Giro::Orientacion,
            "giro" => Giro::Giro,
            _ => Giro::Direccion,
        }
    }
}

/// Una parte (un GLB) de un modelo.
#[derive(Clone, Debug)]
pub struct Parte {
    /// Nombre del fichero dentro del directorio de modelos.
    pub archivo: String,
    /// Como gira esta parte.
    pub giro: Giro,
}

/// Lo que el manifiesto dice de un prototipo.
#[derive(Clone, Debug)]
pub struct InfoModelo {
    /// Nombre del prototipo ("assembling-machine-1").
    pub prototipo: String,
    /// Huella del prototipo en casillas (ancho del edificio).
    pub huella_tiles: f32,
    /// Partes (GLB) del modelo.
    pub partes: Vec<Parte>,
}

/// El manifiesto de modelos entero.
#[derive(Clone, Debug, Default)]
pub struct Manifiesto {
    /// Metros por casilla (a escala de referencia).
    pub escala_tile_m: f32,
    /// Modelos por nombre de prototipo.
    pub modelos: Vec<InfoModelo>,
}

impl Manifiesto {
    /// Busca el modelo de un prototipo.
    pub fn modelo(&self, prototipo: &str) -> Option<&InfoModelo> {
        self.modelos.iter().find(|m| m.prototipo == prototipo)
    }
}

/// Directorio donde se leen los GLB.
///
/// `None` si no se encuentra ninguno con `manifest.json` dentro.
pub fn dir_modelos() -> Option<PathBuf> {
    for var in [VAR_MODELOS, VAR_MODELOS_ALT] {
        if let Ok(d) = std::env::var(var) {
            let d = d.trim();
            if !d.is_empty() {
                return Some(PathBuf::from(d));
            }
        }
    }
    for c in candidatos() {
        let p = Path::new(c);
        if p.join("manifest.json").exists() {
            return Some(p.to_path_buf());
        }
    }
    None
}

/// Los sitios donde se buscan los modelos si no hay variable de entorno.
pub fn candidatos() -> &'static [&'static str] {
    &[
        "/sdcard/Android/data/com.xfactorvr.tabletop/files/models",
        "/storage/emulated/0/Android/data/com.xfactorvr.tabletop/files/models",
        "/data/data/com.xfactorvr.tabletop/files/models",
        "/data/user/0/com.xfactorvr.tabletop/files/models",
        "/sdcard/XFactorVR/assets/models",
        "/storage/emulated/0/XFactorVR/assets/models",
        "/data/local/tmp/XFactorVR/assets/models",
        "/factorio/assets/models",
        "models",
        "android/src/main/assets/models",
    ]
}

/// Lee el manifiesto de un directorio.
///
/// `None` si no hay `manifest.json` o si esta mal.
pub fn cargar_manifesto(dir: &Path) -> Option<Manifiesto> {
    let bytes = std::fs::read(dir.join("manifest.json")).ok()?;
    let texto = String::from_utf8_lossy(&bytes);
    let raiz = crate::json::parse(texto.as_bytes())?;
    Some(manifiesto_de_json(&raiz))
}

/// Convierte el JSON del manifiesto en la estructura.
pub fn manifiesto_de_json(raiz: &Json) -> Manifiesto {
    let mut m = Manifiesto {
        escala_tile_m: raiz
            .get("escala_tile_m")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.04) as f32,
        modelos: Vec::new(),
    };
    let modelos = match raiz.get("modelos").and_then(|v| v.as_obj()) {
        Some(v) => v,
        None => return m,
    };
    for (prototipo, valor) in modelos {
        let huella = valor
            .get("huella_tiles")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        let mut partes = Vec::new();
        if let Some(lista) = valor.get("partes").and_then(|v| v.as_arr()) {
            for p in lista {
                let archivo = match p.get("archivo").and_then(|v| v.as_str()) {
                    Some(a) => a.to_string(),
                    None => continue,
                };
                let giro = Giro::de_texto(p.get("giro").and_then(|v| v.as_str()).unwrap_or("direccion"));
                partes.push(Parte { archivo, giro });
            }
        }
        m.modelos.push(InfoModelo {
            prototipo: prototipo.clone(),
            huella_tiles: huella,
            partes,
        });
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Las pruebas que tocan el entorno se serializan con este candado.
    static LOCK_ENV: Mutex<()> = Mutex::new(());

    #[test]
    fn el_manifiesto_real_se_lee() {
        let ruta = format!(
            "{}/../android/src/main/assets/models/manifest.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let dir = Path::new(&ruta).parent().expect("sin directorio");
        let m = match cargar_manifesto(dir) {
            Some(m) => m,
            None => return, // el repo no esta a mano: el test no dice nada
        };
        assert!(m.modelos.len() >= 20, "faltan modelos en el manifiesto");
        let am = m.modelo("assembling-machine-1").expect("falta assembling-machine-1");
        assert_eq!(am.huella_tiles, 3.0);
        assert_eq!(am.partes.len(), 1);
        assert_eq!(am.partes[0].archivo, "assembling-machine-1.glb");
        assert_eq!(am.partes[0].giro, Giro::Direccion);
        // Un modelo de dos partes con giros distintos.
        let radar = m.modelo("radar").expect("falta radar");
        assert_eq!(radar.partes.len(), 2);
        assert_eq!(radar.partes[1].giro, Giro::Giro);
        // La torreta tiene la base fija y la cabeza con orientacion.
        let torreta = m.modelo("gun-turret").expect("falta gun-turret");
        assert_eq!(torreta.partes[0].giro, Giro::Fijo);
        assert_eq!(torreta.partes[1].giro, Giro::Orientacion);
    }

    #[test]
    fn la_variable_de_entorno_gana() {
        let _g = LOCK_ENV.lock().unwrap_or_else(|p| p.into_inner());
        // Se prueba la logica con una ruta inventada: la variable se lee
        // siempre, aunque el directorio no exista (quien la pone sabe lo que
        // hace; si no existe, no habra modelos y no se dibuja nada).
        let ruta = PathBuf::from("/tmp/xfv_modelos_de_prueba");
        std::env::set_var(VAR_MODELOS, &ruta);
        assert_eq!(dir_modelos(), Some(ruta));
        std::env::remove_var(VAR_MODELOS);
    }

    #[test]
    fn el_registro_encuentra_los_prototipos_del_manifiesto() {
        let _g = LOCK_ENV.lock().unwrap_or_else(|p| p.into_inner());
        let dir = format!(
            "{}/../android/src/main/assets/models",
            env!("CARGO_MANIFEST_DIR")
        );
        if !Path::new(&dir).join("manifest.json").exists() {
            return; // el repo no esta a mano
        }
        std::env::set_var(VAR_MODELOS, &dir);
        // El registro de modelos 3D lee el manifiesto y sabe que prototipo
        // corresponde a cada indice.
        crate::render3d::configurar();
        let i = crate::render3d::indice_de_prototipo("assembling-machine-1");
        assert!(i.is_some(), "no se encontro assembling-machine-1");
        // Un prototipo que no tiene modelo no se registra.
        assert!(crate::render3d::indice_de_prototipo("no-existe-nada").is_none());
        // Y dos llamadas del mismo prototipo dan el mismo indice.
        let j = crate::render3d::indice_de_prototipo("assembling-machine-1").unwrap();
        assert_eq!(i, Some(j));
        std::env::remove_var(VAR_MODELOS);
    }

    #[test]
    fn el_manifiesto_mal_no_entra() {
        assert!(cargar_manifesto(Path::new("/no/existe/este/directorio")).is_none());
    }
}
