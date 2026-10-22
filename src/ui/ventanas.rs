//! ventanas.rs — Las VENTANAS espaciales del HUD de Factorio VR.
//!
//! En vez de un panel gigante con tres sub-paneles dibujados a mano, el HUD
//! son cuatro ventanas en el espacio: CONSTRUIR, INVENTARIO, FABRICAR y VISTA.
//! Cada una tiene su propio plano (posición, rotación y escala), mira al
//! usuario y se puede agarrar con una o dos manos.
//!
//! La idea viene de `XrSurface.h` de GZH-XR: la superficie es un OBJETO
//! manipulable. Aquí cada ventana es ese objeto: la agarras, la mueves, y con
//! las dos manos la escalas (con límites, para que no se vuelva un cartel
//! gigante ni desaparezca).
//!
//! Cuidado con el salto al pasar de una mano a dos: al agarrar con la segunda
//! mano se RECAPTURA la pose base (centro y distancia) sin tocar el ancho, de
//! forma que la ventana no pega un tirón en ese instante.

use glam::{Quat, Vec3};

use super::tabla::{Celda, Color, Primitiva};

/// Identificador de ventana del HUD.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdVentana {
    Construir,
    Inventario,
    Fabricar,
    Vista,
}

impl IdVentana {
    pub const TODAS: [IdVentana; 4] = [
        IdVentana::Construir,
        IdVentana::Inventario,
        IdVentana::Fabricar,
        IdVentana::Vista,
    ];

    /// Título corto de la ventana (sale en la barra naranja).
    pub fn titulo(self) -> &'static str {
        match self {
            IdVentana::Construir => "CONSTRUIR",
            IdVentana::Inventario => "INVENTARIO",
            IdVentana::Fabricar => "FABRICAR",
            IdVentana::Vista => "VISTA",
        }
    }
}

/// Anclaje de la ventana en el espacio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anclaje {
    /// Fija al mundo (a la mesa): no se mueve con la cabeza.
    Mundo,
    /// Pegada a la cabeza: sigue al jugador.
    Cabeza,
}

/// Plano de la ventana en el mundo: dónde está y hacia dónde mira.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plano {
    /// Posición del centro de la ventana, en metros (espacio local XR).
    pub posicion: Vec3,
    /// Rotación (cuaternión). La ventana mira por su -Z local.
    pub rotacion: Quat,
    /// Escala global (1.0 = tamaño natural).
    pub escala: f32,
}

impl Default for Plano {
    fn default() -> Self {
        Self {
            posicion: Vec3::ZERO,
            rotacion: Quat::IDENTITY,
            escala: 1.0,
        }
    }
}

/// Límites de tamaño de una ventana en metros de ancho.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Limites {
    pub ancho_min_m: f32,
    pub ancho_max_m: f32,
}

impl Default for Limites {
    fn default() -> Self {
        Self {
            ancho_min_m: 0.25,
            ancho_max_m: 1.50,
        }
    }
}

/// Una ventana del HUD.
#[derive(Clone, Debug, PartialEq)]
pub struct Ventana {
    pub id: IdVentana,
    pub titulo: String,
    /// Tamaño en píxeles de la textura de la ventana.
    pub ancho_px: u32,
    pub alto_px: u32,
    /// Ancho en metros (el alto sale de la relación de aspecto de los píxeles).
    pub ancho_m: f32,
    pub plano: Plano,
    pub anclaje: Anclaje,
    /// ¿Se puede agarrar y mover con las manos?
    pub agarrable: bool,
}

impl Ventana {
    /// Alto en metros, derivado del ancho y de la relación de aspecto.
    pub fn alto_m(&self) -> f32 {
        if self.ancho_px == 0 {
            0.0
        } else {
            self.ancho_m * self.alto_px as f32 / self.ancho_px as f32
        }
    }

    /// Píxeles por metro: cómo de grande se ve un píxel de la textura.
    pub fn px_por_m(&self) -> f32 {
        if self.ancho_m <= 0.0 {
            1.0
        } else {
            self.ancho_px as f32 / self.ancho_m
        }
    }

