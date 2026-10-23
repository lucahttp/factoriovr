// panel_mesa.h — Geometria del panel del juego TUMBADO sobre la mesa.
//
// El visor (APK OpenXR) recibia el frame del motor por TCP y lo pintaba en
// un quad VERTICAL al fondo de la mesa.  Para que la fabrica este "sobre mi
// mesa" el quad tiene que ir TUMBADO: en el plano XZ, con la normal hacia
// +Y (arriba) y el passthrough alrededor.
//
// Este modulo es matematicas PURAS (sin OpenXR, sin GL, sin Android): se
// puede compilar y probar en el escritorio del Mac.  De aqui salen:
//
//   * la matriz MODELO del panel tumbado (column-major, como el resto del
//     visor),
//   * la geometria del quad tumbado (unitario: el tamano lo mete la escala
//     de la matriz),
//   * el rayo -> pixel (lo que permite que la mano del VR haga clic en el
//     punto correcto del juego) y su inversa pixel -> punto.
//
// ---------------------------------------------------------------------------
// CONVENCIONES FIJADAS (no cambiarlas sin tocar el inyector a la vez)
// ---------------------------------------------------------------------------
//
// 1. El motor dibuja el mundo con la Y INVERTIDA: en el frame que llega por
//    el socket la FILA 0 es la de ARRIBA de la imagen.  El inyector manda el
//    frame tal cual (su `reducir()` solo hace un promediado por bloques, sin
//    voltear), asi que la fila 0 sigue siendo la de arriba.
//
// 2. En el quad la V va INVERTIDA a proposito: v = 0 es la parte de ARRIBA
//    de la imagen y v = 1 la de ABAJO (el motor ya entrega la imagen con la
//    Y invertida, al subirla tal cual a la textura la t=0 es la fila 0).
//
// 3. Por lo tanto el pixel que devuelve `panel_mesa_rayo_a_pixel` cuenta
//    desde ARRIBA: y = 0 es la fila de arriba de la imagen.  Es justo lo que
//    espera el inyector (el raton de SDL cuenta desde arriba y su escala es
//    y * alto_destino / alto_origen).
//
// 4. La geometria es un quad UNITARIO en el plano local XY: x e y van de -1
//    a +1, z = 0 y la normal es (0, 0, 1).  El ancho y el fondo reales los
//    mete la ESCALA de la matriz modelo (medio ancho en X, medio fondo en Y).
//    Asi una sola geometria vale para cualquier resolucion de imagen.
//
// 5. Al tumbarlo: la imagen se coloca como el juego en la mesa, es decir
//    ARRIBA de la imagen = -Z (lejos del jugador) y DERECHA de la imagen =
//    +X, igual que los sprites del mundo (mx = (tile_x - pan_x) * 0.04,
//    mz = (tile_y - pan_y) * 0.04).  La UV del panel NO cambia al tumbarlo:
//    lo que cambia es a que punto de la mesa apunta cada texel.

#ifndef PANEL_MESA_H
#define PANEL_MESA_H

#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// ---------------------------------------------------------------------------
// Constantes del proyecto
// ---------------------------------------------------------------------------

/// Píxeles del juego por casilla en el frame ORIGINAL del motor (zoom 1 de
/// Factorio).  El visor lo usa ya para los sprites: "una casilla son 32 px
/// y aqui una casilla son 0.04 m".
#define PANEL_MESA_PX_POR_CASILLA 32.0f

/// Metros de mesa que mide una casilla del juego (la escala del diorama).
#define PANEL_MESA_M_POR_CASILLA 0.04f

/// Tamaño por defecto del frame que manda el inyector
/// (XFACTORVR_STREAM_ANCHO / XFACTORVR_STREAM_ALTO, por defecto 640x360).
/// OJO: es el tamaño MAXIMO; el inyector solo reduce con factores enteros,
/// asi que el frame puede llegar mas grande (p.ej. 856x384) o mas pequeño.
#define PANEL_MESA_IMG_ANCHO_DEF 640
#define PANEL_MESA_IMG_ALTO_DEF 360

// ---------------------------------------------------------------------------
// Configuracion del panel tumbado
// ---------------------------------------------------------------------------

/// De donde sale el factor de escala.
///
/// El juego dibuja una casilla con PANEL_MESA_PX_POR_CASILLA píxeles en su
/// framebuffer original.  El inyector reduce ese framebuffer al tamaño
/// objetivo con un factor entero (`reducir()` en injector-linux/src/stream.rs:
/// fx = ancho_juego / ancho_objetivo), asi que en la imagen RECIBIDA una
/// casilla ocupa `px_por_casilla_juego * img_w / ancho_juego` píxeles.
///
/// Si el juego va a 32 px/casilla y el frame llega sin reducir (fx = 1), la
/// escala es 32 y la imagen de 640 px cubre 20 casillas = 0.80 m de mesa.
typedef struct {
    /// Ancho del frame del motor recibido, en píxeles.
    int img_w;
    /// Alto del frame del motor recibido, en píxeles.
    int img_h;
    /// Píxeles de la imagen RECIBIDA que cubren una casilla del juego.
    float px_por_casilla;
    /// Metros de mesa que mide una casilla del juego.
    float m_por_casilla;
    /// Altura del panel sobre la superficie de la mesa (Y), en metros.
    float altura_m;
    /// Desplazamiento del centro del panel en X dentro de la mesa.
    float offset_x_m;
    /// Desplazamiento del centro del panel en Z dentro de la mesa.
    float offset_z_m;
} PanelMesaConfig;

