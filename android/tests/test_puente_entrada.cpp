// test_puente_entrada.cpp — Banco de escritorio del cliente XFCE.
//
// Compila y corre en el Mac (clang++/g++), sin NDK ni Android: el modulo
// solo usa sockets POSIX.
//
// Que comprueba:
//   1. La codificacion del mensaje XFCE byte a byte (contra el mismo formato
//      que usa scripts/enviar_clic.py).
//   2. Que el modulo se conecta a un servidor TCP local (que hace de
//      inyector), que el servidor RECIBE el mensaje y que coincide.
//   3. Que el modulo reconecta solo cuando el servidor (motor) se reinicia,
//      y que los clics encolados durante la caida se entregan despues.

#include "../src/puente_entrada.h"

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netinet/in.h>
#include <poll.h>
#include <string.h>
#include <algorithm>
#include <mutex>
#include <sys/socket.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cstdio>
#include <string>
#include <thread>
#include <vector>

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

std::string a_hex(const std::vector<uint8_t>& datos) {
    std::string s;
    char buf[8];
    for (size_t i = 0; i < datos.size(); ++i) {
        snprintf(buf, sizeof(buf), "%02X ", datos[i]);
        s += buf;
    }
    return s;
}

std::vector<uint8_t> esperado(int x, int y, int boton, int accion) {
    // Magic "XFCE" (0x45434658) little-endian, tipo 1, tamano 6, carga
    // <HHBB>. Exactamente lo que arma scripts/enviar_clic.py.
    std::vector<uint8_t> m;
    m.push_back(0x58);
    m.push_back(0x46);
    m.push_back(0x43);
    m.push_back(0x45);
    m.push_back(0x01);
    m.push_back(0x06);
    m.push_back(0x00);
    m.push_back(static_cast<uint8_t>(x & 0xFF));
    m.push_back(static_cast<uint8_t>((x >> 8) & 0xFF));
    m.push_back(static_cast<uint8_t>(y & 0xFF));
    m.push_back(static_cast<uint8_t>((y >> 8) & 0xFF));
    m.push_back(static_cast<uint8_t>(boton));
    m.push_back(static_cast<uint8_t>(accion));
    return m;
}

// ---------------------------------------------------------------- servidor
//
// Hace de inyector: escucha en un puerto, acepta la conexion del modulo y
// guarda los bytes que llegan, troceados en mensajes de 13 bytes (el tamano
// del mensaje XFCE de clic). Todo es no bloqueante, asi que el hilo puede
// seguir aceptando conexiones mientras espera datos.

class Servidor {
public:
    Servidor() : listen_fd_(-1), puerto_(0), parar_(false) {}

    ~Servidor() { parar(); }

    bool arrancar(int puerto = 0) {
        parar_.store(false);
        listen_fd_ = ::socket(AF_INET, SOCK_STREAM, 0);
        if (listen_fd_ < 0) return false;
        int uno = 1;
        setsockopt(listen_fd_, SOL_SOCKET, SO_REUSEADDR, &uno, sizeof(uno));
        int flags = fcntl(listen_fd_, F_GETFL, 0);
        fcntl(listen_fd_, F_SETFL, flags | O_NONBLOCK);
        struct sockaddr_in addr;
        memset(&addr, 0, sizeof(addr));
        addr.sin_family = AF_INET;
        addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
        addr.sin_port = htons(static_cast<uint16_t>(puerto));
        if (::bind(listen_fd_, reinterpret_cast<struct sockaddr*>(&addr), sizeof(addr)) != 0) {
            ::close(listen_fd_);
            listen_fd_ = -1;
            return false;
        }
        if (::listen(listen_fd_, 4) != 0) {
            ::close(listen_fd_);
            listen_fd_ = -1;
            return false;
        }
        socklen_t len = sizeof(addr);
        if (::getsockname(listen_fd_, reinterpret_cast<struct sockaddr*>(&addr), &len) != 0) return false;
        puerto_ = ntohs(addr.sin_port);
        hilo_ = std::thread(&Servidor::bucle, this);
        return true;
    }

    void parar() {
        parar_.store(true);
        std::vector<int> clientes;
        {
            std::lock_guard<std::mutex> lock(mutex_);
            clientes = clientes_;
            clientes_.clear();
            recibidos_.clear();
        }
        for (size_t i = 0; i < clientes.size(); ++i) ::close(clientes[i]);
        if (listen_fd_ >= 0) {
            ::close(listen_fd_);
            listen_fd_ = -1;
        }
        if (hilo_.joinable()) hilo_.join();
    }

    // El motor se reinicia: se corta todo y se vuelve a escuchar.
    bool reiniciar(int puerto) {
        parar();
        return arrancar(puerto);
    }

    int puerto() const { return puerto_; }

