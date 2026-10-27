// puente_entrada.cpp — Cliente del protocolo "XFCE" (puente de entrada).
//
// Implementacion POSIX pura (no depende de Android ni de la NDK) para que el
// mismo codigo corra en el APK y en el banco de escritorio.
//
// Hilos:
//   - el hilo de render llama a puente_entrada_clic(): solo encola (o envia
//     si ya esta conectado) y sigue, sin bloquear.
//   - el hilo de red mantiene el socket vivo, reintenta la conexion con
//     backoff suave (maximo ~1 reintento/segundo) y entrega los pendientes.

#include "puente_entrada.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <netdb.h>
#include <netinet/tcp.h>
#include <poll.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cstdarg>
#include <condition_variable>
#include <deque>
#include <mutex>
#include <string>
#include <thread>

namespace xfv {
namespace {

constexpr int kTamanoCabecera = 7;
constexpr int kBackoffMinMs = 200;   // primer reintento tras caida
constexpr int kBackoffMaxMs = 1000;  // tope: ~1 reintento por segundo
constexpr int kTiempoConexionMs = 700;
constexpr int kEsperaIdleMs = 50;

struct Estado {
    std::mutex mutex;
    std::condition_variable cv;
    std::deque<std::vector<uint8_t> > pendientes;
    std::thread hilo;
    std::string host = "127.0.0.1";
    int puerto = 27021;
    int fd = -1;
    bool corriendo = false;   // el hilo debe seguir vivo
    bool conectado = false;   // el socket esta conectado
    bool hay_pendientes = false;
    int backoff_ms = kBackoffMinMs;
    // ultima posicion armada (flujo armar-soltar)
    bool armado = false;
    int armado_x = 0;
    int armado_y = 0;
    int armado_boton = 1;
};

Estado& estado() {
    static Estado e;
    return e;
}

// Ultimo error, con su propia cadena (vive fuera del mutex del estado).
struct Error {
    std::mutex mutex;
    std::string texto;
};
Error& error_global() {
    static Error e;
    return e;
}

void registrar_error(const char* formato, ...) {
    char buffer[512];
    va_list args;
    va_start(args, formato);
    vsnprintf(buffer, sizeof(buffer), formato, args);
    va_end(args);
    Error& e = error_global();
    std::lock_guard<std::mutex> lock(e.mutex);
    e.texto = buffer;
}

void limpiar_error() {
    Error& e = error_global();
    std::lock_guard<std::mutex> lock(e.mutex);
    e.texto.clear();
}

int ajustar(int valor, int minimo, int maximo) {
    if (valor < minimo) return minimo;
    if (valor > maximo) return maximo;
    return valor;
}

// Pone el socket en modo no bloqueante.
bool poner_no_bloqueante(int fd) {
    int flags = fcntl(fd, F_GETFL, 0);
    if (flags < 0) return false;
    return fcntl(fd, F_SETFL, flags | O_NONBLOCK) == 0;
}

// Cierra el socket de forma ordenada (despertando al motor con FIN).
void cerrar_socket(int fd) {
    if (fd < 0) return;
#ifdef SO_NOSIGPIPE
    // macOS: evita SIGPIPE al escribir en un socket cerrado por el otro lado.
    int on = 1;
    setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &on, sizeof(on));
#endif
    ::shutdown(fd, SHUT_RDWR);
    ::close(fd);
}

// Intenta conectar. Devuelve el fd conectado o -1.
int intentar_conectar(const std::string& host, int puerto) {
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_port = htons(static_cast<uint16_t>(puerto));

    std::string h = host.empty() ? std::string("127.0.0.1") : host;
    if (inet_pton(AF_INET, h.c_str(), &addr.sin_addr) != 1) {
        struct hostent* he = gethostbyname(h.c_str());
        if (he == nullptr || he->h_addrtype != AF_INET) {
            registrar_error("host invalido: %s", h.c_str());
            return -1;
        }
        memcpy(&addr.sin_addr, he->h_addr_list[0], sizeof(addr.sin_addr));
    }

    int fd = ::socket(AF_INET, SOCK_STREAM, 0);
    if (fd < 0) {
        registrar_error("no se pudo crear el socket: %s", strerror(errno));
        return -1;
    }
    if (!poner_no_bloqueante(fd)) {
        registrar_error("no se pudo poner el socket en no bloqueante: %s", strerror(errno));
        ::close(fd);
        return -1;
    }
    int uno = 1;
    setsockopt(fd, IPPROTO_TCP, TCP_NODELAY, &uno, sizeof(uno));

    int r = ::connect(fd, reinterpret_cast<struct sockaddr*>(&addr), sizeof(addr));
    if (r != 0 && errno != EINPROGRESS) {
        registrar_error("no se pudo conectar a %s:%d: %s", h.c_str(), puerto, strerror(errno));
        ::close(fd);
        return -1;
    }
    if (r != 0) {
        struct pollfd pfd;
        pfd.fd = fd;
        pfd.events = POLLOUT;
        pfd.revents = 0;
        int pr = ::poll(&pfd, 1, kTiempoConexionMs);
        if (pr <= 0) {
            registrar_error("tiempo agotado conectando a %s:%d", h.c_str(), puerto);
            ::close(fd);
            return -1;
        }
        int err = 0;
        socklen_t len = sizeof(err);
        if (getsockopt(fd, SOL_SOCKET, SO_ERROR, &err, &len) != 0 || err != 0) {
            registrar_error("no se pudo conectar a %s:%d: %s", h.c_str(), puerto,
                            strerror(err != 0 ? err : errno));
            ::close(fd);
            return -1;
        }
    }
    return fd;
}

// Intenta mandar el mensaje entero. Devuelve true si salio entero.
// `esperar` a true permite esperar a que el socket acepte (hilo de red);
// a false devuelve false en cuanto el socket esta lleno (hilo de render:
// nunca bloquea).
bool enviar_todo(int fd, const std::vector<uint8_t>& msg, bool esperar) {
    size_t enviados = 0;
    while (enviados < msg.size()) {
        ssize_t n = ::send(fd, msg.data() + enviados, msg.size() - enviados, 0);
        if (n > 0) {
            enviados += static_cast<size_t>(n);
            continue;
        }
        if (n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK)) {
            if (!esperar) return false;
            struct pollfd pfd;
            pfd.fd = fd;
            pfd.events = POLLOUT;
            pfd.revents = 0;
            int pr = ::poll(&pfd, 1, kTiempoConexionMs);
            if (pr <= 0) return false;
            continue;
        }
        return false;
    }
    return true;
}

