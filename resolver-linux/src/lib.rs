//! resolver-linux — Resolutor de nombres para el motor de Factorio en Android.
//!
//! El motor de Factorio es un binario de **glibc** que corre dentro de
//! Android (que usa **bionic**).  glibc resuelve los nombres leyendo
//! `/etc/resolv.conf`, y Android no tiene ese fichero: la resolucion la hace
//! netd a traves de binder.  Asi que dentro del motor no se resuelve NADA:
//!
//!   Error HttpSharedState.cpp:177: CURL failed: code:6, 6;
//!       Could not resolve host: auth.factorio.com
//!
//! Eso tumba el servidor multijugador, que necesita pedirle a
//! auth.factorio.com el "server padlock" (el bloqueo con el que se
//! emparejan cliente y servidor).  Sin root no se puede crear
//! /etc/resolv.conf, y Termux no deja meter el motor en su entorno.
//!
//! Este shim se carga con LD_PRELOAD y sustituye getaddrinfo: si la
//! resolucion normal falla, resuelve el nombre con un mini-resolver DNS
//! propio (consulta UDP directa al servidor de nombres) y devuelve la
//! direccion.  Si el nombre ya lo resuelve la glibc, no se toca nada.
//!
//! No usa dependencias: solo std (UDP por socket nativo) y libc.

mod gnu_unwind;

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_void, CStr};
use std::net::UdpSocket;
use std::sync::Mutex;
use std::time::Duration;

// ---------------------------------------------------------------------------
// Tipos de libc
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: u32,
    pub sin_zero: [u8; 8],
}

#[repr(C)]
pub struct SockAddrIn6 {
    pub sin6_family: u16,
    pub sin6_port: u16,
    pub sin6_flowinfo: u32,
    pub sin6_addr: [u8; 16],
    pub sin6_scope_id: u32,
}

#[repr(C)]
pub struct AddrInfo {
    pub ai_flags: c_int,
    pub ai_family: c_int,
    pub ai_socktype: c_int,
    pub ai_protocol: c_int,
    pub ai_addrlen: u32,
    pub ai_addr: *mut c_void,
    pub ai_canonname: *mut c_char,
    pub ai_next: *mut AddrInfo,
}

pub const AF_INET: c_int = 2;
pub const AF_INET6: c_int = 10;
pub const SOCK_STREAM: c_int = 1;
pub const SOCK_DGRAM: c_int = 2;
pub const IPPROTO_TCP: c_int = 6;
pub const IPPROTO_UDP: c_int = 17;
pub const AI_CANONNAME: c_int = 0x0004;
pub const AI_PASSIVE: c_int = 0x0001;
pub const EAI_FAIL: c_int = -6;
pub const EAI_FAMILY: c_int = -6;

#[repr(C)]
pub struct HostEnt {
    pub h_name: *mut c_char,
    pub h_aliases: *mut *mut c_char,
    pub h_addrtype: c_int,
    pub h_length: c_int,
    pub h_addr_list: *mut *mut c_void,
}

extern "C" {
    fn free(ptr: *mut c_void);
    fn malloc(size: usize) -> *mut c_void;
    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize);
    fn strdup(s: *const c_char) -> *mut c_char;
    #[link_name = "open"]
    fn open_real(path: *const c_char, flags: c_int, ...) -> c_int;
    #[link_name = "open64"]
    fn open64_real(path: *const c_char, flags: c_int, ...) -> c_int;
    #[link_name = "openat"]
    fn openat_real(dirfd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    #[link_name = "openat64"]
    fn openat64_real(dirfd: c_int, path: *const c_char, flags: c_int, ...) -> c_int;
    #[link_name = "fopen"]
    fn fopen_real(path: *const c_char, mode: *const c_char) -> *mut c_void;
    #[link_name = "fopen64"]
    fn fopen64_real(path: *const c_char, mode: *const c_char) -> *mut c_void;
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    fn close(fd: c_int) -> c_int;
}