    // Espera a que lleguen `n` mensajes completos (13 bytes cada uno).
    bool esperar_mensajes(size_t n, int timeout_ms, std::vector<std::vector<uint8_t> >& fuera) {
        auto limite = std::chrono::steady_clock::now() + std::chrono::milliseconds(timeout_ms);
        while (std::chrono::steady_clock::now() < limite) {
            {
                std::lock_guard<std::mutex> lock(mutex_);
                if (recibidos_.size() >= n) {
                    fuera.assign(recibidos_.begin(), recibidos_.begin() + static_cast<long>(n));
                    return true;
                }
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
        }
        return false;
    }

private:
    static const size_t kBytesPorMensaje = 13;

    void bucle() {
        std::vector<unsigned char> pendientes;
        while (!parar_.load()) {
            std::vector<struct pollfd> pfds;
            if (listen_fd_ >= 0) {
                struct pollfd p;
                p.fd = listen_fd_;
                p.events = POLLIN;
                p.revents = 0;
                pfds.push_back(p);
            }
            std::vector<int> clientes;
            {
                std::lock_guard<std::mutex> lock(mutex_);
                clientes = clientes_;
            }
            for (size_t i = 0; i < clientes.size(); ++i) {
                struct pollfd p;
                p.fd = clientes[i];
                p.events = POLLIN;
                p.revents = 0;
                pfds.push_back(p);
            }
            if (pfds.empty()) {
                std::this_thread::sleep_for(std::chrono::milliseconds(10));
                continue;
            }
            int pr = ::poll(pfds.data(), static_cast<nfds_t>(pfds.size()), 50);
            if (pr <= 0) continue;

            size_t indice = 0;
            if (listen_fd_ >= 0) {
                if (pfds[indice].revents & POLLIN) {
                    for (;;) {
                        int cliente = ::accept(listen_fd_, nullptr, nullptr);
                        if (cliente < 0) break;
                        int flags = fcntl(cliente, F_GETFL, 0);
                        fcntl(cliente, F_SETFL, flags | O_NONBLOCK);
                        std::lock_guard<std::mutex> lock(mutex_);
                        clientes_.push_back(cliente);
                    }
                }
                ++indice;
            }
            for (size_t i = indice; i < pfds.size(); ++i) {
                if (!(pfds[i].revents & (POLLIN | POLLHUP | POLLERR))) continue;
                int fd = pfds[i].fd;
                unsigned char chunk[256];
                for (;;) {
                    ssize_t n = ::recv(fd, chunk, sizeof(chunk), 0);
                    if (n > 0) {
                        pendientes.insert(pendientes.end(), chunk, chunk + n);
                        continue;
                    }
                    if (n == 0) {
                        // El otro lado cerro: se marca para cerrar.
                        std::lock_guard<std::mutex> lock(mutex_);
                        for (size_t k = 0; k < clientes_.size(); ++k) {
                            if (clientes_[k] == fd) {
                                ::close(fd);
                                clientes_.erase(clientes_.begin() + static_cast<long>(k));
                                break;
                            }
                        }
                        break;
                    }
                    if (errno == EAGAIN || errno == EWOULDBLOCK) break;
                    break;
                }
                while (pendientes.size() >= kBytesPorMensaje) {
                    std::vector<uint8_t> m(pendientes.begin(), pendientes.begin() + static_cast<long>(kBytesPorMensaje));
                    pendientes.erase(pendientes.begin(), pendientes.begin() + static_cast<long>(kBytesPorMensaje));
                    std::lock_guard<std::mutex> lock(mutex_);
                    recibidos_.push_back(m);
                }
            }
        }
    }

    int listen_fd_;
    int puerto_;
    std::atomic<bool> parar_;
    std::thread hilo_;
    std::mutex mutex_;
    std::vector<int> clientes_;
    std::vector<std::vector<uint8_t> > recibidos_;
};

// -------------------------------------------------------------- pruebas

void prueba_armado() {
    std::printf("1) Codificacion byte a byte del mensaje XFCE\n");
    std::vector<uint8_t> m = xfv::puente_entrada_arma_clic(320, 180, 1, 3);
    std::vector<uint8_t> e = esperado(320, 180, 1, 3);
    comprobar(m.size() == 13, "el mensaje mide 13 bytes (7 de cabecera + 6 de carga)");
    comprobar(m == e, "los bytes coinciden con scripts/enviar_clic.py: " + a_hex(m));
    comprobar(m[0] == 0x58 && m[1] == 0x46 && m[2] == 0x43 && m[3] == 0x45,
              "magic = 'XFCE' en little-endian (0x58 0x46 0x43 0x45)");
    comprobar(m[4] == 1, "tipo = 1 (clic)");
    comprobar(m[5] == 6 && m[6] == 0, "tamano de carga = 6 (u16 little-endian)");
    comprobar(m[7] == 0x40 && m[8] == 0x01, "x = 320 (u16 little-endian)");
    comprobar(m[9] == 0xB4 && m[10] == 0x00, "y = 180 (u16 little-endian)");
    comprobar(m[11] == 1, "boton = 1 (izquierdo)");
    comprobar(m[12] == 3, "accion = 3 (clic completo)");

    // Armar-soltar: misma posicion, accion 1 y luego 2.
    std::vector<uint8_t> abajo = xfv::puente_entrada_arma_clic(100, 200, 1, 1);
    std::vector<uint8_t> arriba = xfv::puente_entrada_arma_clic(100, 200, 1, 2);
    comprobar(abajo == esperado(100, 200, 1, 1), "accion=1 (abajo) bien codificada");
    comprobar(arriba == esperado(100, 200, 1, 2), "accion=2 (arriba) bien codificada");
    comprobar(abajo[7] == arriba[7] && abajo[9] == arriba[9],
              "armar y soltar usan la MISMA posicion");

    // Valores fuera de rango: se recortan, no revientan.
    std::vector<uint8_t> r = xfv::puente_entrada_arma_clic(-5, 70000, 9, 9);
    comprobar(r == esperado(0, 65535, 3, 3), "los valores fuera de rango se recortan");
}

void prueba_servidor_y_reconexion() {
    std::printf("2) Envio real a un servidor TCP local (el inyector)\n");
    Servidor servidor;
    if (!servidor.arrancar(0)) {
        comprobar(false, "el servidor local arranco");
        return;
    }
    const int puerto = servidor.puerto();
    comprobar(puerto > 0, "el servidor local escucha en el puerto " + std::to_string(puerto));

    comprobar(xfv::puente_entrada_iniciar("127.0.0.1", puerto), "puente_entrada_iniciar devuelve true");

    // Espera a la conexion (la hace el hilo de red, en segundo plano).
    bool conectado = false;
    for (int i = 0; i < 200 && !conectado; ++i) {
        conectado = xfv::puente_entrada_conectado();
        if (!conectado) std::this_thread::sleep_for(std::chrono::milliseconds(25));
    }
    comprobar(conectado, "el modulo quedo conectado al servidor local");

    bool enviado = xfv::puente_entrada_clic(320, 180, 1, 3);
    comprobar(enviado, "puente_entrada_clic devuelve true con conexion");

    std::vector<std::vector<uint8_t> > mensajes;
    bool llego = servidor.esperar_mensajes(1, 3000, mensajes);
    comprobar(llego, "el servidor recibio un mensaje");
    if (llego) {
        comprobar(mensajes[0].size() == 13, "el mensaje recibido mide 13 bytes");
        comprobar(mensajes[0] == esperado(320, 180, 1, 3),
                  "el mensaje recibido es exactamente el esperado: " + a_hex(mensajes[0]));
    }

    std::printf("3) Reconexion cuando el motor se reinicia\n");
    // El motor se cae y vuelve (mismo puerto).
    bool rearrancado = servidor.reiniciar(puerto);
    comprobar(rearrancado, "el servidor local vuelve a escuchar en el mismo puerto");

    // Se espera a que el modulo note la caida.
    bool caido = false;
    for (int i = 0; i < 200 && !caido; ++i) {
        caido = !xfv::puente_entrada_conectado();
        if (!caido) std::this_thread::sleep_for(std::chrono::milliseconds(25));
    }
    comprobar(caido, "el modulo noto que el motor se fue");

    // Clic con el motor caido: se encola (no hay conexion).
    bool ok = xfv::puente_entrada_clic(111, 222, 1, 1);
    comprobar(!ok, "sin conexion puente_entrada_clic devuelve false");
    comprobar(xfv::puente_entrada_pendientes() >= 1, "el clic quedo en la cola de pendientes");

    // El motor vuelve: el modulo debe reconectar solo y entregar el pendiente.
    bool reconectado = false;
    for (int i = 0; i < 300 && !reconectado; ++i) {
        reconectado = xfv::puente_entrada_conectado();
        if (!reconectado) std::this_thread::sleep_for(std::chrono::milliseconds(25));
    }
    comprobar(reconectado, "el modulo reconecto solo tras volver el motor");

    std::vector<std::vector<uint8_t> > pendientes;
    bool llego_pendiente = servidor.esperar_mensajes(1, 5000, pendientes);
    comprobar(llego_pendiente, "el clic encolado se entrego al motor");
    if (llego_pendiente) {
        comprobar(pendientes[0] == esperado(111, 222, 1, 1),
                  "el pendiente entregado es el que se encolo: " + a_hex(pendientes[0]));
    }

    // Y sigue funcionando despues de reconectar.
    bool ok2 = xfv::puente_entrada_clic(640, 360, 3, 3);
    comprobar(ok2, "clics normales tras reconectar");
    std::vector<std::vector<uint8_t> > post;
    bool llego_post = servidor.esperar_mensajes(2, 4000, post);
    comprobar(llego_post, "el servidor recibio el clic posterior a la reconexion");
    if (llego_post && post.size() >= 2) {
        comprobar(post[1] == esperado(640, 360, 3, 3),
                  "el clic posterior a la reconexion llega bien: " + a_hex(post[1]));
    }

    xfv::puente_entrada_cerrar();
    comprobar(!xfv::puente_entrada_conectado(), "tras cerrar, el modulo no esta conectado");
}

}  // namespace

int main() {
    std::printf("=== Banco del puente de entrada XFCE ===\n");
    prueba_armado();
    prueba_servidor_y_reconexion();
    if (fallos == 0) {
        std::printf("=== TODO CORRECTO (%d fallos) ===\n", fallos);
        return 0;
    }
    std::printf("=== %d FALLOS ===\n", fallos);
    return 1;
}
