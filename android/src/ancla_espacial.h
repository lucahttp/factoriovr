// ancla_espacial.h — Anclaje espacial del diorama de Factorio a la MESA REAL
// ============================================================================
//
// MÓDULO AUTÓNOMO. No toca el resto del visor: solo expone una API que el
// motor puede opcionalmente usar para clavar la fábrica en la mesa física que
// ve el visor de la Pico 4 Ultra, en vez de dejarla flotando en el espacio
// STAGE a 0,74 m del suelo.
//
// ---------------------------------------------------------------------------
// CÓMO SE USA (resumen rápido)
//
//   1) Tras crear la sesión OpenXR, una sola vez:
//
//        xfv::ancla_espacial_iniciar(instancia, sesion, sistema);
//
//      Devuelve false si el runtime no tiene las extensiones de sensing
//      espacial (o si no están habilitadas). En ese caso el módulo queda
//      INACTIVO y el visor sigue usando el STAGE de siempre.
//
//   2) Cuando el usuario ha placedo el visor frente a la mesa y la ha
//      confirmado (pulsación del gatillo / botón de "fijar mesa"), el visor
//      llama:
//
//        xfv::ancla_espacial_preparar_mesa();   // arranca la detección
//
//      y en CADA frame del bucle de render:
//
//        xfv::ancla_espacial_actualizar(frameState);
//
//   3) Para dibujar, el visor pide la matriz del ancla:
//
//        float m[16];
//        xfv::ancla_espacial_matriz(m);   // column-major, como el resto del visor
//
//      Si NO hay ancla válida, la matriz es SIEMPRE la identidad (nunca queda
//      sin inicializar) y el visor debe seguir con su ruta habitual (STAGE).
//
//   4) Opcional: guardar y recuperar la mesa entre sesiones:
//
//        xfv::ancla_espacial_persistir("mesa_cocina");
//        xfv::ancla_espacial_restaurar("mesa_cocina");
//
// ---------------------------------------------------------------------------
// QUÉ HACE EL VISOR MIENTRAS NO HAY ANCLA
//
// Mientras no exista un ancla válida (extensión ausente, mesa no confirmada,
// tracking perdido, error del runtime) el módulo se comporta así:
//
//   * ancla_espacial_disponible()      -> false
//   * ancla_espacial_actualizar(...)   -> false
//   * ancla_espacial_matriz(m)         -> m = IDENTIDAD (siempre, sin NaNs)
//   * ancla_espacial_estado()          -> texto corto ("sin extension",
//                                          "esperando mesa", "mesa perdida"...)
//
// El visor DEBE seguir entonces con su modelo de tabla habitual en STAGE
// (model_mesa con la altura fija de 0,74 m). Es decir: la ruta con ancla es
// una MEJORA opcional, nunca un requisito. Si el módulo no está inicializado
// o falla cualquier llamada OpenXR, se degrada a "sin ancla" y no rompe nada.
//
// ---------------------------------------------------------------------------
// COMPOSICIÓN DE MATRICES (column-major, estilo OpenGL ES del visor)
//
// La matriz que devuelve ancla_espacial_matriz() es la POSE DEL ANCLA
// (la superficie horizontal de la mesa) EXPRESADA EN EL ESPACIO STAGE.
//
// Para colocar la fábrica tumbada sobre la mesa real, el visor compone:
//
//     final = ancla_matriz * (rot_y(yaw_local) * escala * desplazamiento_local)
//
// es decir: la pose real detectada aporta la traslación y la orientación
// base; el yaw local, la escala 1:1 y el pequeño despegue los pone el visor.
// La ruta antigua (model_mesa absoluto en STAGE) se usa tal cual cuando no
// hay ancla.
//
// ---------------------------------------------------------------------------
// ESCALA 1:1 Y ALTURA (ver ADR-014 para el razonamiento)
//
//   * 1 casilla de Factorio = 32 px en la imagen del motor = 0,04 m reales.
//     Una casilla del mapa son 4 cm sobre la mesa de verdad.
//   * La fábrica se dibuja TUMBADA: el plano XZ del diorama es el plano de la
//     mesa y la altura (Y) crece hacia arriba del viewer.
//   * Despegue sobre la superficie: 2 mm, para que no haya z-fighting con
//     la superficie detectada.
//
// ---------------------------------------------------------------------------
// PERSISTENCIA
//
// La persistencia de XR_BD_spatial_anchor es por UUID (no por nombre). Este
// módulo mantiene una tabla etiqueta -> UUID y, si se le da una ruta de
// fichero con ancla_espacial_fijar_ruta_persistencia(), la guarda en disco
// (una línea "etiqueta uuid_hex" por ancla). Sin ruta configurada la tabla
// vive solo en memoria y se pierde al cerrar la app (documentado en el ADR).
//
// ============================================================================