// ---------------------------------------------------------------------------
// Registro de las conexiones salientes
// ---------------------------------------------------------------------------
//
// Sirve para diagnostico: deja constancia de a que direccion se conecta el
// motor (util cuando el servidor se queda esperando al auth server).

#[repr(C)]
pub struct SockAddr {
    pub sa_family: u16,
    pub sa_data: [u8; 14],
}

#[no_mangle]
pub unsafe extern "C" fn connect(
    sockfd: c_int,
    addr: *const SockAddr,
    addrlen: u32,
) -> c_int {
    if !addr.is_null() && addrlen >= 8 {
        let familia = (*addr).sa_family;
        let texto = if familia == AF_INET as u16 && addrlen >= 16 {
            let sin = addr as *const SockAddrIn;
            let ip = u32::from_be((*sin).sin_addr);
            let oct = ip.to_be_bytes();
            format!(
                "{}.{}.{}.{}:{}",
                oct[0], oct[1], oct[2], oct[3],
                u16::from_be((*sin).sin_port)
            )
        } else if familia == AF_INET6 as u16 {
            let datos = (*addr).sa_data;
            let oct = &datos[2..6];
            format!("[ipv6:{:02x}{:02x}{:02x}{:02x}]", oct[0], oct[1], oct[2], oct[3])
        } else {
            format!("familia {}", familia)
        };
        registrar(&format!("connect: {}", texto));
    }
    let f: Option<unsafe extern "C" fn(c_int, *const SockAddr, u32) -> c_int> = {
        let ptr = dlsym(RTLD_NEXT, b"connect\0".as_ptr() as *const c_char);
        if ptr.is_null() {
            None
        } else {
            Some(std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(c_int, *const SockAddr, u32) -> c_int,
            >(ptr))
        }
    };
    match f {
        Some(f) => f(sockfd, addr, addrlen),
        None => -1,
    }
}

// ---------------------------------------------------------------------------
// Proxy CONNECT local
// ---------------------------------------------------------------------------
//
// El motor lleva curl ENLAZADO ESTATICAMENTE, con su propia copia de la
// resolucion de nombres (la de libc.a).  Esa copia busca /etc/resolv.conf,
// no lo encuentra y usa 127.0.0.1 por defecto, donde no hay nada escuchando:
// la peticion al auth server se queda esperando hasta el timeout.
//
// curl SI lee las variables de entorno del proxy (getenv es dinamico), asi que
// la solucion es un proxy CONNECT local: el motor le pide el padlock a
// 127.0.0.1:<puerto>, el proxy resuelve el nombre (con el mini-resolver de
// arriba, que si funciona) y hace de tunel con el destino real.  El padlock
// que devuelve es el de verdad, asi que el cliente lo valida sin problema.
//
// El proxy se arranca solo al cargar la libreria.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

const PROXY_PUERTO_POR_DEFECTO: u16 = 8899;

fn proxy_puerto() -> u16 {
    match std::env::var("XFACTORVR_PROXY_PUERTO") {
        Ok(p) => p.parse().unwrap_or(PROXY_PUERTO_POR_DEFECTO),
        Err(_) => PROXY_PUERTO_POR_DEFECTO,
    }
}

/// Lee la primera linea de la peticion (hasta CRLF).
fn leer_linea(stream: &mut TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return None,
            Ok(_) => {
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n") {
                    break;
                }
                if buf.len() > 8192 {
                    return None;
                }
            }
        }
    }
    String::from_utf8(buf).ok()
}

/// Consume el resto de la cabecera HTTP (hasta la linea vacia).
///
/// Sin esto, las cabeceras que quedan despues del CONNECT se cuelan en el
/// tunel y el servidor TLS recibe basura antes del ClientHello:
/// "error:1408F10B:SSL routines:ssl3_get_record:wrong version number".
fn descartar_cabecera(stream: &mut TcpStream) {
    let mut buf: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n\r\n") {
                    return;
                }
                if buf.len() > 16 * 1024 {
                    return;
                }
            }
        }
    }
}

