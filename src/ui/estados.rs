//! estados.rs — Los ESTADOS de la interacción del HUD de Factorio VR.
//!
//! Esta es la pieza que arregla la jugabilidad "mortal": el jugador SIEMPRE
//! sabe en qué modo está y qué va a pasar al pulsar. Cada modo escribe su
//! PISTA en texto grande, y el gatillo ARMA en vez de actuar al instante
//! (gesto diferido, como `XrTactics.h` de GZH-XR).
//!
//! La pista no es un adorno: es el contrato con el usuario. Si la pista dice
//! "coloca la máquina con el gatillo", el jugador sabe que un toque no borra
//! nada.

use super::tabla::Estado;
use super::ventanas::IdVentana;

/// Modo de trabajo actual del jugador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modo {
    /// Explorar la fábrica. Nada pendiente.
    Normal,
    /// Hay una máquina elegida: se coloca en el suelo.
    Construir,
    /// Hay una receta elegida: se fabrica.
    Fabricar,
    /// Se está moviendo una ventana.
    Mover,
    /// Se está girando una ventana.
    Rotar,
}

impl Modo {
    pub fn etiqueta(self) -> &'static str {
        match self {
            Modo::Normal => "NORMAL",
            Modo::Construir => "CONSTRUIR",
            Modo::Fabricar => "FABRICAR",
            Modo::Mover => "MOVER",
            Modo::Rotar => "ROTAR",
        }
    }
}

/// Estado de las manos respecto al gatillo (armar -> soltar).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EstadoGatillo {
    /// El gatillo no hace nada todavía: hay que armar antes.
    Desarmado,
    /// Armado: el siguiente gatillo actua de verdad.
    Armado,
    /// El gatillo está pulsado: la acción se emite al soltar.
    Pulsado,
}

/// Qué ha pasado con el gatillo en este frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventoGatillo {
    /// Nada.
    Idle,
    /// Se ha armado: hay que soltar para que salga la orden.
    Armando,
    /// Se ha soltado: la orden se puede emitir AHORA.
    Soltado,
    /// El gatillo se pulsó sin estar armado: se ignora (no se pierde nada).
    Rechazado,
}

/// Estado global de la interacción del HUD.
#[derive(Clone, Debug, PartialEq)]
pub struct Estados {
    /// Modo actual.
    pub modo: Modo,
    /// Estado del gatillo.
    pub gatillo: EstadoGatillo,
    /// Control sobre el que está el rayo: (ventana, id de control).
    pub hover: Option<(IdVentana, u32)>,
    /// Texto de PISTA: lo que el jugador lee para saber qué va a pasar.
    pub pista: String,
    /// Item o receta pendiente (máquina elegida, receta elegida).
    pub seleccion: Option<u32>,
    /// Control de la tabla que está seleccionado (ventana + id de control).
    pub control_seleccionado: Option<(IdVentana, u32)>,
}

impl Default for Estados {
    fn default() -> Self {
        Self::new()
    }
}

impl Estados {
    pub fn new() -> Self {
        let mut e = Self {
            modo: Modo::Normal,
            gatillo: EstadoGatillo::Desarmado,
            hover: None,
            pista: String::new(),
            seleccion: None,
            control_seleccionado: None,
        };
        e.actualizar_pista();
        e
    }

    /// Cambia de modo y reescribe la pista.
    pub fn set_modo(&mut self, modo: Modo) {
        self.modo = modo;
        self.actualizar_pista();
    }

    /// Elige una máquina (entra en modo Construir) o la quita (vuelve a Normal).
    ///
    /// `control` es el (ventana, id) del botón de la tabla que la representa.
    pub fn elegir_maquina(&mut self, item_id: u32, control: (IdVentana, u32)) {
        if self.seleccion == Some(item_id) {
            self.seleccion = None;
            self.control_seleccionado = None;
            self.set_modo(Modo::Normal);
        } else {
            self.seleccion = Some(item_id);
            self.control_seleccionado = Some(control);
            self.set_modo(Modo::Construir);
        }
    }

