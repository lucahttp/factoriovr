# `src/ui` — El HUD de Factorio VR

Este módulo convierte el HUD de tres sub-paneles dibujados a mano en C++ en
**ventanas espaciales con una tabla de controles única**.

La idea es de **Generals-Zero-Hour-XR** (`GeneralsMD/Code/Main/`):

| Referencia | Qué se copia |
|---|---|
| `XrPanelLayout.h` | Una sola tabla: píxeles + hit-test + estado. "pixels and hit regions cannot drift apart". |
| `XrSurface.h` | Superficies agarrables con una y dos manos, con límites y recaptura de pose sin saltos. |
| `XrTactics.h` | Gesto diferido (armar → soltar) y pista textual del modo. |
| `XrSceneUI.h` | UI en páginas/secciones, con estado *armed* para no aceptar input por error. |

No se copia el panel de 18 botones: en Factorio el juego ya tiene sus menús;
el HUD solo da **elegir máquina, ver inventario, fabricar, y saber qué va a
pasar**.

---

## Ficheros

| Fichero | Contenido |
|---|---|
| `mod.rs` | Fachada `Hud` (tabla + ventanas + estados) y los tests. |
| `tabla.rs` | `Tabla`, `Control`, `Rol`, `Estado`, `Accion`, hit-test, renderizador y fuente bitmap 5x7. |
| `ventanas.rs` | `Ventana`, `Ventanas`, `Agarre`, `Mano`, `Anclaje`, `Plano`, `Limites`. |
| `estados.rs` | `Modo`, `Estados`, `EstadoGatillo`, `EventoGatillo`, la pista de texto. |
| `items.rs` | Ids de item (espejo de `game_bridge::ITEM_*`) para que `ui` no dependa del motor. |

---

## API pública (exacta)

### Fachada

```rust
pub struct Hud {
    pub tabla: Tabla,
    pub ventanas: Ventanas,
    pub estados: Estados,
}

impl Hud {
    pub fn nuevo(inventario: &[ItemResumen], recetas: &[Receta]) -> Self;
    pub fn inventario_desde_mapa(mapa: &[(u32, u32)]) -> Vec<ItemResumen>;

    pub fn hit_test(&self, ventana: IdVentana, u: f32, v: f32) -> Option<u32>;   // u,v en 0..1
    pub fn hit_test_px(&self, ventana: IdVentana, x: f32, y: f32) -> Option<u32>;
    pub fn tamano_px(&self, ventana: IdVentana) -> (u32, u32);

    pub fn hover(&mut self, ventana: IdVentana, u: f32, v: f32) -> Option<u32>;
    pub fn pulsar(&mut self, ventana: IdVentana, id: u32) -> Option<Accion>;
    pub fn aplicar_accion(&mut self, ventana: IdVentana, id: u32) -> Option<u32>;
    pub fn sincronizar_estados(&mut self);

    pub fn pista(&self) -> &str;
    pub fn render_ventana(&self, ventana: IdVentana) -> Vec<Primitiva>;

    pub fn puede_agarrar(&self, ventana: IdVentana) -> bool;
    pub fn agarrar_una_mano(&mut self, ventana: IdVentana, mano: Mano, posicion: Vec3);
    pub fn agarrar_dos_manos(&mut self, ventana: IdVentana, distancia: f32, centro: Vec3);
    pub fn actualizar_dos_manos(&mut self, ventana: IdVentana, distancia: f32, centro: Vec3) -> f32;
    pub fn soltar(&mut self, ventana: IdVentana);
}
```

### Tabla

```rust
pub struct Tabla { pub controles: Vec<Control> }

impl Tabla {
    pub fn nueva() -> Self;
    pub fn por_defecto(inventario: &[ItemResumen], recetas: &[Receta]) -> Self;
    pub fn control(&self, ventana: IdVentana, id: u32) -> Option<&Control>;
    pub fn control_mut(&mut self, ventana: IdVentana, id: u32) -> Option<&mut Control>;
    pub fn controles_de(&self, ventana: IdVentana) -> impl Iterator<Item = &Control>;
    pub fn hit_test(&self, ventana: IdVentana, x: f32, y: f32) -> Option<&Control>;
    pub fn hit_test_abs(&self, ventana: IdVentana, origen: (f32,f32), punto: (f32,f32)) -> Option<&Control>;
    pub fn puede_pulsar(&self, ventana: IdVentana, id: u32) -> bool;
    pub fn set_estado(&mut self, ventana: IdVentana, id: u32, estado: Estado);
    pub fn set_etiqueta(&mut self, ventana: IdVentana, id: u32, etiqueta: impl Into<String>);
    pub fn limpiar_hover(&mut self, ventana: IdVentana, excepto: Option<u32>);
}

pub struct Control {
    pub id: u32,               // id estable, único por ventana
    pub ventana: IdVentana,
    pub rol: Rol,              // Boton | Seccion | Pestana | Tarjeta | Contexto | Operacion | Peligro | Info
    pub accion: Accion,        // qué pasa al pulsar
    pub etiqueta: String,      // texto visible
    pub estado: Estado,        // Reposo | Hover | Seleccionado | Armado | Pendiente | Desactivado | Activo
    pub rect: Rect,            // x, y, w, h en píxeles de la ventana
    pub badge: Option<String>, // cantidad o coste
}

pub enum Accion {
    Ninguna, Construir(u32), Fabricar(u32), ItemInventario(u32), Cerrar,
    Pestana(u8), Mirar, Demoler, MoverVentana, RotarVentana,
}
```

