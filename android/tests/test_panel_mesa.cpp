// test_panel_mesa.cpp — Banco de escritorio del modulo del panel tumbado.
//
// Compila y corre en el Mac (clang++/g++), sin NDK ni OpenXR: el modulo
// (android/src/panel_mesa.{h,cpp}) es matematicas puras.
//
// Que comprueba:
//   1. Que la matriz tumbada deja el quad en el plano XZ (la normal (0,0,1)
//      pasa a (0,1,0)) y que la escala es la esperada (1 casilla = 0.04 m,
//      distancia entre texels y tamaño total del panel).
//   2. Que un rayo vertical que apunta al centro del panel devuelve el
//      pixel central de la imagen.
//   3. Que un rayo que apunta a una esquina devuelve la esquina correcta.
//   4. Que un rayo que no toca el panel devuelve false.
//   5. Que el pixel Y sale con la fila 0 = ARRIBA.
//   6. La conversion inversa pixel -> punto del panel.

#include "../src/panel_mesa.h"

#include <cmath>
#include <cstdio>
#include <string>

namespace {

int fallos = 0;

void comprobar(bool condicion, const std::string& que) {
    if (condicion) {
        std::printf("  [OK]   %s\n", que.c_str());
    } else {
        std::printf("  [FALLO] %s\n", que.c_str());
        ++fallos;
    }
}

void comprobar_cerca(double valor, double esperado, double tolerancia,
                     const std::string& que) {
    const bool ok = std::fabs(valor - esperado) <= tolerancia;
    if (ok) {
        std::printf("  [OK]   %s\n", que.c_str());
    } else {
        std::printf("  [FALLO] %s (obtenido %.6f, esperado %.6f)\n", que.c_str(), valor,
                    esperado);
        ++fallos;
    }
}

// --- Utilidades de matriz propias del banco (independientes del modulo) ----
// Column-major, como el resto del visor.

void mat_identidad(float* m) {
    for (int i = 0; i < 16; ++i) m[i] = 0.0f;
    m[0] = m[5] = m[10] = m[15] = 1.0f;
}

void mat_multiplica(const float* a, const float* b, float* out) {
    float tmp[16];
    for (int c = 0; c < 4; ++c) {
        for (int r = 0; r < 4; ++r) {
            tmp[c * 4 + r] = a[r] * b[c * 4] + a[4 + r] * b[c * 4 + 1] +
                             a[8 + r] * b[c * 4 + 2] + a[12 + r] * b[c * 4 + 3];
        }
    }
    for (int i = 0; i < 16; ++i) out[i] = tmp[i];
}

void mat_traslacion(float* m, float x, float y, float z) {
    mat_identidad(m);
    m[12] = x;
    m[13] = y;
    m[14] = z;
}

void mat_rotacion_y(float* m, float rad) {
    const float c = std::cos(rad);
    const float s = std::sin(rad);
    mat_identidad(m);
    m[0] = c;  m[8] = s;
    m[2] = -s; m[10] = c;
}

// Punto local del panel (x,y,z) al espacio del mundo.
void punto_mundo(const float* m, float x, float y, float z, float* out) {
    out[0] = m[0] * x + m[4] * y + m[8] * z + m[12];
    out[1] = m[1] * x + m[5] * y + m[9] * z + m[13];
    out[2] = m[2] * x + m[6] * y + m[10] * z + m[14];
}

// Direccion local al espacio del mundo (sin traslacion).
void direccion_mundo(const float* m, float x, float y, float z, float* out) {
    out[0] = m[0] * x + m[4] * y + m[8] * z;
    out[1] = m[1] * x + m[5] * y + m[9] * z;
    out[2] = m[2] * x + m[6] * y + m[10] * z;
}

double distancia(const float* a, const float* b) {
    const double dx = a[0] - b[0];
    const double dy = a[1] - b[1];
    const double dz = a[2] - b[2];
    return std::sqrt(dx * dx + dy * dy + dz * dz);
}

// Rayo vertical desde 1 m por encima del punto indicado.
void rayo_desde_arriba(const float* punto, float* origen, float* dir) {
    origen[0] = punto[0];
    origen[1] = punto[1] + 1.0f;
    origen[2] = punto[2];
    dir[0] = 0.0f;
    dir[1] = -1.0f;
    dir[2] = 0.0f;
}

constexpr int kImgW = 640;
constexpr int kImgH = 360;

}  // namespace