    /// Elige una receta (modo Fabricar).
    pub fn elegir_receta(&mut self, item_id: u32, control: (IdVentana, u32)) {
        self.seleccion = Some(item_id);
        self.control_seleccionado = Some(control);
        self.set_modo(Modo::Fabricar);
    }

    /// Vuelve al modo normal y suelta la selección.
    pub fn cancelar(&mut self) {
        self.seleccion = None;
        self.control_seleccionado = None;
        self.set_modo(Modo::Normal);
    }

    /// El texto de PISTA del modo actual.
    ///
    /// Son frases cortas y en español: se leen de un vistazo y en voz alta.
    pub fn pista_de(modo: Modo) -> &'static str {
        match modo {
            Modo::Normal => "Modo normal: apunta a una maquina y pulsa para construir",
            Modo::Construir => "Modo construir: elige una maquina y colocala con el gatillo",
            Modo::Fabricar => "Modo fabricar: elige una receta y pulsa para fabricar",
            Modo::Mover => "Modo mover: agarra la ventana y mueve la mano",
            Modo::Rotar => "Modo rotar: gira la mano para orientar la ventana",
        }
    }

    /// Recalcula `pista` a partir del modo actual.
    pub fn actualizar_pista(&mut self) {
        self.pista = Self::pista_de(self.modo).to_string();
    }

    /// El jugador mira un control (o ninguno).
    pub fn set_hover(&mut self, hover: Option<(IdVentana, u32)>) {
        self.hover = hover;
    }

    /// ¿El control sobre el que está el rayo está en estado Hover?
    pub fn hover_actual(&self) -> Option<(IdVentana, u32)> {
        self.hover
    }

    /// Arma el gatillo: el siguiente pulso ya cuenta.
    pub fn armar(&mut self) {
        self.gatillo = EstadoGatillo::Armado;
    }

    /// Desarma: el gatillo vuelve a no hacer nada.
    pub fn desarmar(&mut self) {
        self.gatillo = EstadoGatillo::Desarmado;
    }

    /// Procesa el gatillo. Devuelve el evento para que el llamador actúe.
    ///
    /// - Sin armar: el pulso se RECHAZA (no se ejecuta nada por error).
    /// - Armado y se pulsa: queda Pulsado; la orden sale al SOLTAR.
    /// - Pulsado y se suelta: evento Soltado (aquí se emite la orden).
    pub fn gatillo_presionado(&mut self, presionado: bool) -> EventoGatillo {
        match (self.gatillo, presionado) {
            (EstadoGatillo::Desarmado, true) => EventoGatillo::Rechazado,
            (EstadoGatillo::Desarmado, false) => EventoGatillo::Idle,
            (EstadoGatillo::Armado, true) => {
                self.gatillo = EstadoGatillo::Pulsado;
                EventoGatillo::Armando
            }
            (EstadoGatillo::Armado, false) => EventoGatillo::Idle,
            (EstadoGatillo::Pulsado, true) => EventoGatillo::Idle,
            (EstadoGatillo::Pulsado, false) => {
                self.gatillo = EstadoGatillo::Armado;
                EventoGatillo::Soltado
            }
        }
    }

    /// Pista completa: modo + (selección) para que se entienda de un vistazo.
    pub fn pista_legible(&self) -> String {
        match self.seleccion {
            Some(item) => format!(
                "{} · {}",
                Self::pista_de(self.modo),
                super::items::nombre_item(item)
            ),
            None => Self::pista_de(self.modo).to_string(),
        }
    }

    /// Estado del control de la tabla según el estado actual del HUD.
    ///
    /// Convierte el estado abstracto (modo, selección, hover) en el estado
    /// concreto de un control de la tabla, para que el dibujo y el hit-test
    /// hablen del mismo lenguaje.
    pub fn estado_de_control(&self, control: (IdVentana, u32), hover: bool) -> Estado {
        if hover {
            return Estado::Hover;
        }
        match self.modo {
            Modo::Construir | Modo::Fabricar => {
                if self.control_seleccionado == Some(control) {
                    Estado::Seleccionado
                } else {
                    Estado::Reposo
                }
            }
            _ => Estado::Reposo,
        }
    }
}