#pragma once

#include <openxr/openxr.h>

namespace xfv {

// ---------------------------------------------------------------------------
// CONSTANTES DE ESCALA (ver ADR-014)
// ---------------------------------------------------------------------------

// Metros reales que ocupa 1 casilla de Factorio (32 px del motor = 0,04 m).
inline constexpr float kEscalaCasillaM = 0.04f;

// Altura del diorama sobre la superficie detectada, en metros (2 mm).
inline constexpr float kAlturaDioramaM = 0.002f;

// ---------------------------------------------------------------------------
// API PRINCIPAL
// ---------------------------------------------------------------------------

// Comprueba si el runtime soporta el sensing espacial de ByteDance
// (XR_BD_spatial_sensing + XR_BD_spatial_anchor, y XR_BD_spatial_plane o
// XR_BD_spatial_scene para detectar la mesa) y deja el módulo preparado.
//
// Devuelve true si el módulo queda ACTIVO, false si se queda inactivo
// (fallback a STAGE). Nunca falla de forma catastrófica: como mucho, se
// queda inactivo y avisa por log.
bool ancla_espacial_iniciar(XrInstance instancia, XrSession sesion, XrSystemId sistema);

// true si el módulo está activo (extensiones presentes y cargadas).
bool ancla_espacial_disponible();

// Arranca la captura de superficies para encontrar la mesa.
// El llamador debería llamarlo cuando el usuario haya confirmado la mesa.
// Devuelve true si los proveedores de sensing pudieron crearse.
bool ancla_espacial_preparar_mesa();

// Fuerza el anclaje sobre la mejor superficie detectada AHORA (el usuario
// acaba de confirmar "aquí está la mesa"). Opcional: si no se llama, el
// módulo se auto-ancla cuando la superficie lleva varios frames estable.
bool ancla_espacial_confirmar_mesa();

// Actualiza el estado del ancla y su pose. Se llama una vez por frame.
// Devuelve true si HAY un ancla válida y trackeada.
bool ancla_espacial_actualizar(XrFrameState frame);

// Escribe en out16 (16 floats, column-major) la pose del ancla en STAGE.
// Si no hay ancla, escribe SIEMPRE la identidad. out16 puede ser nullptr
// (se ignora).
void ancla_espacial_matriz(float* out16);

// Persiste el ancla actual en el almacenamiento local del dispositivo con la
// etiqueta dada. Requiere soporte de persistencia de anchors.
bool ancla_espacial_persistir(const char* etiqueta);

// Restaura un ancla persistido por etiqueta y lo deja como ancla activa.
bool ancla_espacial_restaurar(const char* etiqueta);

// Texto corto con el estado del módulo, para el log del visor:
// "sin extension", "extension inactiva", "esperando mesa", "anclado",
// "persistido", "mesa perdida", "anclando"...
const char* ancla_espacial_estado();

// ---------------------------------------------------------------------------
// ESCALA Y ALTURA (lectura para el visor)
// ---------------------------------------------------------------------------

// Devuelve los parámetros 1:1 del diorama.
//   escala_casilla -> metros reales por casilla de Factorio (0,04)
//   altura_diorama  -> metros de despegue sobre la mesa (0,002)
// Si alguna salida es nullptr se ignora. Devuelve false si el módulo no
// está activo (los valores siguen siendo válidos: son constantes del diseño).
bool ancla_espacial_parametros(float* escala_casilla, float* altura_diorama);

// Fija la ruta de un fichero donde guardar la tabla etiqueta -> UUID de los
// anclas persistidos. Si es nullptr o "" la tabla se queda solo en memoria.
// La ruta debe ser escribible por la app (p.ej. el directorio files del
// NativeActivity).
void ancla_espacial_fijar_ruta_persistencia(const char* ruta);

// ---------------------------------------------------------------------------
// UTILIDADES EXPORTADAS PARA EL VISOR
// ---------------------------------------------------------------------------

// Devuelve la posición (x,y,z, metros) del ancla en STAGE.
bool ancla_espacial_posicion(float out_xyz[3]);

// Devuelve la escala 1:1 total del diorama (metros por casilla) ya con el
// despegue aplicado en la altura (alto = 0.002 m). Sirve para que el visor
// sepa cómo de grande es la fábrica sobre la mesa.
float ancla_espacial_escala_casilla();

}  // namespace xfv