    /// Cuantos píxeles de la ventana ocupa un punto del mundo dado en metros
    /// local (u, v) del plano. `u` y `v` van de 0 a 1.
    pub fn uv_a_pixeles(&self, u: f32, v: f32) -> (f32, f32) {
        (u * self.ancho_px as f32, v * self.alto_px as f32)
    }
}

/// Manos que pueden agarrar una ventana.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mano {
    Izquierda,
    Derecha,
}

/// Estado de agarre de una ventana.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Agarre {
    /// Nadie la agarra.
    Libre,
    /// Una mano la mueve.
    UnaMano {
        mano: Mano,
        /// Distancia de la mano al centro de la ventana al agarrarla (para
        /// que no salte al cogerla).
        desplazamiento: Vec3,
    },
    /// Dos manos la mueven y escalan.
    DosManos {
        /// Distancia entre las manos al pasar a dos manos (base de escala).
        base_distancia: f32,
        /// Ancho en metros en el momento de pasar a dos manos.
        base_ancho_m: f32,
        /// Centro entre las manos en ese momento.
        base_centro: Vec3,
    },
}

/// Conjunto de ventanas del HUD y sus agarres.
#[derive(Clone, Debug)]
pub struct Ventanas {
    ventanas: Vec<Ventana>,
    agarres: Vec<(IdVentana, Agarre)>,
    pub limites: Limites,
}

impl Default for Ventanas {
    fn default() -> Self {
        Self::por_defecto()
    }
}

impl Ventanas {
    /// Distribución por defecto: cada ventana en SU plano, mirando al usuario,
    /// con un tamaño que se lee a distancia.
    ///
    /// - INVENTARIO: a la izquierda, cerca (se consulta mucho).
    /// - CONSTRUIR: al centro-derecha, un poco más lejos (se elige una máquina).
    /// - FABRICAR: a la derecha, más lejos.
    /// - VISTA: al frente, arriba de la mesa (es el mapa del juego).
    pub fn por_defecto() -> Self {
        let ventanas = vec![
            Ventana {
                id: IdVentana::Inventario,
                titulo: "INVENTARIO".to_string(),
                ancho_px: 512,
                alto_px: 320,
                ancho_m: 0.55,
                plano: Plano {
                    posicion: Vec3::new(-0.55, 1.35, -0.90),
                    rotacion: Quat::from_rotation_y(0.55),
                    escala: 1.0,
                },
                anclaje: Anclaje::Cabeza,
                agarrable: true,
            },
            Ventana {
                id: IdVentana::Construir,
                titulo: "CONSTRUIR".to_string(),
                ancho_px: 512,
                alto_px: 320,
                ancho_m: 0.70,
                plano: Plano {
                    posicion: Vec3::new(0.55, 1.35, -0.95),
                    rotacion: Quat::from_rotation_y(-0.55),
                    escala: 1.0,
                },
                anclaje: Anclaje::Cabeza,
                agarrable: true,
            },
            Ventana {
                id: IdVentana::Fabricar,
                titulo: "FABRICAR".to_string(),
                ancho_px: 512,
                alto_px: 320,
                ancho_m: 0.55,
                plano: Plano {
                    posicion: Vec3::new(1.05, 1.30, -0.45),
                    rotacion: Quat::from_rotation_y(-1.05),
                    escala: 1.0,
                },
                anclaje: Anclaje::Cabeza,
                agarrable: true,
            },
            Ventana {
                id: IdVentana::Vista,
                titulo: "VISTA".to_string(),
                ancho_px: 512,
                alto_px: 320,
                ancho_m: 0.90,
                plano: Plano {
                    posicion: Vec3::new(0.0, 1.60, -1.60),
                    rotacion: Quat::IDENTITY,
                    escala: 1.0,
                },
                anclaje: Anclaje::Mundo,
                agarrable: true,
            },
        ];
        Self {
            ventanas,
            agarres: Vec::new(),
            limites: Limites::default(),
        }
    }