fn manejar_conexion(mut cliente: TcpStream) {
    let peticion = match leer_linea(&mut cliente) {
        Some(p) => p,
        None => return,
    };
    registrar(&format!("proxy: {}", peticion.trim_end()));

    let partes: Vec<&str> = peticion.split_whitespace().collect();
    if partes.is_empty() {
        return;
    }

    if partes[0].eq_ignore_ascii_case("CONNECT") && partes.len() >= 2 {
        // CONNECT host:puerto
        let destino = partes[1];
        let (host, puerto) = match destino.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().unwrap_or(443)),
            None => (destino.to_string(), 443),
        };
        let ip = match resolver(&host) {
            Some(ip) => ip,
            None => {
                let _ = cliente.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                return;
            }
        };
        let oct = ip.to_be_bytes();
        let destino_ip = format!("{}.{}.{}.{}:{}", oct[0], oct[1], oct[2], oct[3], puerto);
        // La cabecera que queda tras el CONNECT no se reenvia: se consume.
        descartar_cabecera(&mut cliente);
        let mut remoto = match TcpStream::connect(&destino_ip) {
            Ok(s) => s,
            Err(e) => {
                registrar(&format!("proxy: no se pudo conectar a {} ({})", destino_ip, e));
                let _ = cliente.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                return;
            }
        };
        if cliente.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").is_err() {
            return;
        }
        // Tunel en las dos direcciones.
        let mut c2 = match cliente.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut r2 = match remoto.try_clone() {
            Ok(s) => s,
            Err(_) => return,
        };
        let h1 = std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match cliente.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if remoto.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let h2 = std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match r2.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if c2.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        let _ = h1.join();
        let _ = h2.join();
    } else {
        // Peticion HTTP absoluta (GET/POST http://host/path): se connects
        // directo y se le reenvia la peticion tal cual.
        if partes.len() >= 2 && partes[1].starts_with("http://") {
            let resto = partes[1].trim_start_matches("http://");
            let (host, camino) = match resto.split_once('/') {
                Some((h, c)) => (h.to_string(), format!("/{}", c)),
                None => (resto.to_string(), "/".to_string()),
            };
            let ip = match resolver(&host) {
                Some(ip) => ip,
                None => {
                    let _ = cliente.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                    return;
                }
            };
            let oct = ip.to_be_bytes();
            let destino_ip = format!("{}.{}.{}.{}", oct[0], oct[1], oct[2], oct[3]);
            let mut remoto = match TcpStream::connect(&destino_ip) {
                Ok(s) => s,
                Err(_) => {
                    let _ = cliente.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                    return;
                }
            };
            let peticion = format!("{} {}\r\n", partes[0], camino);
            if remoto.write_all(peticion.as_bytes()).is_err() {
                return;
            }
            let mut buf = [0u8; 16 * 1024];
            loop {
                match cliente.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if remoto.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
        } else {
            let _ = cliente.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        }
    }
}

/// Arranca el proxy local en un hilo aparte.
fn arrancar_proxy() {
    let puerto = proxy_puerto();
    let listener = match TcpListener::bind(("127.0.0.1", puerto)) {
        Ok(l) => l,
        Err(e) => {
            registrar(&format!("proxy: no se pudo abrir el puerto {} ({})", puerto, e));
            return;
        }
    };
    registrar(&format!("proxy: escuchando en 127.0.0.1:{}", puerto));
    std::thread::spawn(move || {
        for conexion in listener.incoming() {
            if let Ok(c) = conexion {
                std::thread::spawn(move || manejar_conexion(c));
            }
        }
    });
}

/// Se ejecuta al cargar la libreria (seccion .init_array).
#[used]
#[link_section = ".init_array"]
static INICIAR: extern "C" fn() = {
    extern "C" fn inicio() {
        arrancar_proxy();
    }
    inicio
};