int main() {
    std::printf("Banco del panel tumbado (panel_mesa)\n");

    PanelMesaConfig cfg;
    panel_mesa_config_inicial(&cfg);
    comprobar(cfg.img_w == 640 && cfg.img_h == 360, "config por defecto: imagen 640x360");
    comprobar_cerca(cfg.px_por_casilla, 32.0, 1e-6, "config por defecto: 32 px por casilla");
    comprobar_cerca(cfg.m_por_casilla, 0.04, 1e-6, "config por defecto: 0.04 m por casilla");

    // ------------------------------------------------------------------
    // 1. Matriz tumbada: plano XZ, normal arriba, escala correcta
    // ------------------------------------------------------------------
    float mesa[16];
    mat_identidad(mesa);
    float panel[16];
    panel_mesa_matriz_tumbado(mesa, &cfg, panel);

    float normal[3];
    panel_mesa_normal_mundo(panel, normal);
    comprobar_cerca(normal[0], 0.0, 1e-5, "la normal del quad pasa a X = 0");
    comprobar_cerca(normal[1], 1.0, 1e-5, "la normal del quad pasa a Y = 1 (apunta ARRIBA)");
    comprobar_cerca(normal[2], 0.0, 1e-5, "la normal del quad pasa a Z = 0");

    // Tamaño: 640 px / 32 px por casilla = 20 casillas = 0.80 m.
    float ancho = 0.0f;
    float fondo = 0.0f;
    panel_mesa_tamano(&cfg, &ancho, &fondo);
    comprobar_cerca(ancho, 0.80, 1e-5, "ancho del panel: 20 casillas = 0.80 m");
    comprobar_cerca(fondo, 0.45, 1e-5, "fondo del panel: 11.25 casillas = 0.45 m");

    // Las cuatro esquinas locales (-1,-1) .. (1,1) en el mundo.
    float inf_izq[3], inf_der[3], sup_izq[3], sup_der[3];
    punto_mundo(panel, -1.0f, -1.0f, 0.0f, inf_izq);
    punto_mundo(panel, 1.0f, -1.0f, 0.0f, inf_der);
    punto_mundo(panel, -1.0f, 1.0f, 0.0f, sup_izq);
    punto_mundo(panel, 1.0f, 1.0f, 0.0f, sup_der);

    comprobar_cerca(inf_der[0] - inf_izq[0], 0.80, 1e-5, "el panel mide 0.80 m en X");
    comprobar_cerca(std::fabs(sup_izq[2] - inf_izq[2]), 0.45, 1e-5, "el panel mide 0.45 m en Z");
    comprobar_cerca(inf_izq[1], cfg.altura_m, 1e-5, "el panel queda a la altura pedida");
    comprobar_cerca(inf_izq[1], inf_der[1], 1e-5, "el panel es plano (misma Y en las esquinas)");
    // ARRIBA de la imagen (y local = +1) -> -Z (lejos del jugador).
    comprobar(sup_izq[2] < inf_izq[2], "la parte de ARRIBA de la imagen queda en -Z (lejos)");
    // DERECHA de la imagen (x local = +1) -> +X.
    comprobar(inf_der[0] > inf_izq[0], "la parte de DERECHA de la imagen queda en +X");

    // Distancia entre dos texels vecinos (1 px de diferencia).
    float p0[3], p1[3];
    panel_mesa_pixel_a_punto(panel, kImgW, kImgH, 100, 100, p0);
    panel_mesa_pixel_a_punto(panel, kImgW, kImgH, 101, 100, p1);
    comprobar_cerca(distancia(p0, p1), 0.80 / static_cast<double>(kImgW), 1e-5,
                    "un texel mide 0.80/640 m de ancho (escala correcta)");

    // ------------------------------------------------------------------
    // 2. Rayo vertical al centro -> pixel central
    // ------------------------------------------------------------------
    float origen[3], dir[3];
    float centro[3];
    punto_mundo(panel, 0.0f, 0.0f, 0.0f, centro);
    rayo_desde_arriba(centro, origen, dir);
    int px = -1, py = -1;
    const bool ok_centro = panel_mesa_rayo_a_pixel(origen, dir, panel, kImgW, kImgH, &px, &py);
    comprobar(ok_centro, "el rayo vertical del centro toca el panel");
    comprobar(px == kImgW / 2, "el pixel X del centro es 320");
    comprobar(py == kImgH / 2, "el pixel Y del centro es 180");

    // ------------------------------------------------------------------
    // 3. Rayo a las esquinas -> la esquina correcta
    // ------------------------------------------------------------------
    struct Esquina {
        float lx, ly;
        int ex, ey;
        const char* nombre;
    };
    const Esquina esquinas[4] = {
        {-1.0f, 1.0f, 0, 0, "superior izquierda"},
        {1.0f, 1.0f, kImgW - 1, 0, "superior derecha"},
        {-1.0f, -1.0f, 0, kImgH - 1, "inferior izquierda"},
        {1.0f, -1.0f, kImgW - 1, kImgH - 1, "inferior derecha"},
    };
    for (int i = 0; i < 4; ++i) {
        float punto[3];
        punto_mundo(panel, esquinas[i].lx, esquinas[i].ly, 0.0f, punto);
        rayo_desde_arriba(punto, origen, dir);
        px = py = -1;
        const bool ok = panel_mesa_rayo_a_pixel(origen, dir, panel, kImgW, kImgH, &px, &py);
        char msg[128];
        std::snprintf(msg, sizeof(msg), "la esquina %s da el pixel (%d, %d)",
                      esquinas[i].nombre, esquinas[i].ex, esquinas[i].ey);
        comprobar(ok && px == esquinas[i].ex && py == esquinas[i].ey, msg);
    }

    // ------------------------------------------------------------------
    // 4. Rayos que NO tocant el panel
    // ------------------------------------------------------------------
    // Vertical, pero fuera del ancho del panel.
    float fuera[3];
    punto_mundo(panel, 2.0f, 0.0f, 0.0f, fuera);
    rayo_desde_arriba(fuera, origen, dir);
    comprobar(!panel_mesa_rayo_a_pixel(origen, dir, panel, kImgW, kImgH, &px, &py),
              "un rayo vertical fuera del panel devuelve false");

    // Horizontal, paralelo al panel tumbado.
    float centro2[3];
    punto_mundo(panel, 0.0f, 0.0f, 0.0f, centro2);
    origen[0] = centro2[0];
    origen[1] = centro2[1] + 0.5f;
    origen[2] = centro2[2];
    dir[0] = 1.0f;
    dir[1] = 0.0f;
    dir[2] = 0.0f;
    comprobar(!panel_mesa_rayo_a_pixel(origen, dir, panel, kImgW, kImgH, &px, &py),
              "un rayo paralelo al panel devuelve false");

    // Desde debajo del panel y hacia abajo: el panel queda detras.
    origen[0] = centro2[0];
    origen[1] = centro2[1] - 0.5f;
    origen[2] = centro2[2];
    dir[0] = 0.0f;
    dir[1] = -1.0f;
    dir[2] = 0.0f;
    comprobar(!panel_mesa_rayo_a_pixel(origen, dir, panel, kImgW, kImgH, &px, &py),
              "un rayo que se aleja del panel (panel detras) devuelve false");

    // ------------------------------------------------------------------
    // 5. Convencion de pixel: la fila 0 es la de ARRIBA
    // ------------------------------------------------------------------
    // El punto de la imagen con py = 0 esta en la parte de ARRIBA de la
    // imagen, o sea en el lado LEJOS de la mesa (-Z).
    float arriba[3], abajo[3];
    panel_mesa_pixel_a_punto(panel, kImgW, kImgH, 320, 0, arriba);
    panel_mesa_pixel_a_punto(panel, kImgW, kImgH, 320, kImgH - 1, abajo);
    comprobar(arriba[2] < abajo[2],
              "la fila 0 (py = 0) es la de ARRIBA de la imagen (lado -Z de la mesa)");

    // Y el rayo a la esquina de arriba da py = 0 (se repite aqui para dejar
    // la convencion escrita).
    {
        float org[3];
        punto_mundo(panel, -1.0f, 1.0f, 0.0f, org);
        rayo_desde_arriba(org, origen, dir);
        const bool ok = panel_mesa_rayo_a_pixel(origen, dir, panel, kImgW, kImgH, &px, &py);
        comprobar(ok && py == 0, "el rayo a la esquina de ARRIBA devuelve py = 0");
    }

    // ------------------------------------------------------------------
    // 6. Conversion inversa: pixel -> punto del panel
    // ------------------------------------------------------------------
    {
        const int prueba[4][2] = {{0, 0}, {kImgW - 1, 0}, {0, kImgH - 1}, {kImgW / 2, kImgH / 2}};
        bool todo = true;
        for (int i = 0; i < 4; ++i) {
            float punto[3];
            if (!panel_mesa_pixel_a_punto(panel, kImgW, kImgH, prueba[i][0], prueba[i][1], punto)) {
                todo = false;
                continue;
            }
            float org[3], d[3];
            rayo_desde_arriba(punto, org, d);
            int rx = -1, ry = -1;
            if (!panel_mesa_rayo_a_pixel(org, d, panel, kImgW, kImgH, &rx, &ry) || rx != prueba[i][0] ||
                ry != prueba[i][1]) {
                todo = false;
            }
        }
        comprobar(todo, "pixel -> punto -> rayo -> pixel da la vuelta exacta");
    }

    // La inversa fuera de rango falla.
    float fuera2[3];
    comprobar(!panel_mesa_pixel_a_punto(panel, kImgW, kImgH, -1, 0, fuera2),
              "pixel_a_punto con pixel fuera de rango devuelve false");
    comprobar(!panel_mesa_pixel_a_punto(panel, kImgW, kImgH, kImgW, 0, fuera2),
              "pixel_a_punto con pixel fuera de rango (X) devuelve false");

    // ------------------------------------------------------------------
    // 7. Con la mesa girada y desplazada (matriz de mesa real)
    // ------------------------------------------------------------------
    {
        float mesa2[16], rot[16], trans[16], tmp[16];
        mat_rotacion_y(rot, 0.7f);
        mat_traslacion(trans, 1.2f, 0.0f, -0.35f);
        mat_multiplica(trans, rot, tmp);
        mat_multiplica(tmp, mesa, mesa2);

        float panel2[16];
        panel_mesa_matriz_tumbado(mesa2, &cfg, panel2);

        // El centro del panel sigue en el origen del area de juego de la mesa,
        //大家一起 con el giro y la traslacion.
        float c2[3];
        punto_mundo(panel2, 0.0f, 0.0f, 0.0f, c2);
        float esperado[3];
        punto_mundo(mesa2, 0.0f, cfg.altura_m, 0.0f, esperado);
        char msg[160];
        std::snprintf(msg, sizeof(msg), "con la mesa girada el centro del panel sigue en su sitio (%.4f, %.4f, %.4f)",
                      c2[0], c2[1], c2[2]);
        comprobar_cerca(c2[0], esperado[0], 1e-4, msg);
        comprobar_cerca(c2[1], esperado[1], 1e-4, "con la mesa girada la altura del panel se respeta");
        comprobar_cerca(c2[2], esperado[2], 1e-4, "con la mesa girada la posicion Z del panel se respeta");

        // Y el rayo sigue dando el pixel central (la mesa no rota el pixel).
        float org2[3], d2[3];
        rayo_desde_arriba(c2, org2, d2);
        const bool ok = panel_mesa_rayo_a_pixel(org2, d2, panel2, kImgW, kImgH, &px, &py);
        comprobar(ok && px == kImgW / 2 && py == kImgH / 2,
                  "con la mesa girada el rayo al centro sigue dando el pixel central");

        // La normal en el mundo tambien gira con la mesa, pero sigue siendo
        // perpendicular a la mesa (mismo sentido que la Y de la mesa).
        float n2[3];
        panel_mesa_normal_mundo(panel2, n2);
        float y_mesa[3];
        direccion_mundo(mesa2, 0.0f, 1.0f, 0.0f, y_mesa);
        comprobar_cerca(n2[0], y_mesa[0], 1e-4, "la normal del panel es la Y de la mesa");
        comprobar_cerca(n2[1], y_mesa[1], 1e-4, "la normal del panel es la Y de la mesa (Y)");
        comprobar_cerca(n2[2], y_mesa[2], 1e-4, "la normal del panel es la Y de la mesa (Z)");
    }

    // ------------------------------------------------------------------
    // 8. Version "de mesa" (el panel cubre el area de la mesa)
    // ------------------------------------------------------------------
    {
        float panel3[16];
        panel_mesa_matriz_tumbado_mesa(mesa, 1.20f, 0.80f, 0.002f, 32.0f, 0.04f, panel3);
        float a[3], b[3];
        punto_mundo(panel3, -1.0f, 0.0f, 0.0f, a);
        punto_mundo(panel3, 1.0f, 0.0f, 0.0f, b);
        comprobar_cerca(b[0] - a[0], 1.20, 1e-5, "la version de mesa cubre 1.20 m de ancho");
        float c[3], d[3];
        punto_mundo(panel3, 0.0f, -1.0f, 0.0f, c);
        punto_mundo(panel3, 0.0f, 1.0f, 0.0f, d);
        comprobar_cerca(std::fabs(d[2] - c[2]), 0.80, 1e-5, "la version de mesa cubre 0.80 m de fondo");
        comprobar(panel_mesa_imagen_esperada(1.20f, 0.04f, 32.0f) == 960,
                  "1.20 m con 0.04 m/celda y 32 px/celda son 960 px de imagen");
    }

    // ------------------------------------------------------------------
    // 9. Escala de la imagen recibida (el inyector reduce con factores
    //    enteros) y factor de px por casilla
    // ------------------------------------------------------------------
    {
        const float px_casilla = panel_mesa_px_por_casilla_recibida(640, 1280, 32.0f);
        comprobar_cerca(px_casilla, 16.0, 1e-4,
                        "si el juego va a 1280 px y se reduce a 640, quedan 16 px por casilla");
        const float sin_reducir = panel_mesa_px_por_casilla_recibida(856, 856, 32.0f);
        comprobar_cerca(sin_reducir, 32.0, 1e-4, "sin reduccion son 32 px por casilla");
    }

    // ------------------------------------------------------------------
    // 10. Geometria: quad unitario en el plano XY con la V invertida
    // ------------------------------------------------------------------
    {
        PanelMesaVertice v[PANEL_MESA_VERTICES];
        panel_mesa_vertices(v);
        bool plano = true;
        bool normal_ok = true;
        bool uv_ok = true;
        for (int i = 0; i < PANEL_MESA_VERTICES; ++i) {
            if (v[i].z != 0.0f) plano = false;
            if (v[i].nx != 0.0f || v[i].ny != 0.0f || v[i].nz != 1.0f) normal_ok = false;
            if (v[i].x < -1.0f || v[i].x > 1.0f || v[i].y < -1.0f || v[i].y > 1.0f) plano = false;
        }
        // El vertice con y = +1 (ARRIBA de la imagen) tiene v = 0.
        for (int i = 0; i < PANEL_MESA_VERTICES; ++i) {
            if (v[i].y > 0.5f && v[i].v != 0.0f) uv_ok = false;
            if (v[i].y < -0.5f && v[i].v != 1.0f) uv_ok = false;
        }
        comprobar(plano, "la geometria es un quad unitario en el plano XY (z = 0)");
        comprobar(normal_ok, "la geometria tiene la normal (0,0,1)");
        comprobar(uv_ok, "la geometria tiene la V invertida (v = 0 es ARRIBA)");
    }

    std::printf("\n%s: %d fallo(s)\n", fallos == 0 ? "TODO CORRECTO" : "HAY FALLOS", fallos);
    return fallos == 0 ? 0 : 1;
}