    pub fn ventana(&self, id: IdVentana) -> Option<&Ventana> {
        self.ventanas.iter().find(|v| v.id == id)
    }

    pub fn ventana_mut(&mut self, id: IdVentana) -> Option<&mut Ventana> {
        self.ventanas.iter_mut().find(|v| v.id == id)
    }

    pub fn ventanas(&self) -> &[Ventana] {
        &self.ventanas
    }

    /// Estado de agarre actual de una ventana.
    pub fn agarre(&self, id: IdVentana) -> Agarre {
        self.agarres
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, a)| *a)
            .unwrap_or(Agarre::Libre)
    }

    /// ¿Está la ventana agarrada?
    pub fn esta_agarrada(&self, id: IdVentana) -> bool {
        self.agarre(id) != Agarre::Libre
    }

    /// ¿Está la ventana agarrada con dos manos?
    pub fn esta_agarrada_dos_manos(&self, id: IdVentana) -> bool {
        matches!(self.agarre(id), Agarre::DosManos { .. })
    }

    /// Marca el arranque de un agarre con UNA mano.
    ///
    /// `posicion_mano` es la posición de la mano en el espacio; se guarda el
    /// desplazamiento respecto al centro de la ventana para que al mover la
    /// mano la ventana no salte.
    pub fn agarrar_una_mano(&mut self, id: IdVentana, mano: Mano, posicion_mano: Vec3) {
        let Some(v) = self.ventana_mut(id) else { return };
        if !v.agarrable {
            return;
        }
        let desplazamiento = v.plano.posicion - posicion_mano;
        self.set_agarre(
            id,
            Agarre::UnaMano {
                mano,
                desplazamiento,
            },
        );
    }

    /// Marca el arranque de un agarre con DOS manos.
    ///
    /// `distancia` es la distancia entre las manos y `centro` el punto medio.
    /// Aquí se recaptura la pose base: el ancho NO cambia en este instante
    /// (si no, al pasar de una mano a dos la ventana pegaría un tirón), a
    /// partir de ahora el escalado es relativo a esta distancia.
    pub fn agarrar_dos_manos(
        &mut self,
        id: IdVentana,
        distancia: f32,
        centro: Vec3,
    ) {
        let Some(v) = self.ventana_mut(id) else { return };
        if !v.agarrable {
            return;
        }
        if !distancia.is_finite() || distancia <= 0.0 {
            return;
        }
        let base_ancho_m = v.ancho_m;
        self.set_agarre(
            id,
            Agarre::DosManos {
                base_distancia: distancia,
                base_ancho_m,
                base_centro: centro,
            },
        );
    }

    /// Actualiza el agarre con dos manos: escala por la distancia y mueve con
    /// el centro, respetando los límites de tamaño.
    ///
    /// Devuelve el nuevo ancho en metros.
    pub fn actualizar_dos_manos(&mut self, id: IdVentana, distancia: f32, centro: Vec3) -> f32 {
        let agarres = self.agarre(id);
        let (min_m, max_m) = (self.limites.ancho_min_m, self.limites.ancho_max_m);
        let Some(v) = self.ventana_mut(id) else { return 0.0 };
        let Agarre::DosManos {
            base_distancia,
            base_ancho_m,
            base_centro,
        } = agarres
        else {
            return v.ancho_m;
        };
        if !distancia.is_finite() || distancia <= 0.0 || base_distancia <= 0.0 {
            return v.ancho_m;
        }
        let factor = distancia / base_distancia;
        let ancho = (base_ancho_m * factor).clamp(min_m, max_m);
        v.ancho_m = ancho;
        // La ventana sigue al centro de las manos, manteniendo el desplazamiento
        // que tenía respecto al centro en el momento de la captura.
        let delta = centro - base_centro;
        v.plano.posicion += delta;
        ancho
    }

    /// Actualiza el agarre con una mano: mueve la ventana con la mano.
    pub fn actualizar_una_mano(&mut self, id: IdVentana, posicion_mano: Vec3) {
        let agarres = self.agarre(id);
        let Some(v) = self.ventana_mut(id) else { return };
        let Agarre::UnaMano { desplazamiento, .. } = agarres else {
            return;
        };
        v.plano.posicion = posicion_mano + desplazamiento;
    }

    /// Suelta la ventana (el agarre se libera).
    pub fn soltar(&mut self, id: IdVentana) {
        self.set_agarre(id, Agarre::Libre);
    }

    /// Suelta todo.
    pub fn soltar_todas(&mut self) {
        self.agarres.clear();
    }

    fn set_agarre(&mut self, id: IdVentana, agarre: Agarre) {
        if let Some(slot) = self.agarres.iter_mut().find(|(i, _)| *i == id) {
            slot.1 = agarre;
        } else {
            self.agarres.push((id, agarre));
        }
    }

    /// Fija el ancho de una ventana respetando los límites.
    pub fn fijar_ancho(&mut self, id: IdVentana, ancho_m: f32) -> f32 {
        let l = self.limites;
        let Some(v) = self.ventana_mut(id) else { return 0.0 };
        v.ancho_m = ancho_m.clamp(l.ancho_min_m, l.ancho_max_m);
        v.ancho_m
    }

    /// Orienta la ventana hacia la cabeza del jugador (para que se lea).
    pub fn mirar_a(&mut self, id: IdVentana, cabeza: Vec3) {
        let Some(v) = self.ventana_mut(id) else { return };
        let dir = v.plano.posicion - cabeza;
        if dir.length_squared() < 1e-6 {
            return;
        }
        let yaw = (-dir.x).atan2(-dir.z);
        v.plano.rotacion = Quat::from_rotation_y(yaw);
    }

    /// Orientación de todas las ventanas hacia la cabeza.
    pub fn mirar_todas_a(&mut self, cabeza: Vec3) {
        for v in IdVentana::TODAS {
            self.mirar_a(v, cabeza);
        }
    }
}

