// puente_entrada.h — Cliente del protocolo "XFCE" del puente de entrada.
//
// CONTEXTO
// --------
// El motor de Factorio corre dentro de la Pico y dibuja el mundo. El
// inyector (LD_PRELOAD dentro del proceso del motor) manda los fotogramas al
// APK por TCP; ese canal es el de salida (XFV1, puerto 27020).
//
// Este modulo es el canal de ENTRADA: el inyector hace de SERVIDOR en
// 127.0.0.1:27021 y el APK (este modulo) es el CLIENTE que manda los clics.
// Cada clic viaja como un mensaje XFCE y el motor lo recibe como si fuera su
// raton (SDL_PushEvent), de modo que el motor aplica sus propias reglas:
// seleccion, fantasma de construccion, rotacion, deshacer...
//
// PROTOCOLO "XFCE" (little-endian, TCP)
// --------------------------------------
//   cabecera (7 bytes): magic u32 = 0x45434658 ("XFCE"), tipo u8 (1 = clic),
//                        tamano u16 (bytes de la carga)
//   carga del clic (6 bytes): x u16 (pixel X en la imagen TRANSMITIDA),
//                        y u16 (pixel Y en la imagen TRANSMITIDA),
//                        boton u8 (1=izquierdo, 2=medio, 3=derecho),
//                        accion u8 (1=abajo, 2=arriba, 3=clic completo)
//
// Es byte a byte lo mismo que arma scripts/enviar_clic.py.
//
// ACCION: ARMAR-SOLTAR
// -------------------
// La accion NO es un "clic completo" cuando el motor necesita el gesto
// completo. El flujo recomendado es ARMAR-SOLTAR sobre la MISMA posicion:
//
//     if (el_usuario_empuño_el_rayo) {
//         xfv::puente_entrada_clic(px, py, 1, 1);   // abajo  (armar)
//     }
//     if (el_usuario_solto_el_rayo) {
//         xfv::puente_entrada_clic(px, py, 1, 2);   // arriba (soltar)
//     }
//
// Al soltar se manda la MISMA posicion que se armo: asi el motor ve un clic
// completo y aplica sus reglas. Si en vez mandamos accion=3 (clic completo)
// el motor recibe un golpe de raton sin la secuencia de pulsacion, y algunas
// acciones del Factorio (seleccionar con arrastre, rotar, colocar) no
//responden igual. Por eso el helper `puente_entrada_arma_soltar` de abajo
//guarda la ultima posicion armada y la reutiliza en el "arriba".
//
// CONCURRENCIA
// ------------
// El hilo de render (OpenXR) llama a puente_entrada_clic(); el hilo de red
// escribe. El hilo de render NUNCA bloquea: como mucho mete el mensaje en
// una cola (maximo 32 pendientes) y sigue. El hilo de red se encarga de
// reconectar solo si el motor (inyector) se reinicia.
//
// ESTE MODULO NO DEPENDE DE ANDROID: solo sockets POSIX, para poder probarlo
// en el escritorio con el banco android/tests/test_puente_entrada.cpp.
//
// EJEMPLO DE USO
// --------------
//     #include "puente_entrada.h"
//
//     // al arrancar la app:
//     xfv::puente_entrada_iniciar("127.0.0.1", 27021);
//
//     // en el bucle de render, cuando el rayo de la mano golpea la mesa:
//     if (golpe_de_la_mano) {
//         int px = (int)hit.pixel_x, py = (int)hit.pixel_y;
//         if (rayo_bajo) {
//             xfv::puente_entrada_arma_soltar(px, py, 1);   // abajo
//         } else if (rayo_suelto) {
//             xfv::puente_entrada_suelta(px, py, 1);        // arriba
//         }
//     }
//
//     // al cerrar la app:
//     xfv::puente_entrada_cerrar();

#ifndef PUENTE_ENTRADA_H
#define PUENTE_ENTRADA_H

#include <cstddef>
#include <cstdint>
#include <vector>

