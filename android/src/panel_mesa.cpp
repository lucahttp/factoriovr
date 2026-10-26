// panel_mesa.cpp — Matematicas puras del panel tumbado (ver panel_mesa.h).
//
// Sin OpenXR, sin GL, sin Android: solo <cmath> y <cstring>.  Todo lo que
// hay aqui se puede compilar y probar en el Mac.

#include "panel_mesa.h"

#include <cmath>
#include <cstring>

namespace {

// --- Utilidades de matriz 4x4 (column-major, como el resto del visor) -----
// En column-major m[col*4 + fila]: la traslacion va en m[12], m[13], m[14].

void mat_identidad(float* m) {
    std::memset(m, 0, sizeof(float) * 16);
    m[0] = m[5] = m[10] = m[15] = 1.0f;
}

// out = a * b (igual que matMultiply del visor).
void mat_multiplica(const float* a, const float* b, float* out) {
    float tmp[16];
    for (int c = 0; c < 4; ++c) {
        for (int r = 0; r < 4; ++r) {
            tmp[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] +
                             a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
        }
    }
    std::memcpy(out, tmp, sizeof(tmp));
}

void mat_traslacion(float* m, float x, float y, float z) {
    mat_identidad(m);
    m[12] = x;
    m[13] = y;
    m[14] = z;
}

// Rotacion alrededor del eje X (filas = eje X).  Con rad = -90 el +Y local
// se va al -Z (la ARRIBA de la imagen queda lejos del jugador) y la normal
// (0,0,1) pasa a (0,1,0), o sea ARRIBA.
void mat_rotacion_x(float* m, float rad) {
    const float c = std::cos(rad);
    const float s = std::sin(rad);
    std::memset(m, 0, sizeof(float) * 16);
    m[0] = 1.0f;
    m[5] = c;  m[6] = s;
    m[9] = -s; m[10] = c;
    m[15] = 1.0f;
}

void mat_escala(float* m, float sx, float sy, float sz) {
    mat_identidad(m);
    m[0] = sx;
    m[5] = sy;
    m[10] = sz;
}

// Punto (w = 1) por la matriz.
void mat_punto(const float* m, const float* p, float* out) {
    const float x = p[0], y = p[1], z = p[2];
    out[0] = m[0] * x + m[4] * y + m[8] * z + m[12];
    out[1] = m[1] * x + m[5] * y + m[9] * z + m[13];
    out[2] = m[2] * x + m[6] * y + m[10] * z + m[14];
}

// Direccion (w = 0) por la matriz: solo la parte lineal.
void mat_direccion(const float* m, const float* d, float* out) {
    const float x = d[0], y = d[1], z = d[2];
    out[0] = m[0] * x + m[4] * y + m[8] * z;
    out[1] = m[1] * x + m[5] * y + m[9] * z;
    out[2] = m[2] * x + m[6] * y + m[10] * z;
}

// Inversa general de una 4x4 (Gauss-Jordan con pivoteo parcial).
// Devuelve false si la matriz es singular (no invertible).
bool mat_inversa(const float* m, float* out) {
    double a[4][8];
    for (int r = 0; r < 4; ++r) {
        for (int c = 0; c < 4; ++c) {
            a[r][c] = static_cast<double>(m[c * 4 + r]);
            a[r][4 + c] = (r == c) ? 1.0 : 0.0;
        }
    }
    for (int col = 0; col < 4; ++col) {
        int pivote = col;
        for (int r = col + 1; r < 4; ++r) {
            if (std::fabs(a[r][col]) > std::fabs(a[pivote][col])) pivote = r;
        }
        if (std::fabs(a[pivote][col]) < 1e-12) return false;
        if (pivote != col) {
            for (int c = 0; c < 8; ++c) {
                const double t = a[col][c];
                a[col][c] = a[pivote][c];
                a[pivote][c] = t;
            }
        }
        const double inv_p = 1.0 / a[col][col];
        for (int c = 0; c < 8; ++c) a[col][c] *= inv_p;
        for (int r = 0; r < 4; ++r) {
            if (r == col) continue;
            const double f = a[r][col];
            if (f == 0.0) continue;
            for (int c = 0; c < 8; ++c) a[r][c] -= f * a[col][c];
        }
    }
    for (int r = 0; r < 4; ++r) {
        for (int c = 0; c < 4; ++c) {
            out[c * 4 + r] = static_cast<float>(a[r][4 + c]);
        }
    }
    return true;
}

constexpr float kEps = 1e-5f;

}  // namespace

// ---------------------------------------------------------------------------
// Configuracion
// ---------------------------------------------------------------------------