// ---------------------------------------------------------------------------
// Redireccion de /etc/resolv.conf
// ---------------------------------------------------------------------------
//
// El motor de Factorio lleva curl ENLAZADO ESTATICAMENTE, con su propia
// copia de getaddrinfo y su propio NSS.  Esa copia no pasa por los simbolos
// dinamicos, asi que interceptar getaddrinfo no le afecta: por eso el shim
// tambien intercepta la apertura de /etc/resolv.conf (que en Android no
// existe) y le da el fichero de configuracion del visor.
//
// El fichero de destino se busca en:
//   $XFACTORVR_RESOLV_CONF
//   /data/local/tmp/glibc/resolv.conf
//
// y si no existe se genera uno con los servidores de nombres publicos.

const RESOLV_REAL: &str = "/etc/resolv.conf";

fn ruta_resolv_conf() -> Option<std::ffi::CString> {
    if let Ok(p) = std::env::var("XFACTORVR_RESOLV_CONF") {
        if !p.is_empty() {
            return std::ffi::CString::new(p).ok();
        }
    }
    let por_defecto = "/data/local/tmp/glibc/resolv.conf";
    if std::path::Path::new(por_defecto).exists() {
        return std::ffi::CString::new(por_defecto).ok();
    }
    None
}

/// Si `path` es /etc/resolv.conf, devuelve la ruta que hay que abrir en su
/// lugar.  En otro caso, None.
fn redirigir(path: *const c_char) -> Option<std::ffi::CString> {
    if path.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(path) }.to_bytes();
    if s != RESOLV_REAL.as_bytes() {
        return None;
    }
    ruta_resolv_conf()
}

// Los ganchos se escriben a mano (Rust no admite varargs en FFI de forma
// segura, pero en ARM64 pasar un argumento de mas a una funcion variadica es
// inocuo: open/openat siempre reciben el modo en x2).

unsafe fn dlsym_next_open(
    _handle: *mut c_void,
    sym: &[u8],
) -> Option<unsafe extern "C" fn(*const c_char, c_int) -> c_int> {
    let ptr = dlsym(RTLD_NEXT, sym.as_ptr() as *const c_char);
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute::<
            *mut c_void,
            unsafe extern "C" fn(*const c_char, c_int) -> c_int,
        >(ptr))
    }
}

#[no_mangle]
pub unsafe extern "C" fn open(path: *const c_char, flags: c_int, mode: c_int) -> c_int {
    let f = match dlsym_next_open(RTLD_NEXT, b"open\0") {
        Some(f) => f,
        None => return -1,
    };
    match redirigir(path) {
        Some(d) => f(d.as_ptr(), flags),
        None => f(path, flags),
    }
}

#[no_mangle]
pub unsafe extern "C" fn open64(path: *const c_char, flags: c_int, mode: c_int) -> c_int {
    open(path, flags, mode)
}