// Hilo de red: mantiene el socket vivo y entrega los pendientes.
void hilo_red() {
    Estado& e = estado();
    for (;;) {
        {
            std::unique_lock<std::mutex> lock(e.mutex);
            if (!e.corriendo) return;
            if (e.fd < 0) {
                // Espera a que haya algo que hacer (o a que pare el modulo).
                e.cv.wait_for(lock, std::chrono::milliseconds(kBackoffMinMs));
                if (!e.corriendo) return;
            }
        }

        // 1) Conectar si hace falta.
        int fd = -1;
        {
            std::lock_guard<std::mutex> lock(e.mutex);
            fd = e.fd;
        }
        if (fd < 0) {
            std::string host;
            int puerto;
            {
                std::lock_guard<std::mutex> lock(e.mutex);
                host = e.host;
                puerto = e.puerto;
            }
            int nuevo = intentar_conectar(host, puerto);
            {
                std::lock_guard<std::mutex> lock(e.mutex);
                if (!e.corriendo) {
                    if (nuevo >= 0) cerrar_socket(nuevo);
                    return;
                }
                if (nuevo >= 0) {
                    e.fd = nuevo;
                    e.conectado = true;
                    e.backoff_ms = kBackoffMinMs;
                    limpiar_error();
                }
                fd = e.fd;
            }
            if (fd < 0) {
                // Backoff suave: espera growing hasta 1 segundo.
                std::unique_lock<std::mutex> lock(e.mutex);
                if (!e.corriendo) return;
                int espera = e.backoff_ms;
                e.backoff_ms = (e.backoff_ms * 2 > kBackoffMaxMs) ? kBackoffMaxMs : e.backoff_ms * 2;
                e.cv.wait_for(lock, std::chrono::milliseconds(espera), [&e]() {
                    return !e.corriendo || e.hay_pendientes;
                });
                continue;
            }
        }

        // 2) Si hay pendientes, mandarlos.
        bool hay = false;
        std::vector<uint8_t> msg;
        {
            std::lock_guard<std::mutex> lock(e.mutex);
            hay = !e.pendientes.empty();
            if (hay) {
                msg = e.pendientes.front();
                e.pendientes.pop_front();
                e.hay_pendientes = !e.pendientes.empty();
            }
        }
        if (hay) {
            bool ok = enviar_todo(fd, msg, true);
            if (!ok) {
                // Se cae el socket: devolvemos el mensaje a la cola.
                std::lock_guard<std::mutex> lock(e.mutex);
                e.pendientes.push_front(msg);
                e.hay_pendientes = true;
                if (e.fd >= 0) {
                    cerrar_socket(e.fd);
                    e.fd = -1;
                }
                e.conectado = false;
                e.backoff_ms = kBackoffMinMs;
                registrar_error("se perdio la conexion con el motor, se reintentara");
                e.cv.notify_all();
            }
            continue;
        }

        // 3) Nada que mandar: vigilamos el socket para notar caidas.
        struct pollfd pfd;
        pfd.fd = fd;
        pfd.events = POLLIN;
        pfd.revents = 0;
        int pr = ::poll(&pfd, 1, kEsperaIdleMs);
        bool caido = false;
        if (pr > 0 && (pfd.revents & (POLLHUP | POLLERR | POLLNVAL))) caido = true;
        if (pr < 0 && errno != EINTR) caido = true;
        if (caido) {
            std::lock_guard<std::mutex> lock(e.mutex);
            if (e.fd >= 0) {
                cerrar_socket(e.fd);
                e.fd = -1;
            }
            e.conectado = false;
            e.backoff_ms = kBackoffMinMs;
            registrar_error("el motor cerro la conexion, se reintentara");
            e.cv.notify_all();
        }
    }
}

}  // namespace

