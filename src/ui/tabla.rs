//! tabla.rs — La TABLA DE CONTROLES ÚNICA del HUD de Factorio VR.
//!
//! Idea copiada de GZH-XR (`GeneralsMD/Code/Main/XrPanelLayout.h`): una sola
//! tabla define a la vez **los píxeles, el hit-test y el estado** de cada
//! control. El C++ no dibuja ningún control por su cuenta: pide la lista de
//! primitivas que sale de aquí y, si quiere saber qué hay bajo el rayo, pregunta
//! a la MISMA tabla. Así los píxeles y las zonas de pulsación no pueden
//! desincronizarse ("pixels and hit regions cannot drift apart").
//!
//! Aquí no se dibuja nada: solo hay DATOS. Las primitivas de dibujo son una
//! lista de rectángulos, bordes, textos (en celdas de fuente bitmap 5x7) y
//! badges. El C++ las pinta; si luego se toca el C++ para pintar "bonito",
//! la tabla sigue mandando en lo que el jugador puede pulsar.
//!
//! Todos los píxeles son enteros y con la Y hacia abajo, como en GZH-XR.

use super::items::{nombre_item, ITEM_ASSEMBLING_MACHINE, ITEM_CHEST, ITEM_FURNACE,
    ITEM_MINING_DRILL, ITEM_POWER_POLE, ITEM_RADAR, ITEM_ROBOPORT, ITEM_TURRET};
use super::ventanas::IdVentana;

// ---------------------------------------------------------------------------
// Geometría
// ---------------------------------------------------------------------------

/// Rectángulo en píxeles dentro de su ventana (origen arriba-izquierda).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// ¿El punto (en píxeles de la ventana) cae dentro? Borde derecho e
    /// inferior exclusive: dos controles contiguos NO se pisan.
    pub fn contiene(&self, x: f32, y: f32) -> bool {
        x >= self.x as f32
            && x < (self.x + self.w) as f32
            && y >= self.y as f32
            && y < (self.y + self.h) as f32
    }
}

// ---------------------------------------------------------------------------
// Rol y estado
// ---------------------------------------------------------------------------

/// Para qué sirve el control. Solo algunos roles se pueden pulsar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rol {
    /// Botón normal ("Ensambladora").
    Boton,
    /// Cabecera de sección: "MAQUINAS". No se pulsa.
    Seccion,
    /// Pestaña de página.
    Pestana,
    /// Tarjeta grande (una opción que ocupa mucho).
    Tarjeta,
    /// Tarjeta de estado/contexto: no se pulsa.
    Contexto,
    /// Operación de grupo ("Guardar vista").
    Operacion,
    /// Acción destructiva ("Demoler"): se pinta en rojo.
    Peligro,
    /// Línea de información.
    Info,
}

impl Rol {
    /// ¿El control recibe pulsaciones? (GZH-XR: `xrPanelRoleHittable`.)
    pub fn es_pulsable(self) -> bool {
        matches!(
            self,
            Rol::Boton | Rol::Pestana | Rol::Tarjeta | Rol::Operacion | Rol::Peligro
        )
    }
}

/// Estado visual y semántico del control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Estado {
    /// Normal.
    Reposo,
    /// El rayo está encima.
    Hover,
    /// Elegido por el jugador.
    Seleccionado,
    /// Armado: la acción está preparada, pero aún no se ha soltado.
    Armado,
    /// Operación pendiente (falta un paso).
    Pendiente,
    /// No disponible: NO recibe pulsación.
    Desactivado,
    /// Activo / encendido (una pestaña activa, un interruptor ON).
    Activo,
}

impl Estado {
    /// Un control desactivado no se puede pulsar (ni aunque el rayo lo toque).
    pub fn acepta_pulsacion(self) -> bool {
        !matches!(self, Estado::Desactivado)
    }
}

/// Qué quiere el jugador que pase al pulsar este control.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accion {
    /// Nada (decoración).
    Ninguna,
    /// Poner una máquina del item_id indicado en modo construcción.
    Construir(u32),
    /// Fabricar el item_id indicado.
    Fabricar(u32),
    /// Mirar un item del inventario (paracipes lo muestra en grande).
    ItemInventario(u32),
    /// Cerrar la ventana.
    Cerrar,
    /// Cambiar a la pestaña indicada (0 = construir, 1 = inventario...).
    Pestana(u8),
    /// Mirar la vista del juego.
    Mirar,
    /// Demoler lo que hay bajo el rayo.
    Demoler,
    /// Mover la ventana agarrándola.
    MoverVentana,
    /// Rotar la ventana.
    RotarVentana,
}