### Ventanas

```rust
pub enum IdVentana { Construir, Inventario, Fabricar, Vista }
pub enum Anclaje { Mundo, Cabeza }
pub enum Mano { Izquierda, Derecha }

pub struct Ventana {
    pub id: IdVentana,
    pub titulo: String,
    pub ancho_px: u32,
    pub alto_px: u32,
    pub ancho_m: f32,          // ancho real en metros (limitado 0,25 .. 1,50)
    pub plano: Plano,          // posicion, rotacion, escala
    pub anclaje: Anclaje,
    pub agarrable: bool,
}

impl Ventana {
    pub fn alto_m(&self) -> f32;
    pub fn px_por_m(&self) -> f32;
    pub fn uv_a_pixeles(&self, u: f32, v: f32) -> (f32, f32);
}

pub enum Agarre {
    Libre,
    UnaMano { mano: Mano, desplazamiento: Vec3 },
    DosManos { base_distancia: f32, base_ancho_m: f32, base_centro: Vec3 },
}

impl Ventanas {
    pub fn por_defecto() -> Self;
    pub fn ventana(&self, id: IdVentana) -> Option<&Ventana>;
    pub fn ventana_mut(&mut self, id: IdVentana) -> Option<&mut Ventana>;
    pub fn ventanas(&self) -> &[Ventana];
    pub fn agarre(&self, id: IdVentana) -> Agarre;
    pub fn esta_agarrada(&self, id: IdVentana) -> bool;
    pub fn esta_agarrada_dos_manos(&self, id: IdVentana) -> bool;
    pub fn agarrar_una_mano(&mut self, id: IdVentana, mano: Mano, posicion_mano: Vec3);
    pub fn agarrar_dos_manos(&mut self, id: IdVentana, distancia: f32, centro: Vec3);
    pub fn actualizar_dos_manos(&mut self, id: IdVentana, distancia: f32, centro: Vec3) -> f32;
    pub fn actualizar_una_mano(&mut self, id: IdVentana, posicion_mano: Vec3);
    pub fn soltar(&mut self, id: IdVentana);
    pub fn soltar_todas(&mut self);
    pub fn fijar_ancho(&mut self, id: IdVentana, ancho_m: f32) -> f32;
    pub fn mirar_a(&mut self, id: IdVentana, cabeza: Vec3);
    pub fn mirar_todas_a(&mut self, cabeza: Vec3);
}
```

### Estados

```rust
pub enum Modo { Normal, Construir, Fabricar, Mover, Rotar }
pub enum EstadoGatillo { Desarmado, Armado, Pulsado }
pub enum EventoGatillo { Idle, Armando, Soltado, Rechazado }

pub struct Estados {
    pub modo: Modo,
    pub gatillo: EstadoGatillo,
    pub hover: Option<(IdVentana, u32)>,
    pub pista: String,
    pub seleccion: Option<u32>,
    pub control_seleccionado: Option<(IdVentana, u32)>,
}

impl Estados {
    pub fn new() -> Self;
    pub fn set_modo(&mut self, modo: Modo);
    pub fn elegir_maquina(&mut self, item_id: u32, control: (IdVentana, u32));
    pub fn elegir_receta(&mut self, item_id: u32, control: (IdVentana, u32));
    pub fn cancelar(&mut self);
    pub fn pista_de(modo: Modo) -> &'static str;   // texto de PISTA
    pub fn actualizar_pista(&mut self);
    pub fn set_hover(&mut self, hover: Option<(IdVentana, u32)>);
    pub fn hover_actual(&self) -> Option<(IdVentana, u32)>;
    pub fn armar(&mut self);
    pub fn desarmar(&mut self);
    pub fn gatillo_presionado(&mut self, presionado: bool) -> EventoGatillo;
    pub fn pista_legible(&self) -> String;
    pub fn estado_de_control(&self, control: (IdVentana, u32), hover: bool) -> Estado;
}
```

---

## Formato de las primitivas de dibujo

`render_ventana(ventana)` devuelve `Vec<Primitiva>`. **No se dibuja nada en
Rust**: el C++ solo pinta estas primitivas.