std::vector<uint8_t> puente_entrada_arma_clic(int x, int y, int boton, int accion) {
    const uint16_t xu = static_cast<uint16_t>(ajustar(x, 0, 0xFFFF));
    const uint16_t yu = static_cast<uint16_t>(ajustar(y, 0, 0xFFFF));
    const uint8_t bu = static_cast<uint8_t>(ajustar(boton, 1, 3));
    const uint8_t au = static_cast<uint8_t>(ajustar(accion, 1, 3));

    std::vector<uint8_t> msg;
    msg.reserve(kTamanoCabecera + PUENTE_ENTRADA_TAMANO_CARGA);
    // Magic u32 little-endian.
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_MAGIC & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 8) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 16) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 24) & 0xFFu));
    // Tipo u8 y tamano u16.
    msg.push_back(PUENTE_ENTRADA_TIPO_CLIC);
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_TAMANO_CARGA & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_TAMANO_CARGA >> 8) & 0xFFu));
    // Carga: x u16, y u16, boton u8, accion u8.
    msg.push_back(static_cast<uint8_t>(xu & 0xFFu));
    msg.push_back(static_cast<uint8_t>((xu >> 8) & 0xFFu));
    msg.push_back(static_cast<uint8_t>(yu & 0xFFu));
    msg.push_back(static_cast<uint8_t>((yu >> 8) & 0xFFu));
    msg.push_back(bu);
    msg.push_back(au);
    return msg;
}

std::vector<uint8_t> puente_entrada_arma_tecla(uint16_t scancode, int accion) {
    const uint8_t au = static_cast<uint8_t>(ajustar(accion, 1, 2));
    std::vector<uint8_t> msg;
    msg.reserve(kTamanoCabecera + PUENTE_ENTRADA_TAMANO_CARGA_TECLA);
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_MAGIC & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 8) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 16) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 24) & 0xFFu));
    msg.push_back(PUENTE_ENTRADA_TIPO_TECLA);
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_TAMANO_CARGA_TECLA & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_TAMANO_CARGA_TECLA >> 8) & 0xFFu));
    // Carga: scancode u16, accion u8, relleno u8
    msg.push_back(static_cast<uint8_t>(scancode & 0xFFu));
    msg.push_back(static_cast<uint8_t>((scancode >> 8) & 0xFFu));
    msg.push_back(au);
    msg.push_back(0);
    return msg;
}