// ---------------------------------------------------------------------------
// Control
// ---------------------------------------------------------------------------

/// Un control de la tabla. La geometría es la MISMA que se usa para pintar.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    /// Id estable y único dentro de la ventana. Lo usa el C++ para saber
    /// qué ha pulsado el jugador sin mirar la acción.
    pub id: u32,
    /// Ventana a la que pertenece.
    pub ventana: IdVentana,
    /// Rol (qué pinta y si se puede pulsar).
    pub rol: Rol,
    /// Acción al pulsar.
    pub accion: Accion,
    /// Texto visible (mayúsculas, corto: se lee a distancia).
    pub etiqueta: String,
    /// Estado actual.
    pub estado: Estado,
    /// Rectángulo en píxeles dentro de la ventana.
    pub rect: Rect,
    /// Badge opcional (cantidad, coste...) que va en la esquina.
    pub badge: Option<String>,
}

impl Control {
    /// ¿Este control se puede pulsar ahora mismo?
    pub fn pulsable(&self) -> bool {
        self.rol.es_pulsable() && self.estado.acepta_pulsacion()
    }
}

// ---------------------------------------------------------------------------
// Datos de las ventanas (inventario y recetas)
// ---------------------------------------------------------------------------

/// Un item del inventario del jugador, resumido para el HUD.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemResumen {
    pub item_id: u32,
    pub nombre: String,
    pub cantidad: u32,
}

/// Una receta fabricable.
#[derive(Clone, Debug, PartialEq)]
pub struct Receta {
    pub item_id: u32,
    pub nombre: String,
    /// Coste: (item_id, cantidad).
    pub coste: Vec<(u32, u32)>,
    /// Cuántas unidades salen por aplicación.
    pub cantidad: u32,
    /// ¿Se puede fabricar ahora mismo?
    pub disponible: bool,
}

// ---------------------------------------------------------------------------
// La tabla
// ---------------------------------------------------------------------------

/// La tabla de controles del HUD completo (las cuatro ventanas).
///
/// Es la ÚNICA fuente de geometría: de aquí salen los píxeles que se pintan y
/// las zonas que se pueden pulsar.
#[derive(Clone, Debug, Default)]
pub struct Tabla {
    pub controles: Vec<Control>,
}

impl Tabla {
    pub fn nueva() -> Self {
        Self::default()
    }

    /// Construye la tabla de por defecto: las cuatro ventanas del HUD.
    ///
    /// - `inventario`: lo que lleva el jugador (se ordena por cantidad).
    /// - `recetas`: lo que se puede fabricar.
    pub fn por_defecto(inventario: &[ItemResumen], recetas: &[Receta]) -> Self {
        let mut t = Self::nueva();
        t.construir_ventana_principal();
        t.construir_ventana_inventario(inventario);
        t.construir_ventana_fabricar(recetas);
        t.construir_ventana_vista();
        t
    }

    fn push(&mut self, control: Control) {
        debug_assert!(
            !self
                .controles
                .iter()
                .any(|c| c.ventana == control.ventana && c.id == control.id),
            "id de control duplicado en la misma ventana"
        );
        self.controles.push(control);
    }