```rust
pub enum Primitiva {
    Rect  { x: i32, y: i32, w: i32, h: i32, color: Color },
    Borde { x: i32, y: i32, w: i32, h: i32, color: Color, grosor: i32 },
    Texto { celdas: Vec<Celda>, escala: i32 },
    Badge { x: i32, y: i32, w: i32, h: i32, color: Color, celdas: Vec<Celda> },
}

pub struct Celda { pub x: i32, pub y: i32, pub filas: [u8; 7], pub color: Color }
pub struct Color { pub r: u8, pub g: u8, pub b: u8, pub a: u8 }
```

* Píxeles enteros, **origen arriba-izquierda**, Y hacia abajo (como en
  `XrPanelLayout.h`).
* `Celda.filas[0..7]` son las 7 filas de la fuente bitmap 5x7; cada fila es un
  byte donde el **bit 4 es el píxel de la izquierda** y el bit 0 el de la
  derecha. `Celda.x/y` es la esquina superior izquierda del glifo.
* `escala` = píxeles por píxel de la fuente (3 para botones, 2 para
  secciones). Carácter = 5 px de ancho + 1 px de separación.
* La fuente cubre A-Z, 0-9 y `. , : ; - _ + = / % ! ? ( ) < > ° · Ñ`; las
  minúsculas y las vocales acentuadas caen en la letra base.

---

## Cómo lo cablea el agente principal

1. **`src/lib.rs`**: añadir `pub mod ui;` (el módulo es autocontenido: no
   depende de `game_bridge`, `tabletop` ni `openxr`).
2. **Texturas**: crear un quad por ventana de `ancho_px x alto_px` y pintar
   `hud.render_ventana(ventana)` en una textura de ese tamaño. La ventana se
   coloca en el mundo con `ventanas.ventana(id).plano` (posición, rotación,
   escala) y `ancho_m` es el ancho real en metros.
3. **Hit-test**: cuando el rayo toca la ventana, pasar las UV del quad
   (`u`, `v` en 0..1) a `hud.hit_test(ventana, u, v)`. Sale de la **misma**
   tabla que se ha pintado, así que no puede desincronizarse.
4. **Pulsación**: `hud.pulsar(ventana, id)` → `Option<Accion>`. `None` = el
   control estaba `Desactivado` (no hay recursos): no se hace nada.
5. **Modo y pista**: `hud.estados.set_modo(Modo::Construir)` etc. El texto de
   pista sale de `hud.pista()` (o `hud.pista_legible()`, que añade el objeto
   elegido) y se pinta grande en el cartel del visor.
6. **Gesto diferido**: `hud.estados.armar()` y luego
   `hud.estados.gatillo_presionado(true/false)`. `Rechazado` = no estaba
   armado (no se ejecuta nada); `Soltado` = aquí se emite la orden.
7. **Ventanas manipulables**: `hud.agarrar_una_mano(...)`,
   `hud.agarrar_dos_manos(distancia, centro)` y `hud.actualizar_dos_manos(...)`
   cada frame. Al pasar de una mano a dos manos el ancho NO cambia en ese
   instante (sin salto); después se escala con límites de 0,25 m a 1,50 m.

### Los ids de control (por ventana)

| Ventana | id | Qué es |
|---|---|---|
| Construir | 1 | Cabecera "ELIGE UNA MAQUINA" (no pulsable) |
| Construir | 10..17 | Las 8 máquinas (ENSAMBLADORA, MINERA, HORNO, TORRETA, RADAR, ROBOPUERTO, COFRE, POSTE) |
| Construir | 30 | DEMOLER (rojo, `Rol::Peligro`) |
| Construir | 31 | Texto de ayuda (no pulsable) |
| Inventario | 1 | Cabecera "EN TU BOLSA" |
| Inventario | 10..15 | Los 6 primeros items, con la cantidad en badge |
| Inventario | 90 | "LA BOLSA ESTA VACIA" |
| Fabricar | 1 | Cabecera "FABRICAR" |
| Fabricar | 10..13 | Las 4 recetas, con el coste en badge |
| Vista | 10..12 | MIRAR MAPA / CENTRAR / GIRAR |
| Vista | 50 | La PISTA del modo actual (texto dinámico) |

---

## Tests

11 tests en `mod.rs` (`cargo test --test ui_hud` mientras `lib.rs` no declare
el módulo; `cargo test --lib ui::` cuando lo declare):

* el hit-test acierta el control y **nunca** el vecino;
* una sección (no pulsable) no recibe el rayo;
* un control `Desactivado` **no** recibe pulsación ni aparece bajo el rayo;
* la pista de cada modo es la correcta;
* elegir máquina entra en modo Construir y escribe la pista;
* el gatillo sin armar se rechaza (gesto diferido);
* al agarrar con dos manos se respetan los límites (0,25 m / 1,50 m) y **no hay
  salto** al pasar de una mano a dos;
* las cuatro ventanas no están apiladas en el mismo sitio;
* cada ventana dibuja su título;
* la fuente 5x7 cubre A-Z y 0-9;
* todo rectángulo pintado viene de la tabla o del marco (nada de geometría
  oculta).