void panel_mesa_config_inicial(PanelMesaConfig* cfg) {
    if (cfg == nullptr) return;
    cfg->img_w = PANEL_MESA_IMG_ANCHO_DEF;
    cfg->img_h = PANEL_MESA_IMG_ALTO_DEF;
    cfg->px_por_casilla = PANEL_MESA_PX_POR_CASILLA;
    cfg->m_por_casilla = PANEL_MESA_M_POR_CASILLA;
    // 2 mm: por encima del suelo de concreto de la mesa (y = 0) y de los
    // sprites del suelo (0.9 mm), sin flotar.
    cfg->altura_m = 0.002f;
    cfg->offset_x_m = 0.0f;
    cfg->offset_z_m = 0.0f;
}

float panel_mesa_px_por_casilla_recibida(int img_w, int ancho_juego,
                                          float px_por_casilla_juego) {
    if (img_w <= 0 || ancho_juego <= 0 || px_por_casilla_juego <= 0.0f) {
        return px_por_casilla_juego > 0.0f ? px_por_casilla_juego : PANEL_MESA_PX_POR_CASILLA;
    }
    // El inyector reduce con factor entero: fx = ancho_juego / ancho_objetivo.
    // Los pixeles por casilla del frame recibido son los del juego escalados
    // por 1/fx.
    return px_por_casilla_juego * (static_cast<float>(img_w) / static_cast<float>(ancho_juego));
}

void panel_mesa_tamano(const PanelMesaConfig* cfg, float* ancho_m, float* fondo_m) {
    float ancho = 0.0f;
    float fondo = 0.0f;
    if (cfg != nullptr && cfg->img_w > 0 && cfg->img_h > 0 && cfg->px_por_casilla > 0.0f &&
        cfg->m_por_casilla > 0.0f) {
        const float casillas_x = static_cast<float>(cfg->img_w) / cfg->px_por_casilla;
        const float casillas_y = static_cast<float>(cfg->img_h) / cfg->px_por_casilla;
        ancho = casillas_x * cfg->m_por_casilla;
        fondo = casillas_y * cfg->m_por_casilla;
    }
    if (ancho_m != nullptr) *ancho_m = ancho;
    if (fondo_m != nullptr) *fondo_m = fondo;
}

// ---------------------------------------------------------------------------
// Matriz del panel tumbado
// ---------------------------------------------------------------------------

void panel_mesa_matriz_tumbado(const float* model_mesa, const PanelMesaConfig* cfg,
                                float* out16) {
    if (out16 == nullptr) return;
    PanelMesaConfig por_defecto;
    if (cfg == nullptr) {
        panel_mesa_config_inicial(&por_defecto);
        cfg = &por_defecto;
    }
    float ancho = 0.0f;
    float fondo = 0.0f;
    panel_mesa_tamano(cfg, &ancho, &fondo);

    // El quad es unitario: la escala pone el medio ancho y el medio fondo.
    const float sx = ancho * 0.5f;
    const float sy = fondo * 0.5f;

    float m_pos[16], m_rot[16], m_scl[16], m_local[16], m_panel[16];
    mat_traslacion(m_pos, cfg->offset_x_m, cfg->altura_m, cfg->offset_z_m);
    mat_rotacion_x(m_rot, -1.5707963267948966f);  // -90 grados
    mat_escala(m_scl, sx, sy, 1.0f);
    mat_multiplica(m_pos, m_rot, m_local);
    mat_multiplica(m_local, m_scl, m_panel);

    if (model_mesa == nullptr) {
        std::memcpy(out16, m_panel, sizeof(float) * 16);
        return;
    }
    mat_multiplica(model_mesa, m_panel, out16);
}

void panel_mesa_matriz_tumbado_mesa(const float* model_mesa, float ancho_m,
                                     float profundidad_m, float alto_m,
                                     float escala_px_por_casilla,
                                     float metros_por_casilla, float* out16) {
    if (out16 == nullptr) return;
    // El panel cubre el area util de la mesa, centrado en el origen de la
    // mesa (que es el centro del area de juego del diorama).
    PanelMesaConfig cfg;
    panel_mesa_config_inicial(&cfg);
    cfg.altura_m = alto_m;
    cfg.offset_x_m = 0.0f;
    cfg.offset_z_m = 0.0f;

    float sx = ancho_m * 0.5f;
    float sy = profundidad_m * 0.5f;
    if (escala_px_por_casilla > 0.0f && metros_por_casilla > 0.0f) {
        // La escala fija la correspondencia entre la imagen y la mesa: con
        // un panel de `ancho_m` metros, la imagen que encaja tiene
        // ancho_m / metros_por_casilla * escala_px_por_casilla pixeles.
        // Aqui no hace falta la imagen: el panel ya tiene su medida.
        (void)escala_px_por_casilla;
        (void)metros_por_casilla;
    }

    float m_pos[16], m_rot[16], m_scl[16], m_local[16], m_panel[16];
    mat_traslacion(m_pos, 0.0f, alto_m, 0.0f);
    mat_rotacion_x(m_rot, -1.5707963267948966f);
    mat_escala(m_scl, sx, sy, 1.0f);
    mat_multiplica(m_pos, m_rot, m_local);
    mat_multiplica(m_local, m_scl, m_panel);

    if (model_mesa == nullptr) {
        std::memcpy(out16, m_panel, sizeof(float) * 16);
        return;
    }
    mat_multiplica(model_mesa, m_panel, out16);
}