    /// Ventana CONSTRUIR: elegir máquina. Ocho máquinas + demoler.
    ///
    /// Grid de 3 columnas: se lee mejor que una fila larga de 18 botones
    /// (que es justo lo que NO queremos copiar de GZH-XR).
    fn construir_ventana_principal(&mut self) {
        const MAQUINAS: &[(u32, &str)] = &[
            (ITEM_ASSEMBLING_MACHINE, "ENSAMBLADORA"),
            (ITEM_MINING_DRILL, "MINERA"),
            (ITEM_FURNACE, "HORNO"),
            (ITEM_TURRET, "TORRETA"),
            (ITEM_RADAR, "RADAR"),
            (ITEM_ROBOPORT, "ROBOPUERTO"),
            (ITEM_CHEST, "COFRE"),
            (ITEM_POWER_POLE, "POSTE"),
        ];

        self.push(Control {
            id: 1,
            ventana: IdVentana::Construir,
            rol: Rol::Seccion,
            accion: Accion::Ninguna,
            etiqueta: "ELIGE UNA MAQUINA".to_string(),
            estado: Estado::Reposo,
            rect: Rect::new(16, 48, 480, 24),
            badge: None,
        });

        let bx = [16i32, 176, 336];
        let by = [80i32, 144, 208];
        for (i, (item_id, nombre)) in MAQUINAS.iter().enumerate() {
            self.push(Control {
                id: 10 + i as u32,
                ventana: IdVentana::Construir,
                rol: Rol::Boton,
                accion: Accion::Construir(*item_id),
                etiqueta: nombre.to_string(),
                estado: Estado::Reposo,
                rect: Rect::new(bx[i % 3], by[i / 3], 152, 56),
                badge: None,
            });
        }

        // Acción destructiva, aparte y en rojo.
        self.push(Control {
            id: 30,
            ventana: IdVentana::Construir,
            rol: Rol::Peligro,
            accion: Accion::Demoler,
            etiqueta: "DEMOLER".to_string(),
            estado: Estado::Reposo,
            rect: Rect::new(16, 272, 152, 32),
            badge: None,
        });
        self.push(Control {
            id: 31,
            ventana: IdVentana::Construir,
            rol: Rol::Contexto,
            accion: Accion::Ninguna,
            etiqueta: "COLOCA LA MAQUINA EN EL SUELO".to_string(),
            estado: Estado::Reposo,
            rect: Rect::new(176, 272, 320, 32),
            badge: None,
        });
    }

    /// Ventana INVENTARIO: lo que llevas, con la cantidad en badge.
    fn construir_ventana_inventario(&mut self, inventario: &[ItemResumen]) {
        self.push(Control {
            id: 1,
            ventana: IdVentana::Inventario,
            rol: Rol::Seccion,
            accion: Accion::Ninguna,
            etiqueta: "EN TU BOLSA".to_string(),
            estado: Estado::Reposo,
            rect: Rect::new(16, 48, 480, 24),
            badge: None,
        });

        let mut items: Vec<&ItemResumen> = inventario.iter().collect();
        items.sort_by(|a, b| b.cantidad.cmp(&a.cantidad));
        for (i, item) in items.iter().take(6).enumerate() {
            let y = 80 + i as i32 * 36;
            self.push(Control {
                id: 10 + i as u32,
                ventana: IdVentana::Inventario,
                rol: Rol::Tarjeta,
                accion: Accion::ItemInventario(item.item_id),
                etiqueta: item.nombre.to_uppercase(),
                estado: Estado::Reposo,
                rect: Rect::new(16, y, 480, 32),
                badge: Some(format!("x{}", item.cantidad)),
            });
        }
        if items.is_empty() {
            self.push(Control {
                id: 90,
                ventana: IdVentana::Inventario,
                rol: Rol::Info,
                accion: Accion::Ninguna,
                etiqueta: "LA BOLSA ESTA VACIA".to_string(),
                estado: Estado::Reposo,
                rect: Rect::new(16, 80, 480, 32),
                badge: None,
            });
        }
    }

    /// Ventana FABRICAR: recetas pocas y grandes.
    fn construir_ventana_fabricar(&mut self, recetas: &[Receta]) {
        self.push(Control {
            id: 1,
            ventana: IdVentana::Fabricar,
            rol: Rol::Seccion,
            accion: Accion::Ninguna,
            etiqueta: "FABRICAR".to_string(),
            estado: Estado::Reposo,
            rect: Rect::new(16, 48, 480, 24),
            badge: None,
        });

        for (i, receta) in recetas.iter().take(4).enumerate() {
            let y = 80 + i as i32 * 56;
            let coste = receta
                .coste
                .iter()
                .map(|(id, n)| format!("{} x{}", nombre_item(*id), n))
                .collect::<Vec<_>>()
                .join(" + ");
            self.push(Control {
                id: 10 + i as u32,
                ventana: IdVentana::Fabricar,
                rol: Rol::Boton,
                accion: Accion::Fabricar(receta.item_id),
                etiqueta: receta.nombre.to_uppercase(),
                estado: if receta.disponible {
                    Estado::Reposo
                } else {
                    Estado::Desactivado
                },
                rect: Rect::new(16, y, 480, 48),
                badge: Some(coste),
            });
        }
    }