/// Rellena la config con los valores por defecto (imagen 640x360, 32 px por
/// casilla, 0.04 m por casilla, panel a 2 mm de la mesa y centrado en el
/// origen de la mesa, que es el centro del area de juego del diorama).
void panel_mesa_config_inicial(PanelMesaConfig* cfg);

/// Píxeles por casilla que quedan en la imagen RECIBIDA.
/// `ancho_juego` es el ancho del framebuffer original del motor (el que
/// ve el inyector antes de reducir) y `px_por_casilla_juego` los píxeles por
/// casilla del juego (32 por defecto).
float panel_mesa_px_por_casilla_recibida(int img_w, int ancho_juego,
                                          float px_por_casilla_juego);

/// Tamaño fisico del panel en metros: `ancho_m` en X y `fondo_m` en Z
/// (la altura de la imagen, tumbada).  Sale de la imagen y de la escala:
/// casillas = px_de_imagen / px_por_casilla, metros = casillas * m_por_casilla.
void panel_mesa_tamano(const PanelMesaConfig* cfg, float* ancho_m, float* fondo_m);

/// Matriz MODELO del panel tumbado, ya en el espacio del mundo.
///
/// `model_mesa` es la matriz modelo de la mesa (column-major, la misma que
/// usa el resto del visor: `mat4_multiply(model_mesa, m_pos, m_panel)`).
/// El resultado es `model_mesa * T(offset_x, altura, offset_z) * Rx(-90) *
/// S(ancho/2, fondo/2, 1)`, o sea:
///   - la normal (0,0,1) del quad pasa a (0,1,0) (apunta ARRIBA),
///   - la ARRIBA de la imagen queda en -Z (lejos) y la DERECHA en +X,
///   - el panel mide lo que diga la escala (1 casilla = m_por_casilla).
void panel_mesa_matriz_tumbado(const float* model_mesa, const PanelMesaConfig* cfg,
                                float* out16);

/// Version con la firma "de mesa": `ancho_m` y `profundidad_m` son las
/// medidas utiles de la MESA (X y Z) y el panel se dimensiona para cubrirlas
/// (centrado en el origen de la mesa), a `alto_m` de altura sobre la
/// superficie.  `escala_px_por_casilla` y `metros_por_casilla` fijan la
/// correspondencia entre la imagen y la mesa (con
/// `panel_mesa_imagen_esperada` se obtiene el tamaño de imagen que encaja).
void panel_mesa_matriz_tumbado_mesa(const float* model_mesa, float ancho_m,
                                     float profundidad_m, float alto_m,
                                     float escala_px_por_casilla,
                                     float metros_por_casilla, float* out16);

/// Ancho en píxeles de la imagen que corresponde a un panel de `ancho_m`
/// metros con la escala dada (el alto sale de la relacion de la imagen).
int panel_mesa_imagen_esperada(float ancho_m, float metros_por_casilla,
                                float escala_px_por_casilla);

// ---------------------------------------------------------------------------
// Geometria
// ---------------------------------------------------------------------------

/// Vertice del quad tumbado (mismos campos que el vertice del visor, sin
/// color: el que lo pinte lo rellena).
typedef struct {
    float x, y, z;
    float nx, ny, nz;
    float u, v;
} PanelMesaVertice;

/// Numero de vertices del quad tumbado (2 triangulos).
#define PANEL_MESA_VERTICES 6

/// Rellena los 6 vertices del quad UNITARIO tumbado (x e y en [-1,1], z = 0,
/// normal (0,0,1)) con la V invertida: v = 0 es ARRIBA de la imagen y
/// v = 1 ABAJO.  La matriz de `panel_mesa_matriz_tumbado` le da el tamaño y
/// la orientacion en la mesa.
void panel_mesa_vertices(PanelMesaVertice* out);

// ---------------------------------------------------------------------------
// Rayo <-> pixel
// ---------------------------------------------------------------------------

/// Convierte el rayo de la mano (origen y direccion en el espacio del
/// mundo) en el pixel de la imagen del motor que hay debajo.
///
/// Devuelve false si el rayo no toca el panel (o si va en paralelo).
/// `model_panel` es la matriz del panel tumbado (la de
/// `panel_mesa_matriz_tumbado`) y `img_w`/`img_h` el tamaño de la imagen
/// del motor.
///
/// CONVENCION DE SALIDA: `out_y` cuenta desde ARRIBA (0 = fila de arriba de
/// la imagen), como espera el inyector.
bool panel_mesa_rayo_a_pixel(const float* origen, const float* dir,
                              const float* model_panel, int img_w, int img_h,
                              int* out_x, int* out_y);

/// La inversa: el punto del mundo (en el plano del panel) donde cae un
/// pixel de la imagen.  Sirve para depurar y para el hover.
/// El pixel (px, py) es el CENTRO de ese pixel de la imagen.
bool panel_mesa_pixel_a_punto(const float* model_panel, int img_w, int img_h,
                               int px, int py, float* out3);

/// Normal del quad tumbado en el espacio del mundo (siempre (0,1,0) en el
/// sistema de la mesa; se calcula aplicando la matriz por si la mesa esta
/// girada).
void panel_mesa_normal_mundo(const float* model_panel, float* out3);

#ifdef __cplusplus
}  // extern "C"
#endif

#endif  // PANEL_MESA_H