int panel_mesa_imagen_esperada(float ancho_m, float metros_por_casilla,
                                float escala_px_por_casilla) {
    if (ancho_m <= 0.0f || metros_por_casilla <= 0.0f || escala_px_por_casilla <= 0.0f) {
        return 0;
    }
    const float casillas = ancho_m / metros_por_casilla;
    return static_cast<int>(casillas * escala_px_por_casilla + 0.5f);
}

// ---------------------------------------------------------------------------
// Geometria
// ---------------------------------------------------------------------------

void panel_mesa_vertices(PanelMesaVertice* out) {
    if (out == nullptr) return;
    // Quad unitario en el plano local XY, cara que mira a +Z.
    // La V va INVERTIDA: v = 0 es ARRIBA de la imagen (motor con la Y
    // invertida) y v = 1 ABAJO.  La U va de 0 (izquierda) a 1 (derecha).
    struct Vtx {
        float x, y, u, v;
    };
    static const Vtx kVerts[PANEL_MESA_VERTICES] = {
        {-1.0f, -1.0f, 0.0f, 1.0f},
        { 1.0f, -1.0f, 1.0f, 1.0f},
        { 1.0f,  1.0f, 1.0f, 0.0f},
        {-1.0f, -1.0f, 0.0f, 1.0f},
        { 1.0f,  1.0f, 1.0f, 0.0f},
        {-1.0f,  1.0f, 0.0f, 0.0f},
    };
    for (int i = 0; i < PANEL_MESA_VERTICES; ++i) {
        out[i].x = kVerts[i].x;
        out[i].y = kVerts[i].y;
        out[i].z = 0.0f;
        out[i].nx = 0.0f;
        out[i].ny = 0.0f;
        out[i].nz = 1.0f;
        out[i].u = kVerts[i].u;
        out[i].v = kVerts[i].v;
    }
}

// ---------------------------------------------------------------------------
// El puntero visible
// ---------------------------------------------------------------------------
//
// El rayo de la mano dice QUE pixel del motor hay debajo (eso ya lo hacia
// `panel_mesa_rayo_a_pixel`), pero sin dibujarse el jugador no ve donde
// esta apuntando: el clic XFCE sale a ciegas.
//
// El puntero es un ANILLO tumbado en el plano del panel, en el punto del
// rayo.  Se genera en el espacio LOCAL del panel (el mismo en el que vive
// el quad unitario), con el tamano dado en radios locales: el motor lo
// escala a metros con la matriz del panel.

/// Numero de segmentos del anillo del puntero.
#define PUNTERO_SEGMENTOS 16

/// Vertices que ocupa el anillo (2 triangulos por segmento).
#define PUNTERO_VERTICES (PUNTERO_SEGMENTOS * 6)

int panel_mesa_vertices_puntero(float hx, float hy, float radio_x, float radio_y,
                                float altura_local, PanelMesaVertice* out,
                                int max_vertices) {
    if (out == nullptr || max_vertices < PUNTERO_VERTICES) return 0;
    if (radio_x <= 0.0f || radio_y <= 0.0f) return 0;

    // El anillo: elipses de radio interior (0,55) y exterior.
    const float ri_x = radio_x * 0.55f;
    const float ri_y = radio_y * 0.55f;

    int n = 0;
    for (int s = 0; s < PUNTERO_SEGMENTOS; ++s) {
        const float a0 = 6.28318530718f * (float)s / (float)PUNTERO_SEGMENTOS;
        const float a1 = 6.28318530718f * (float)(s + 1) / (float)PUNTERO_SEGMENTOS;
        const float c0 = cosf(a0), s0 = sinf(a0);
        const float c1 = cosf(a1), s1 = sinf(a1);

        // Los cuatro puntos del cuadrilatero del segmento.
        const float xi[4] = { hx + ri_x * c0, hx + radio_x * c1,
                              hx + radio_x * c0, hx + ri_x * c1 };
        const float yi[4] = { hy + ri_y * s0, hy + radio_y * s1,
                              hy + radio_y * s0, hy + ri_y * s1 };
        // UV sin sentido (el puntero no usa textura): se pone la del punto.
        const float ui[4] = { 0.0f, 1.0f, 1.0f, 0.0f };
        const float vi[4] = { 0.0f, 0.0f, 1.0f, 1.0f };

        // Dos triangulos: (0,1,2) y (0,2,3).
        const int orden[6] = { 0, 1, 2, 0, 2, 3 };
        for (int k = 0; k < 6; ++k) {
            const int j = orden[k];
            out[n].x = xi[j];
            out[n].y = yi[j];
            out[n].z = altura_local;
            out[n].nx = 0.0f;
            out[n].ny = 0.0f;
            out[n].nz = 1.0f;
            out[n].u = ui[j];
            out[n].v = vi[j];
            ++n;
        }
    }
    return n;
}