    /// Ventana VISTA: mirar, centrar yDimensionar el mapa.
    fn construir_ventana_vista(&mut self) {
        let botones: &[(u32, Accion, &str)] = &[
            (10, Accion::Mirar, "MIRAR MAPA"),
            (11, Accion::MoverVentana, "CENTRAR"),
            (12, Accion::RotarVentana, "GIRAR"),
        ];
        for (i, (id, accion, texto)) in botones.iter().enumerate() {
            self.push(Control {
                id: *id,
                ventana: IdVentana::Vista,
                rol: Rol::Boton,
                accion: *accion,
                etiqueta: texto.to_string(),
                estado: Estado::Reposo,
                rect: Rect::new(16 + i as i32 * 122, 64, 114, 48),
                badge: None,
            });
        }
        // La pista del modo actual vive aquí: es lo que el jugador lee para
        // saber qué va a pasar. El texto lo cambia el motor de estados.
        self.push(Control {
            id: 50,
            ventana: IdVentana::Vista,
            rol: Rol::Contexto,
            accion: Accion::Ninguna,
            etiqueta: "MODO NORMAL".to_string(),
            estado: Estado::Reposo,
            rect: Rect::new(16, 124, 348, 56),
            badge: None,
        });
    }

    // -----------------------------------------------------------------------
    // Consultas
    // -----------------------------------------------------------------------

    /// Control por id dentro de una ventana.
    pub fn control(&self, ventana: IdVentana, id: u32) -> Option<&Control> {
        self.controles
            .iter()
            .find(|c| c.ventana == ventana && c.id == id)
    }

    /// Control mutable por id dentro de una ventana.
    pub fn control_mut(&mut self, ventana: IdVentana, id: u32) -> Option<&mut Control> {
        self.controles
            .iter_mut()
            .find(|c| c.ventana == ventana && c.id == id)
    }

    /// Controles de una ventana, en orden de dibujo.
    pub fn controles_de(&self, ventana: IdVentana) -> impl Iterator<Item = &Control> {
        self.controles.iter().filter(move |c| c.ventana == ventana)
    }

