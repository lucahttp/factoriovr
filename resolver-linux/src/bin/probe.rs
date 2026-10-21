//! Sonda de resolucion de nombres para el visor.
//!
//! Compila para aarch64-unknown-linux-gnu (glibc) y se ejecuta en la Pico con
//! el linker de glibc del payload:
//!
//!   /data/local/tmp/glibc/lib/ld-linux-aarch64.so.1 \
//!       --library-path /data/local/tmp/glibc/lib \
//!       /data/local/tmp/probe
//!
//! Sin el shim (resolver-linux) falla con "Could not resolve host"; con el
//! shim deberia resolver.

use std::net::ToSocketAddrs;

// El ejecutable tambien necesita los stubs del desenrollador: el sysroot de
// glibc de Termux no trae libgcc_s.
#[path = "../gnu_unwind.rs"]
mod gnu_unwind;

fn main() {
    let mut fallos = 0;
    for host in ["auth.factorio.com", "www.factorio.com", "google.com"] {
        match (host, 443u16).to_socket_addrs() {
            Ok(mut addrs) => match addrs.next() {
                Some(a) => println!("[ok]   {} -> {}", host, a),
                None => {
                    println!("[vacio] {}", host);
                    fallos += 1;
                }
            },
            Err(e) => {
                println!("[error] {} -> {}", host, e);
                fallos += 1;
            }
        }
    }
    println!("fallos: {}", fallos);
}