// ---------------------------------------------------------------------------
// Rayo <-> pixel
// ---------------------------------------------------------------------------

bool panel_mesa_rayo_a_pixel(const float* origen, const float* dir,
                              const float* model_panel, int img_w, int img_h,
                              int* out_x, int* out_y) {
    if (origen == nullptr || dir == nullptr || model_panel == nullptr) return false;
    if (img_w <= 0 || img_h <= 0) return false;

    float inv[16];
    if (!mat_inversa(model_panel, inv)) return false;

    // El rayo pasa al espacio local del panel (donde el quad esta en z = 0).
    float o_local[3], d_local[3];
    mat_punto(inv, origen, o_local);
    mat_direccion(inv, dir, d_local);

    if (std::fabs(d_local[2]) < 1e-9f) return false;  // paralelo al panel
    const float t = -o_local[2] / d_local[2];
    if (t < 0.0f) return false;  // el panel queda detras del rayo

    const float hx = o_local[0] + t * d_local[0];
    const float hy = o_local[1] + t * d_local[1];
    if (hx < -1.0f - kEps || hx > 1.0f + kEps) return false;
    if (hy < -1.0f - kEps || hy > 1.0f + kEps) return false;

    // Local -> UV.  La V va invertida: la parte de ARRIBA de la imagen es
    // v = 0 (y local = +1).
    float u = (hx + 1.0f) * 0.5f;
    float v = (1.0f - hy) * 0.5f;
    if (u < 0.0f) u = 0.0f;
    if (u > 1.0f) u = 1.0f;
    if (v < 0.0f) v = 0.0f;
    if (v > 1.0f) v = 1.0f;

    int px = static_cast<int>(std::floor(u * static_cast<float>(img_w)));
    int py = static_cast<int>(std::floor(v * static_cast<float>(img_h)));
    if (px < 0) px = 0;
    if (px > img_w - 1) px = img_w - 1;
    if (py < 0) py = 0;
    if (py > img_h - 1) py = img_h - 1;

    if (out_x != nullptr) *out_x = px;
    if (out_y != nullptr) *out_y = py;
    return true;
}

bool panel_mesa_pixel_a_punto(const float* model_panel, int img_w, int img_h,
                               int px, int py, float* out3) {
    if (model_panel == nullptr || out3 == nullptr) return false;
    if (img_w <= 0 || img_h <= 0) return false;
    if (px < 0 || px >= img_w) return false;
    if (py < 0 || py >= img_h) return false;

    // Pixel -> UV (centro del pixel) -> local.  La V invertida: la fila 0
    // (ARRIBA) es v = 0, o sea y local = +1.
    const float u = (static_cast<float>(px) + 0.5f) / static_cast<float>(img_w);
    const float v = (static_cast<float>(py) + 0.5f) / static_cast<float>(img_h);
    const float local[3] = {u * 2.0f - 1.0f, 1.0f - v * 2.0f, 0.0f};
    mat_punto(model_panel, local, out3);
    return true;
}

void panel_mesa_normal_mundo(const float* model_panel, float* out3) {
    if (out3 == nullptr) return;
    if (model_panel == nullptr) {
        out3[0] = 0.0f;
        out3[1] = 1.0f;
        out3[2] = 0.0f;
        return;
    }
    // La normal local es (0,0,1).  Como la escala del panel es (sx, sy, 1)
    // (la Z no se escala), la parte lineal de la matriz la lleva tal cual.
    const float local[3] = {0.0f, 0.0f, 1.0f};
    mat_direccion(model_panel, local, out3);
    const float largo = std::sqrt(out3[0] * out3[0] + out3[1] * out3[1] + out3[2] * out3[2]);
    if (largo > 1e-9f) {
        out3[0] /= largo;
        out3[1] /= largo;
        out3[2] /= largo;
    }
}