// ---------------------------------------------------------------------------
// Primitivas del marco de la ventana (el "cristal" con la barra de título)
// ---------------------------------------------------------------------------

/// Dibuja el marco de la ventana: fondo, barra de título naranja y el texto
/// del título. Devuelve datos; el C++ los pinta.
pub fn primitivas_marco(v: &Ventana) -> Vec<Primitiva> {
    let mut out = Vec::new();
    let w = v.ancho_px as i32;
    let h = v.alto_px as i32;
    out.push(Primitiva::Rect {
        x: 0,
        y: 0,
        w,
        h,
        color: Color::FONDO,
    });
    out.push(Primitiva::Rect {
        x: 0,
        y: 0,
        w,
        h: 40,
        color: Color::TITULO,
    });
    let titulo = v.titulo.to_uppercase();
    let escala = 3;
    let ancho = super::tabla::ancho_texto(&titulo, escala);
    let x = (w - ancho) / 2;
    let celdas = super::tabla::celdas_5x7(&titulo, x, 12, escala, Color::rgb(20, 16, 12));
    out.push(Primitiva::Texto { celdas, escala });
    out
}

/// Etiqueta de "ventana agarrada" (una mano o dos).
pub fn primitivas_agarre(v: &Ventana, estado: Agarre) -> Vec<Primitiva> {
    let texto = match estado {
        Agarre::Libre => "",
        Agarre::UnaMano { .. } => "MOVER",
        Agarre::DosManos { .. } => "MOVER / ESCALAR",
    };
    if texto.is_empty() {
        return Vec::new();
    }
    let celdas = super::tabla::celdas_5x7(
        texto,
        v.ancho_px as i32 - 140,
        v.alto_px as i32 - 20,
        2,
        Color::BORDE_HOVER,
    );
    vec![Primitiva::Texto { celdas, escala: 2 }]
}

/// Reexportado para que el C++ no tenga que conocer el módulo de la tabla.
pub type CeldaTexto = Celda;