std::vector<uint8_t> puente_entrada_arma_rueda(int dx, int dy) {
    const int16_t dx16 = static_cast<int16_t>(ajustar(dx, -32767, 32767));
    const int16_t dy16 = static_cast<int16_t>(ajustar(dy, -32767, 32767));
    std::vector<uint8_t> msg;
    msg.reserve(kTamanoCabecera + PUENTE_ENTRADA_TAMANO_CARGA_RUEDA);
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_MAGIC & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 8) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 16) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 24) & 0xFFu));
    msg.push_back(PUENTE_ENTRADA_TIPO_RUEDA);
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_TAMANO_CARGA_RUEDA & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_TAMANO_CARGA_RUEDA >> 8) & 0xFFu));
    // Carga: dx i16, dy i16
    msg.push_back(static_cast<uint8_t>(static_cast<uint16_t>(dx16) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((static_cast<uint16_t>(dx16) >> 8) & 0xFFu));
    msg.push_back(static_cast<uint8_t>(static_cast<uint16_t>(dy16) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((static_cast<uint16_t>(dy16) >> 8) & 0xFFu));
    return msg;
}

std::vector<uint8_t> puente_entrada_arma_movimiento(int x, int y) {
    const uint16_t xu = static_cast<uint16_t>(ajustar(x, 0, 0xFFFF));
    const uint16_t yu = static_cast<uint16_t>(ajustar(y, 0, 0xFFFF));
    std::vector<uint8_t> msg;
    msg.reserve(kTamanoCabecera + PUENTE_ENTRADA_TAMANO_CARGA_MOV);
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_MAGIC & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 8) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 16) & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_MAGIC >> 24) & 0xFFu));
    msg.push_back(PUENTE_ENTRADA_TIPO_MOVIMIENTO);
    msg.push_back(static_cast<uint8_t>(PUENTE_ENTRADA_TAMANO_CARGA_MOV & 0xFFu));
    msg.push_back(static_cast<uint8_t>((PUENTE_ENTRADA_TAMANO_CARGA_MOV >> 8) & 0xFFu));
    // Carga: x u16, y u16
    msg.push_back(static_cast<uint8_t>(xu & 0xFFu));
    msg.push_back(static_cast<uint8_t>((xu >> 8) & 0xFFu));
    msg.push_back(static_cast<uint8_t>(yu & 0xFFu));
    msg.push_back(static_cast<uint8_t>((yu >> 8) & 0xFFu));
    return msg;
}

bool puente_entrada_iniciar(const char* host, int puerto) {
    if (host == nullptr) host = "127.0.0.1";
    if (puerto <= 0 || puerto > 65535) {
        registrar_error("puerto invalido: %d", puerto);
        return false;
    }
    Estado& e = estado();
    std::lock_guard<std::mutex> lock(e.mutex);
    if (e.corriendo) {
        // Ya arrancado: solo cambiamos el destino.
        e.host = host;
        e.puerto = puerto;
        if (e.fd >= 0) {
            cerrar_socket(e.fd);
            e.fd = -1;
            e.conectado = false;
        }
        e.cv.notify_all();
        return true;
    }
    e.host = host;
    e.puerto = puerto;
    e.conectado = false;
    e.corriendo = true;
    e.hilo = std::thread(hilo_red);
    return true;
}

void puente_entrada_cerrar() {
    Estado& e = estado();
    std::thread hilo;
    {
        std::lock_guard<std::mutex> lock(e.mutex);
        if (!e.corriendo && e.fd < 0 && e.hilo.joinable() == false) {
            e.pendientes.clear();
            e.hay_pendientes = false;
            return;
        }
        e.corriendo = false;
        e.cv.notify_all();
        if (e.fd >= 0) {
            cerrar_socket(e.fd);
            e.fd = -1;
        }
        e.conectado = false;
        hilo = std::move(e.hilo);
    }
    if (hilo.joinable()) hilo.join();
    {
        std::lock_guard<std::mutex> lock(e.mutex);
        e.pendientes.clear();
        e.hay_pendientes = false;
    }
}