#[no_mangle]
pub unsafe extern "C" fn openat(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: c_int,
) -> c_int {
    match redirigir(path) {
        Some(d) => {
            let f: Option<
                unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int,
            > = {
                let ptr = dlsym(RTLD_NEXT, b"openat\0".as_ptr() as *const c_char);
                if ptr.is_null() {
                    None
                } else {
                    Some(std::mem::transmute::<
                        *mut c_void,
                        unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int,
                    >(ptr))
                }
            };
            match f {
                Some(f) => f(dirfd, d.as_ptr(), flags),
                None => -1,
            }
        }
        None => {
            let f: Option<
                unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int,
            > = {
                let ptr = dlsym(RTLD_NEXT, b"openat\0".as_ptr() as *const c_char);
                if ptr.is_null() {
                    None
                } else {
                    Some(std::mem::transmute::<
                        *mut c_void,
                        unsafe extern "C" fn(c_int, *const c_char, c_int) -> c_int,
                    >(ptr))
                }
            };
            match f {
                Some(f) => f(dirfd, path, flags),
                None => -1,
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn openat64(
    dirfd: c_int,
    path: *const c_char,
    flags: c_int,
    mode: c_int,
) -> c_int {
    openat(dirfd, path, flags, mode)
}

#[no_mangle]
pub unsafe extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut c_void {
    let destino = match redirigir(path) {
        Some(d) => d,
        None => {
            let f: Option<unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void> = {
                let ptr = dlsym(RTLD_NEXT, b"fopen\0".as_ptr() as *const c_char);
                if ptr.is_null() {
                    None
                } else {
                    Some(std::mem::transmute::<
                        *mut c_void,
                        unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void,
                    >(ptr))
                }
            };
            return match f {
                Some(f) => f(path, mode),
                None => std::ptr::null_mut(),
            };
        }
    };
    let f: Option<unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void> = {
        let ptr = dlsym(RTLD_NEXT, b"fopen\0".as_ptr() as *const c_char);
        if ptr.is_null() {
            None
        } else {
            Some(std::mem::transmute::<
                *mut c_void,
                unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void,
            >(ptr))
        }
    };
    match f {
        Some(f) => f(destino.as_ptr(), mode),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn fopen64(path: *const c_char, mode: *const c_char) -> *mut c_void {
    fopen(path, mode)
}

// --- registro de diagnostico -----------------------------------------------
//
// Con XFACTORVR_RESOLVER_LOG=<ruta> el shim deja constancia de lo que hace.
// Se escribe con open/write a pelo (sin std::fs) para no meter malloc ni
// bloqueos dentro de getaddrinfo, que puede llamar el motor desde varios
// hilos a la vez.

const O_WRONLY: c_int = 1;
const O_CREAT: c_int = 64;
const O_APPEND: c_int = 1024;

fn registrar(mensaje: &str) {
    let ruta = match std::env::var("XFACTORVR_RESOLVER_LOG") {
        Ok(r) if !r.is_empty() => r,
        _ => return,
    };
    unsafe {
        let mut buf = Vec::with_capacity(mensaje.len() + 1);
        buf.extend_from_slice(mensaje.as_bytes());
        buf.push(b'\n');
        let c = match std::ffi::CString::new(ruta) {
            Ok(c) => c,
            Err(_) => return,
        };
        let fd = open(c.as_ptr(), O_WRONLY | O_CREAT | O_APPEND, 0o644);
        if fd < 0 {
            return;
        }
        write(fd, buf.as_ptr() as *const c_void, buf.len());
        close(fd);
    }
}

// ---------------------------------------------------------------------------
// Mini-resolver DNS
// ---------------------------------------------------------------------------

/// Servidores de nombres que se consultan si la glibc no puede resolver.
///
/// El primero es el del router (responde rapido y sin salir de la red); los
/// publicos son el plan B.  El orden importa: cada consulta que no responde
/// cuesta un timeout entero.
const SERVIDORES: [&str; 3] = ["172.20.0.1", "8.8.8.8", "1.1.1.1"];

static CACHE: Mutex<Option<HashMap<String, (u32, u64)>>> = Mutex::new(None);

fn cache() -> &'static Mutex<Option<HashMap<String, (u32, u64)>>> {
    &CACHE
}

/// Direccion IPv4 en orden de red.
fn ip_a_bytes(ip: [u8; 4]) -> u32 {
    u32::from_be_bytes(ip)
}

/// Construye la consulta DNS A de un nombre y la envia por UDP.
///
/// Devuelve la primera direccion IPv4 encontrada (en orden de red) o None.
fn resolver_udp(nombre: &str) -> Option<u32> {
    // Consulta: cabecera + nombre + QTYPE=A + QCLASS=IN.
    let mut consulta: Vec<u8> = Vec::with_capacity(64);
    consulta.extend_from_slice(&0x1234u16.to_be_bytes()); // id
    consulta.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: recursion
    consulta.extend_from_slice(&1u16.to_be_bytes()); // qdcount
    consulta.extend_from_slice(&0u16.to_be_bytes()); // ancount
    consulta.extend_from_slice(&0u16.to_be_bytes()); // nscount
    consulta.extend_from_slice(&0u16.to_be_bytes()); // arcount
    for etiqueta in nombre.trim_end_matches('.').split('.') {
        if etiqueta.is_empty() || etiqueta.len() > 63 {
            return None;
        }
        consulta.push(etiqueta.len() as u8);
        consulta.extend_from_slice(etiqueta.as_bytes());
    }
    consulta.push(0); // fin del nombre
    consulta.extend_from_slice(&1u16.to_be_bytes()); // QTYPE = A
    consulta.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN

    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(Duration::from_millis(1500))).ok()?;

    for servidor in SERVIDORES.iter() {
        let destino = format!("{}:53", servidor);
        if socket.send_to(&consulta, &destino).is_err() {
            registrar(&format!("dns: no se pudo enviar a {}", destino));
            continue;
        }
        let mut respuesta = [0u8; 512];
        let n = match socket.recv(&mut respuesta) {
            Ok(n) => n,
            Err(e) => {
                registrar(&format!("dns: sin respuesta de {} ({})", destino, e));
                continue;
            }
        };
        match parse_a(&respuesta[..n]) {
            Some(ip) => return Some(ip),
            None => registrar(&format!(
                "dns: respuesta de {} sin registro A ({} bytes)",
                destino, n
            )),
        }
    }
    None
}

/// Extrae la primera direccion A de una respuesta DNS.
fn parse_a(buf: &[u8]) -> Option<u32> {
    if buf.len() < 12 {
        return None;
    }
    let qd = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let an = u16::from_be_bytes([buf[6], buf[7]]) as usize;
    if an == 0 {
        return None;
    }
    let mut pos = 12usize;
    // Saltar la zona de preguntas: nombre + QTYPE (2) + QCLASS (2).
    for _ in 0..qd {
        pos = saltar_nombre(buf, pos)?;
        if pos + 4 > buf.len() {
            return None;
        }
        pos += 4;
    }
    // Recorrer las respuestas.
    for _ in 0..an {
        pos = saltar_nombre(buf, pos)?;
        if pos + 10 > buf.len() {
            return None;
        }
        let tipo = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let rdlen = u16::from_be_bytes([buf[pos + 8], buf[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > buf.len() {
            return None;
        }
        if tipo == 1 && rdlen == 4 {
            return Some(ip_a_bytes([
                buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3],
            ]));
        }
        pos += rdlen;
    }
    None
}

/// Devuelve la posicion tras un nombre (con posibles compresion).
fn saltar_nombre(buf: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        if pos >= buf.len() {
            return None;
        }
        let len = buf[pos] as usize;
        if len == 0 {
            return Some(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            // Puntero: se salta y se acaba el nombre.
            return Some(pos + 2);
        }
        pos += 1 + len;
    }
}

/// Resuelve un nombre a IPv4, con cache.
fn resolver(nombre: &str) -> Option<u32> {
    let mut guard = cache().lock().ok()?;
    let mapa = guard.get_or_insert_with(HashMap::new);
    if let Some((ip, expira)) = mapa.get(nombre) {
        if *expira > ahora() {
            return Some(*ip);
        }
    }
    let ip = resolver_udp(nombre)?;
    // Cache de 5 minutos: suficiente para el padlock y para las peticiones
    // de la API sin invalidar la cache en cada llamada.
    mapa.insert(nombre.to_string(), (ip, ahora() + 300));
    Some(ip)
}

fn ahora() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Intercepcion de getaddrinfo
// ---------------------------------------------------------------------------

/// Construye la lista de AddrInfo para una IPv4 y la devuelve.
///
/// Si `hints` pide AF_INET6 solo, no hay nada que hacer: el motor no lo pide
/// (curl usa AF_UNSPEC).
unsafe fn construir(nombre: *const c_char, ip: u32, puerto: u16, hints: *const AddrInfo)
    -> *mut AddrInfo
{
    let (familia, socktype, protocolo, flags) = if hints.is_null() {
        (AF_INET, 0, 0, 0)
    } else {
        let h = &*hints;
        // AF_UNSPEC (0) significa "cualquiera": el resultado debe bringar una
        // familia concreta, si no curl descarta la lista y dice que no pudo
        // resolver.
        let fam = if h.ai_family == 0 || h.ai_family == AF_INET { AF_INET } else { h.ai_family };
        if fam != AF_INET {
            return std::ptr::null_mut();
        }
        (fam, h.ai_socktype, h.ai_protocol, h.ai_flags)
    };

    let nodo = malloc(std::mem::size_of::<AddrInfo>()) as *mut AddrInfo;
    if nodo.is_null() {
        return std::ptr::null_mut();
    }
    let sock = malloc(std::mem::size_of::<SockAddrIn>()) as *mut SockAddrIn;
    if sock.is_null() {
        free(nodo as *mut c_void);
        return std::ptr::null_mut();
    }
    (*sock).sin_family = AF_INET as u16;
    // El puerto va en orden de red.
    (*sock).sin_port = puerto.to_be();
    (*sock).sin_addr = ip;
    (*sock).sin_zero = [0u8; 8];

    (*nodo).ai_flags = flags;
    (*nodo).ai_family = familia;
    (*nodo).ai_socktype = socktype;
    (*nodo).ai_protocol = protocolo;
    (*nodo).ai_addrlen = std::mem::size_of::<SockAddrIn>() as u32;
    (*nodo).ai_addr = sock as *mut c_void;
    (*nodo).ai_canonname = if flags & AI_CANONNAME != 0 && !nombre.is_null() {
        strdup(nombre)
    } else {
        std::ptr::null_mut()
    };
    (*nodo).ai_next = std::ptr::null_mut();
    nodo
}

/// Convierte el servicio que pide el llamante en un puerto.
///
/// curl llama con "https" o con el numero ("443").  Si no se puede saber, se
/// asume 443, que es lo que usa el motor para el auth server.
unsafe fn puerto_de(servicio: *const c_char) -> u16 {
    if servicio.is_null() {
        return 443;
    }
    let s = match CStr::from_ptr(servicio).to_str() {
        Ok(s) => s,
        Err(_) => return 443,
    };
    if let Ok(n) = s.parse::<u16>() {
        return n;
    }
    match s {
        "http" => 80,
        "https" => 443,
        _ => 443,
    }
}

/// getaddrinfo con resolucion propia como plan B.
///
/// La glibc no sabe resolver en Android (no hay /etc/resolv.conf), asi que
/// si su getaddrinfo falla se resuelve el nombre por UDP y se devuelve la
/// direccion.  Es lo que permite que el servidor de Factorio consiga el
/// padlock de auth.factorio.com.
#[no_mangle]
pub unsafe extern "C" fn getaddrinfo(
    node: *const c_char,
    service: *const c_char,
    hints: *const AddrInfo,
    res: *mut *mut AddrInfo,
) -> c_int {
    let original = getaddrinfo as *const ();
    // La funcion real de la glibc se recupera con dlsym(RTLD_NEXT) la
    // primera vez; si no se puede, se resuelve siempre por el camino propio.
    let real: Option<extern "C" fn(*const c_char, *const c_char, *const AddrInfo, *mut *mut AddrInfo) -> c_int> =
        unsafe { dlsym_next(original as *const c_void) };

    if let Some(real) = real {
        let rc = real(node, service, hints, res);
        if rc == 0 {
            if !node.is_null() {
                if let Ok(s) = CStr::from_ptr(node).to_str() {
                    registrar(&format!("getaddrinfo: {} resuelto por la glibc", s));
                }
            }
            return 0;
        }
        registrar("getaddrinfo: la glibc fallo, resuelvo yo");
    }

    if node.is_null() {
        // Sin nombre (AI_PASSIVE): no hay nada que inventar.
        return EAI_FAIL;
    }
    let nombre = match CStr::from_ptr(node).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return EAI_FAIL,
    };
    // "0.0.0.0" o vacio: son direcciones literales, no nombres.
    if nombre.is_empty() || nombre.parse::<std::net::Ipv4Addr>().is_ok() {
        return EAI_FAIL;
    }
    match resolver(&nombre) {
        Some(ip) => {
            registrar(&format!("getaddrinfo: {} -> {}", nombre, ip));
            let nodo = construir(node, ip, puerto_de(service), hints);
            if nodo.is_null() {
                return EAI_FAIL;
            }
            *res = nodo;
            0
        }
        None => {
            registrar(&format!("getaddrinfo: {} NO resuelto", nombre));
            EAI_FAIL
        }
    }
}

// dlsym(RTLD_NEXT, "getaddrinfo")
extern "C" {
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

const RTLD_NEXT: *mut c_void = -1isize as *mut c_void;

unsafe fn dlsym_next(
    _original: *const c_void,
) -> Option<extern "C" fn(*const c_char, *const c_char, *const AddrInfo, *mut *mut AddrInfo) -> c_int>
{
    let sym = b"getaddrinfo\0";
    let ptr = dlsym(RTLD_NEXT, sym.as_ptr() as *const c_char);
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute::<
            *mut c_void,
            extern "C" fn(*const c_char, *const c_char, *const AddrInfo, *mut *mut AddrInfo) -> c_int,
        >(ptr))
    }
}

// ---------------------------------------------------------------------------
// gethostbyname: algunos caminos viejos lo usan
// ---------------------------------------------------------------------------

#[no_mangle]
pub unsafe extern "C" fn gethostbyname(name: *const c_char) -> *mut HostEnt {
    if name.is_null() {
        return std::ptr::null_mut();
    }
    let nombre = match CStr::from_ptr(name).to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return std::ptr::null_mut(),
    };
    // Primero se deja que la glibc lo intente.
    let real: Option<extern "C" fn(*const c_char) -> *mut HostEnt> = unsafe {
        let sym = b"gethostbyname\0";
        let ptr = dlsym(RTLD_NEXT, sym.as_ptr() as *const c_char);
        if ptr.is_null() {
            None
        } else {
            Some(std::mem::transmute::<*mut c_void, extern "C" fn(*const c_char) -> *mut HostEnt>(ptr))
        }
    };
    if let Some(real) = real {
        let p = real(name);
        if !p.is_null() {
            return p;
        }
    }
    let ip = match resolver(&nombre) {
        Some(ip) => ip,
        None => return std::ptr::null_mut(),
    };
    let ent = malloc(std::mem::size_of::<HostEnt>()) as *mut HostEnt;
    if ent.is_null() {
        return std::ptr::null_mut();
    }
    let addr = malloc(4) as *mut u32;
    if addr.is_null() {
        free(ent as *mut c_void);
        return std::ptr::null_mut();
    }
    *addr = ip;
    let lista = malloc(std::mem::size_of::<*mut c_void>()) as *mut *mut c_void;
    if lista.is_null() {
        free(addr as *mut c_void);
        free(ent as *mut c_void);
        return std::ptr::null_mut();
    }
    *lista = addr as *mut c_void;
    (*ent).h_name = strdup(name);
    (*ent).h_aliases = std::ptr::null_mut();
    (*ent).h_addrtype = AF_INET;
    (*ent).h_length = 4;
    (*ent).h_addr_list = lista;
    ent
}

// Silencia el aviso de "unused" de memcpy, que se usa como util.
#[allow(dead_code)]
unsafe fn copiar(dest: *mut c_void, src: *const c_void, n: usize) {
    memcpy(dest, src, n);
}