    /// ¿Hay algún control pulsable bajo el punto (píxeles de la ventana)?
    ///
    /// SOLO se usa la tabla: no hay geometría aparte. Los huecos son terreno
    /// muerto (nada que pulsar).
    pub fn hit_test(&self, ventana: IdVentana, x: f32, y: f32) -> Option<&Control> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        self.controles_de(ventana)
            .find(|c| c.pulsable() && c.rect.contiene(x, y))
    }

    /// Hit-test con el origen de la ventana en píxeles absolutos (el C++ sabe
    /// dónde está la ventana en la pantalla) y un punto absoluto.
    pub fn hit_test_abs(
        &self,
        ventana: IdVentana,
        origen: (f32, f32),
        punto: (f32, f32),
    ) -> Option<&Control> {
        self.hit_test(
            ventana,
            punto.0 - origen.0,
            punto.1 - origen.1,
        )
    }

    /// ¿Se puede pulsar este control ahora? (desactivado = no).
    pub fn puede_pulsar(&self, ventana: IdVentana, id: u32) -> bool {
        self.control(ventana, id)
            .map(|c| c.pulsable())
            .unwrap_or(false)
    }

    /// Cambia el estado de un control (hover, seleccionado...).
    pub fn set_estado(&mut self, ventana: IdVentana, id: u32, estado: Estado) {
        if let Some(c) = self.control_mut(ventana, id) {
            c.estado = estado;
        }
    }

    /// Cambia el texto de un control (pista del modo, nombre de un item...).
    pub fn set_etiqueta(&mut self, ventana: IdVentana, id: u32, etiqueta: impl Into<String>) {
        if let Some(c) = self.control_mut(ventana, id) {
            c.etiqueta = etiqueta.into().to_uppercase();
        }
    }

    /// Quita el estado Hover de todos los controles salvo el indicado.
    pub fn limpiar_hover(&mut self, ventana: IdVentana, excepto: Option<u32>) {
        for c in self.controles.iter_mut().filter(|c| c.ventana == ventana) {
            if c.estado == Estado::Hover && Some(c.id) != excepto {
                c.estado = Estado::Reposo;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Primitivas de dibujo (datos, no dibujo)
// ---------------------------------------------------------------------------

/// Color RGBA de 8 bits por canal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    /// Fondo del cristal oscuro del HUD.
    pub const FONDO: Color = Color::rgba(14, 20, 26, 235);
    /// Naranja Factorio de la barra de título.
    pub const TITULO: Color = Color::rgb(255, 148, 30);
    /// Texto normal.
    pub const TEXTO: Color = Color::rgb(232, 240, 246);
    /// Texto atenuado (secciones).
    pub const TEXTO_TENUE: Color = Color::rgb(150, 165, 178);
    /// Botón normal.
    pub const BOTON: Color = Color::rgba(26, 38, 50, 230);
    /// Botón con el rayo encima.
    pub const BOTON_HOVER: Color = Color::rgba(44, 66, 86, 240);
    /// Botón seleccionado / armado.
    pub const BOTON_ACTIVO: Color = Color::rgba(70, 104, 46, 240);
    /// Acción destructiva.
    pub const PELIGRO: Color = Color::rgba(120, 32, 28, 235);
    /// Contorno.
    pub const BORDE: Color = Color::rgba(120, 145, 165, 180);
    /// Borde del control con el rayo encima.
    pub const BORDE_HOVER: Color = Color::rgb(255, 200, 90);
}

/// Una celda de la fuente bitmap 5x7: 7 filas de 5 píxeles (bit 4 = izquierda).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Celda {
    /// X en píxeles de la celda dentro de la ventana.
    pub x: i32,
    /// Y en píxeles de la celda dentro de la ventana.
    pub y: i32,
    /// Las 7 filas de 5 bits.
    pub filas: [u8; 7],
    /// Color del texto.
    pub color: Color,
}

/// Una primitiva de dibujo. El C++ las pinta y ya.
#[derive(Clone, Debug, PartialEq)]
pub enum Primitiva {
    /// Rectángulo relleno.
    Rect {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: Color,
    },
    /// Contorno de un rectángulo.
    Borde {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: Color,
        grosor: i32,
    },
    /// Texto: celdas de fuente bitmap 5x7 ya colocadas (no hace falta fuente
    /// del sistema). `escala` = píxeles por píxel de la fuente.
    Texto {
        celdas: Vec<Celda>,
        escala: i32,
    },
    /// Badge: rectángulo con un número dentro.
    Badge {
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        color: Color,
        celdas: Vec<Celda>,
    },
}

/// Ancho en píxeles de un texto con la fuente 5x7 (5 + 1 de separación).
pub fn ancho_texto(texto: &str, escala: i32) -> i32 {
    let n = texto.chars().count() as i32;
    if n == 0 {
        0
    } else {
        (n * 6 - 1) * escala
    }
}

/// Alto en píxeles de una línea con la fuente 5x7.
pub const ALTO_LINEA: i32 = 7;

/// Convierte un texto en celdas 5x7 colocadas en (x, y).
pub fn celdas_5x7(texto: &str, x: i32, y: i32, escala: i32, color: Color) -> Vec<Celda> {
    let mut celdas = Vec::new();
    let mut cursor = x;
    for ch in texto.chars() {
        if let Some(filas) = glifo(ch) {
            celdas.push(Celda {
                x: cursor,
                y,
                filas,
                color,
            });
        }
        cursor += 6 * escala;
    }
    celdas
}

// ---------------------------------------------------------------------------
// Fuente bitmap 5x7 dibujada a mano (truco de GZH-XR: sin fuente del sistema)
// ---------------------------------------------------------------------------

/// Convierte arte ASCII (7 filas de 5 caracteres, '#' = píxel) a bits.
const fn arte(filas: [&str; 7]) -> [u8; 7] {
    let mut salida = [0u8; 7];
    let mut y = 0usize;
    while y < 7 {
        let fila = filas[y].as_bytes();
        let mut x = 0usize;
        while x < 5 {
            if fila[x] == b'#' {
                salida[y] |= 1u8 << (4 - x);
            }
            x += 1;
        }
        y += 1;
    }
    salida
}

/// Glifo de un carácter (sin acentos: se usan las letras base; la Ñ tiene
/// su propio glifo). `None` = carácter no dibujable (se salta).
pub fn glifo(c: char) -> Option<[u8; 7]> {
    Some(match c {
        ' ' => arte([
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ]),
        'A' => arte([
            ".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ]),
        'B' => arte([
            "####.", "#...#", "#...#", "####.", "#...#", "#...#", "####.",
        ]),
        'C' => arte([
            ".###.", "#...#", "#....", "#....", "#....", "#...#", ".###.",
        ]),
        'D' => arte([
            "####.", "#...#", "#...#", "#...#", "#...#", "#...#", "####.",
        ]),
        'E' => arte([
            "#####", "#....", "#....", "####.", "#....", "#....", "#####",
        ]),
        'F' => arte([
            "#####", "#....", "#....", "####.", "#....", "#....", "#....",
        ]),
        'G' => arte([
            ".###.", "#...#", "#....", "#.###", "#...#", "#...#", ".###.",
        ]),
        'H' => arte([
            "#...#", "#...#", "#...#", "#####", "#...#", "#...#", "#...#",
        ]),
        'I' => arte([
            ".###.", "..#..", "..#..", "..#..", "..#..", "..#..", ".###.",
        ]),
        'J' => arte([
            "..###", "...#.", "...#.", "...#.", "...#.", "#..#.", ".##..",
        ]),
        'K' => arte([
            "#...#", "#..#.", "#.#..", "##...", "#.#..", "#..#.", "#...#",
        ]),
        'L' => arte([
            "#....", "#....", "#....", "#....", "#....", "#....", "#####",
        ]),
        'M' => arte([
            "#...#", "##.##", "#.#.#", "#.#.#", "#...#", "#...#", "#...#",
        ]),
        'N' => arte([
            "#...#", "##..#", "#.#.#", "#..##", "#...#", "#...#", "#...#",
        ]),
        'O' => arte([
            ".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ]),
        'P' => arte([
            "####.", "#...#", "#...#", "####.", "#....", "#....", "#....",
        ]),
        'Q' => arte([
            ".###.", "#...#", "#...#", "#...#", "#.#.#", "#..#.", ".##.#",
        ]),
        'R' => arte([
            "####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#",
        ]),
        'S' => arte([
            ".####", "#....", "#....", ".###.", "....#", "....#", "####.",
        ]),
        'T' => arte([
            "#####", "..#..", "..#..", "..#..", "..#..", "..#..", "..#..",
        ]),
        'U' => arte([
            "#...#", "#...#", "#...#", "#...#", "#...#", "#...#", ".###.",
        ]),
        'V' => arte([
            "#...#", "#...#", "#...#", "#...#", "#...#", ".#.#.", "..#..",
        ]),
        'W' => arte([
            "#...#", "#...#", "#...#", "#.#.#", "#.#.#", "##.##", "#...#",
        ]),
        'X' => arte([
            "#...#", "#...#", ".#.#.", "..#..", ".#.#.", "#...#", "#...#",
        ]),
        'Y' => arte([
            "#...#", "#...#", ".#.#.", "..#..", "..#..", "..#..", "..#..",
        ]),
        'Z' => arte([
            "#####", "....#", "...#.", "..#..", ".#...", "#....", "#####",
        ]),
        '0' => arte([
            ".###.", "#...#", "#..##", "#.#.#", "##..#", "#...#", ".###.",
        ]),
        '1' => arte([
            "..#..", ".##..", "..#..", "..#..", "..#..", "..#..", ".###.",
        ]),
        '2' => arte([
            ".###.", "#...#", "....#", "...#.", "..#..", ".#...", "#####",
        ]),
        '3' => arte([
            "#####", "...#.", "..#..", "...#.", "....#", "#...#", ".###.",
        ]),
        '4' => arte([
            "...#.", "..##.", ".#.#.", "#..#.", "#####", "...#.", "...#.",
        ]),
        '5' => arte([
            "#####", "#....", "####.", "....#", "....#", "#...#", ".###.",
        ]),
        '6' => arte([
            "..##.", ".#...", "#....", "####.", "#...#", "#...#", ".###.",
        ]),
        '7' => arte([
            "#####", "....#", "...#.", "..#..", "..#..", ".#...", ".#...",
        ]),
        '8' => arte([
            ".###.", "#...#", "#...#", ".###.", "#...#", "#...#", ".###.",
        ]),
        '9' => arte([
            ".###.", "#...#", "#...#", ".####", "....#", "...#.", ".##..",
        ]),
        '.' => arte([
            ".....", ".....", ".....", ".....", ".....", ".##..", ".##..",
        ]),
        ',' => arte([
            ".....", ".....", ".....", ".....", ".##..", ".##..", ".#...",
        ]),
        ':' => arte([
            ".....", ".##..", ".##..", ".....", ".##..", ".##..", ".....",
        ]),
        ';' => arte([
            ".....", ".##..", ".##..", ".....", ".##..", ".##..", ".#...",
        ]),
        '-' => arte([
            ".....", ".....", ".....", "#####", ".....", ".....", ".....",
        ]),
        '_' => arte([
            ".....", ".....", ".....", ".....", ".....", ".....", "#####",
        ]),
        '+' => arte([
            ".....", "..#..", "..#..", "#####", "..#..", "..#..", ".....",
        ]),
        '=' => arte([
            ".....", ".....", "#####", ".....", "#####", ".....", ".....",
        ]),
        '/' => arte([
            "....#", "....#", "...#.", "..#..", ".#...", "#....", "#....",
        ]),
        '%' => arte([
            "##..#", "##.#.", "...#.", "..#..", ".#...", "#.##.", "#..##",
        ]),
        '!' => arte([
            "..#..", "..#..", "..#..", "..#..", "..#..", ".....", "..#..",
        ]),
        '?' => arte([
            ".###.", "#...#", "....#", "...#.", "..#..", ".....", "..#..",
        ]),
        '(' => arte([
            "...#.", "..#..", ".#...", ".#...", ".#...", "..#..", "...#.",
        ]),
        ')' => arte([
            ".#...", "..#..", "...#.", "...#.", "...#.", "..#..", ".#...",
        ]),
        '<' => arte([
            ".....", "..#..", ".#...", "#....", ".#...", "..#..", ".....",
        ]),
        '>' => arte([
            ".....", ".#...", "..#..", "...#.", "..#..", ".#...", ".....",
        ]),
        '°' => arte([
            ".###.", ".#.#.", ".###.", ".....", ".....", ".....", ".....",
        ]),
        '·' => arte([
            ".....", ".....", ".....", ".##..", ".....", ".....", ".....",
        ]),
        // La Ñ con eñe: es la única letra con diacrítico que merece glifo propio.
        'Ñ' => arte([
            "###..", "#..#.", "#...#", "#...#", "#####", "#...#", "#...#",
        ]),
        // Acentos y minúsculas se_maps a la letra base (sin diacrítico).
        'Á' | 'À' | 'Ä' => glifo('A').unwrap(),
        'É' | 'È' | 'Ë' => glifo('E').unwrap(),
        'Í' | 'Ì' | 'Ï' => glifo('I').unwrap(),
        'Ó' | 'Ò' | 'Ö' => glifo('O').unwrap(),
        'Ú' | 'Ù' | 'Ü' => glifo('U').unwrap(),
        'á' | 'à' | 'ä' => glifo('A').unwrap(),
        'é' | 'è' | 'ë' => glifo('E').unwrap(),
        'í' | 'ì' | 'ï' => glifo('I').unwrap(),
        'ó' | 'ò' | 'ö' => glifo('O').unwrap(),
        'ú' | 'ù' | 'ü' => glifo('U').unwrap(),
        'ñ' => glifo('Ñ').unwrap(),
        'x' => glifo('X').unwrap(),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// Renderizador: tabla -> primitivas
// ---------------------------------------------------------------------------

/// Escala de las etiquetas de botón (píxeles por píxel de la fuente 5x7).
pub const ESCALA_TEXTO: i32 = 3;
/// Escala de las cabeceras de sección.
pub const ESCALA_SECCION: i32 = 2;

/// Convierte los controles de una ventana en primitivas de dibujo.
///
/// El orden de salida es el orden de la tabla: primero el fondo, luego los
/// bordes y encima los textos (que así nunca quedan tapados).
pub fn primitivas_de_controles(controles: &[&Control]) -> Vec<Primitiva> {
    let mut out: Vec<Primitiva> = Vec::new();

    // 1) Fondos
    for c in controles {
        let (x, y, w, h) = (c.rect.x, c.rect.y, c.rect.w, c.rect.h);
        match c.rol {
            Rol::Boton | Rol::Tarjeta | Rol::Operacion => {
                let color = match c.estado {
                    Estado::Hover => Color::BOTON_HOVER,
                    Estado::Seleccionado | Estado::Armado | Estado::Activo => Color::BOTON_ACTIVO,
                    Estado::Desactivado => Color::rgba(30, 34, 38, 200),
                    _ => Color::BOTON,
                };
                out.push(Primitiva::Rect { x, y, w, h, color });
            }
            Rol::Peligro => {
                let color = match c.estado {
                    Estado::Hover => Color::rgba(180, 50, 44, 245),
                    _ => Color::PELIGRO,
                };
                out.push(Primitiva::Rect { x, y, w, h, color });
            }
            Rol::Contexto => out.push(Primitiva::Rect {
                x,
                y,
                w,
                h,
                color: Color::rgba(10, 16, 22, 220),
            }),
            Rol::Pestana => out.push(Primitiva::Rect {
                x,
                y,
                w,
                h,
                color: Color::rgba(20, 30, 40, 230),
            }),
            Rol::Seccion | Rol::Info => {}
        }
    }

    // 2) Bordes
    for c in controles {
        let color = match c.estado {
            Estado::Hover => Color::BORDE_HOVER,
            Estado::Seleccionado | Estado::Armado | Estado::Activo => Color::TITULO,
            Estado::Desactivado => Color::rgba(80, 90, 100, 120),
            _ => Color::BORDE,
        };
        let grosor = if c.estado == Estado::Hover { 3 } else { 2 };
        match c.rol {
            Rol::Seccion | Rol::Info => {}
            _ => out.push(Primitiva::Borde {
                x: c.rect.x,
                y: c.rect.y,
                w: c.rect.w,
                h: c.rect.h,
                color,
                grosor,
            }),
        }
    }

    // 3) Textos
    for c in controles {
        let escala = match c.rol {
            Rol::Seccion => ESCALA_SECCION,
            _ => ESCALA_TEXTO,
        };
        let color = match c.estado {
            Estado::Desactivado => Color::rgba(120, 128, 136, 255),
            _ => match c.rol {
                Rol::Seccion | Rol::Info => Color::TEXTO_TENUE,
                _ => Color::TEXTO,
            },
        };
        let texto = c.etiqueta.to_uppercase();
        if texto.is_empty() {
            continue;
        }
        let ancho = ancho_texto(&texto, escala);
        // Los badges ocupan la derecha: el texto se recorta para no solaparse.
        let ancho_disponible = match &c.badge {
            Some(_) => c.rect.w - 96,
            None => c.rect.w - 16,
        };
        let texto = if ancho > ancho_disponible {
            recortar(&texto, (ancho_disponible.max(20) / (6 * escala)).max(1) as usize)
        } else {
            texto
        };
        let x = c.rect.x + (c.rect.w - ancho_texto(&texto, escala)).max(8) / 2;
        let y = c.rect.y + (c.rect.h - ALTO_LINEA * escala) / 2;
        let celdas = celdas_5x7(&texto, x, y, escala, color);
        out.push(Primitiva::Texto { celdas, escala });

        // Badge: cantidad o coste, en la esquina derecha.
        if let Some(badge) = &c.badge {
            let bw = 88.min(c.rect.w - 16);
            let bx = c.rect.x + c.rect.w - bw - 8;
            let by = c.rect.y + (c.rect.h - 20) / 2;
            out.push(Primitiva::Badge {
                x: bx,
                y: by,
                w: bw,
                h: 20,
                color: Color::rgba(255, 148, 30, 210),
                celdas: celdas_5x7(
                    &badge.to_uppercase(),
                    bx + 4,
                    by + 6,
                    2,
                    Color::rgb(20, 16, 12),
                ),
            });
        }
    }

    out
}

/// Recorta un texto a un máximo de caracteres.
fn recortar(texto: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    texto.chars().take(max_chars).collect()
}

// ---------------------------------------------------------------------------
// Nombres de los items (texto para el HUD)
// ---------------------------------------------------------------------------