namespace xfv {

// Tamano maximo de la cola de pendientes (mensajes que aun no salieron).
constexpr size_t PUENTE_ENTRADA_MAX_PENDIENTES = 32;

// Valor magico del protocolo XFCE, en little-endian ("XFCE").
constexpr uint32_t PUENTE_ENTRADA_MAGIC = 0x45434658u;
// Tipos de mensaje del protocolo.
constexpr uint8_t PUENTE_ENTRADA_TIPO_CLIC = 1;
constexpr uint8_t PUENTE_ENTRADA_TIPO_TECLA = 2;
constexpr uint8_t PUENTE_ENTRADA_TIPO_RUEDA = 3;
constexpr uint8_t PUENTE_ENTRADA_TIPO_MOVIMIENTO = 4;

// Bytes de la carga por tipo.
constexpr uint16_t PUENTE_ENTRADA_TAMANO_CARGA = 6;
constexpr uint16_t PUENTE_ENTRADA_TAMANO_CARGA_CLIC = 6;
constexpr uint16_t PUENTE_ENTRADA_TAMANO_CARGA_TECLA = 4;
constexpr uint16_t PUENTE_ENTRADA_TAMANO_CARGA_RUEDA = 4;
constexpr uint16_t PUENTE_ENTRADA_TAMANO_CARGA_MOV = 4;

// Scancodes comunes de SDL2 para Factorio
constexpr uint16_t KEY_A = 4;
constexpr uint16_t KEY_D = 7;
constexpr uint16_t KEY_E = 8;
constexpr uint16_t KEY_Q = 20;
constexpr uint16_t KEY_R = 21;
constexpr uint16_t KEY_S = 22;
constexpr uint16_t KEY_W = 26;
constexpr uint16_t KEY_ESC = 41;

// Acciones del protocolo.
enum Accion : uint8_t {
    kAccionAbajo = 1,   // bajar el boton / pulsar tecla
    kAccionArriba = 2,  // soltar el boton / soltar tecla
    kAccionClic = 3     // clic completo (no recomendado: ver arriba)
};

// Botones del protocolo.
enum Boton : uint8_t {
    kBotonIzquierdo = 1,
    kBotonMedio = 2,
    kBotonDerecho = 3
};

// Arma (byte a byte) mensajes XFCE. Funciones PURAS: no tocan red ni estado.
std::vector<uint8_t> puente_entrada_arma_clic(int x, int y, int boton, int accion);
std::vector<uint8_t> puente_entrada_arma_tecla(uint16_t scancode, int accion);
std::vector<uint8_t> puente_entrada_arma_rueda(int dx, int dy);
std::vector<uint8_t> puente_entrada_arma_movimiento(int x, int y);

// Arranca el puente de entrada. Crea el socket de forma NO bloqueante y
// lanza el hilo de red, que es el que se encarga de conectar y reconectar.
// Devuelve false si el host/puerto no son validos.
// Tras esta llamada, puente_entrada_conectado() puede seguir siendo false un
// rato: la conexion se establece en segundo plano.
bool puente_entrada_iniciar(const char* host, int puerto);

// Cierra el puente: para el hilo de red, cierra el socket y vacia la cola.
// Se puede llamar aunque nunca se haya iniciado.
void puente_entrada_cerrar();

// true si el socket con el motor esta conectado ahora mismo.
bool puente_entrada_conectado();

// Envia un clic por el puente de entrada.
bool puente_entrada_clic(int x, int y, int boton, int accion);

// Envia una pulsacion o soltura de tecla (scancode de SDL2).
bool puente_entrada_tecla(uint16_t scancode, int accion);

// Envia movimiento de la rueda del raton (zoom in/out en Factorio).
bool puente_entrada_rueda(int dx, int dy);

// Envia movimiento del cursor del raton (sin clic).
bool puente_entrada_movimiento(int x, int y);

// FLUJO ARMAR-SOLTAR (recomendado).
// "arma" manda accion=1 (abajo) y recuerda la posicion; "suelta" manda
// accion=2 (arriba) sobre la MISMA posicion que se armo, para que el motor
// vea un clic completo. Si no se armo antes, "suelta" usa la posicion que se
// le pase.
bool puente_entrada_arma_soltar(int x, int y, int boton);
bool puente_entrada_suelta(int x, int y, int boton);

// Ultimo error registrado (cadena estable en memoria; "" si no hay error).
const char* puente_entrada_ultimo_error();

// Numero de mensajes en la cola de pendientes (para diagnostico y banco).
size_t puente_entrada_pendientes();

// Version del protocolo que entiende este modulo (solo informativa).
const char* puente_entrada_version();

}  // namespace xfv

// Atajos en el espacio global para no obligar a escribir "xfv::" en todas
// partes. Si en un fichero se hace "using namespace xfv;" NO se usen estos
// atajos en ese fichero (quedarian ambiguos).
inline bool puente_entrada_iniciar(const char* host, int puerto) {
    return xfv::puente_entrada_iniciar(host, puerto);
}
inline void puente_entrada_cerrar() { xfv::puente_entrada_cerrar(); }
inline bool puente_entrada_conectado() { return xfv::puente_entrada_conectado(); }
inline bool puente_entrada_clic(int x, int y, int boton, int accion) {
    return xfv::puente_entrada_clic(x, y, boton, accion);
}
inline bool puente_entrada_tecla(uint16_t scancode, int accion) {
    return xfv::puente_entrada_tecla(scancode, accion);
}
inline bool puente_entrada_rueda(int dx, int dy) {
    return xfv::puente_entrada_rueda(dx, dy);
}
inline bool puente_entrada_movimiento(int x, int y) {
    return xfv::puente_entrada_movimiento(x, y);
}
inline const char* puente_entrada_ultimo_error() { return xfv::puente_entrada_ultimo_error(); }
inline std::vector<uint8_t> xfv_arma_clic(int x, int y, int boton, int accion) {
    return xfv::puente_entrada_arma_clic(x, y, boton, accion);
}

#endif  // PUENTE_ENTRADA_H