bool puente_entrada_conectado() {
    Estado& e = estado();
    std::lock_guard<std::mutex> lock(e.mutex);
    return e.conectado;
}

static bool enviar_mensaje_o_encolar(const std::vector<uint8_t>& msg, const char* descripcion) {
    Estado& e = estado();
    std::lock_guard<std::mutex> lock(e.mutex);
    if (!e.corriendo) {
        registrar_error("el puente de entrada no esta iniciado");
        return false;
    }
    if (e.conectado && e.fd >= 0) {
        // Envio directo: el socket es no bloqueante, asi que NO esperamos.
        ssize_t n = ::send(e.fd, msg.data(), msg.size(), 0);
        if (n >= 0 && static_cast<size_t>(n) == msg.size()) return true;
        // Se cayo (o esta lleno): el resto va a la cola del hilo de red.
        std::vector<uint8_t> resto;
        if (n > 0) {
            resto.assign(msg.begin() + n, msg.end());
        } else {
            resto = msg;
            e.conectado = false;
            registrar_error("no se pudo enviar %s: %s", descripcion, strerror(errno));
        }
        if (e.pendientes.size() < PUENTE_ENTRADA_MAX_PENDIENTES) {
            e.pendientes.push_back(resto);
            e.hay_pendientes = true;
            e.cv.notify_all();
            return true;
        }
        registrar_error("socket caido y cola de pendientes llena");
        return false;
    }
    // Sin conexion: encolamos para cuando vuelva el motor.
    if (e.pendientes.size() >= PUENTE_ENTRADA_MAX_PENDIENTES) {
        registrar_error("no hay conexion y la cola de pendientes esta llena (%u)",
                       static_cast<unsigned>(PUENTE_ENTRADA_MAX_PENDIENTES));
        return false;
    }
    e.pendientes.push_back(msg);
    e.hay_pendientes = true;
    e.cv.notify_all();
    return false;  // no habia conexion: el mensaje queda encolado
}

bool puente_entrada_clic(int x, int y, int boton, int accion) {
    return enviar_mensaje_o_encolar(puente_entrada_arma_clic(x, y, boton, accion), "clic");
}

bool puente_entrada_tecla(uint16_t scancode, int accion) {
    return enviar_mensaje_o_encolar(puente_entrada_arma_tecla(scancode, accion), "tecla");
}

bool puente_entrada_rueda(int dx, int dy) {
    return enviar_mensaje_o_encolar(puente_entrada_arma_rueda(dx, dy), "rueda");
}

bool puente_entrada_movimiento(int x, int y) {
    return enviar_mensaje_o_encolar(puente_entrada_arma_movimiento(x, y), "movimiento");
}

bool puente_entrada_arma_soltar(int x, int y, int boton) {
    Estado& e = estado();
    {
        std::lock_guard<std::mutex> lock(e.mutex);
        e.armado = true;
        e.armado_x = x;
        e.armado_y = y;
        e.armado_boton = boton;
    }
    return puente_entrada_clic(x, y, boton, kAccionAbajo);
}

bool puente_entrada_suelta(int x, int y, int boton) {
    int ux = x;
    int uy = y;
    {
        Estado& e = estado();
        std::lock_guard<std::mutex> lock(e.mutex);
        if (e.armado) {
            ux = e.armado_x;
            uy = e.armado_y;
            boton = e.armado_boton;
            e.armado = false;
        }
    }
    return puente_entrada_clic(ux, uy, boton, kAccionArriba);
}

const char* puente_entrada_ultimo_error() {
    Error& e = error_global();
    std::lock_guard<std::mutex> lock(e.mutex);
    return e.texto.c_str();
}

size_t puente_entrada_pendientes() {
    Estado& e = estado();
    std::lock_guard<std::mutex> lock(e.mutex);
    return e.pendientes.size();
}

const char* puente_entrada_version() { return "XFCE/1"; }

}  // namespace xfv
