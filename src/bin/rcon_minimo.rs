//! rcon_minimo.rs — Cliente RCON minimo, copiado del sondeo en Python
//! (scripts/rcon_probe.py).
//!
//! Se usa para seccionar el problema del ADR-011: si esta copia minima
//! habla con el motor, el fallo esta en el cliente del crate
//! (src/game_bridge.rs); si no, es del servidor o del entorno.
//!
//! Uso: rcon_minimo [host:puerto] [password]

fn rcon_minimo(host: &str, port: u16, password: &str) {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    fn pack(id: i32, ptype: i32, body: &str) -> Vec<u8> {
        let mut payload = body.as_bytes().to_vec();
        payload.push(0);
        payload.push(0);
        let mut out = Vec::new();
        out.extend_from_slice(&(payload.len() as i32 + 8).to_le_bytes());
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&ptype.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Lectura EXACTAMENTE como la del sondeo: 12 bytes de cabecera y
    /// despues `length - 8` del cuerpo (que incluyen los dos nulls).
    fn leer(s: &mut TcpStream) -> Option<(i32, i32, String)> {
        let mut header = [0u8; 12];
        s.read_exact(&mut header).ok()?;
        let length = i32::from_le_bytes(header[0..4].try_into().ok()?);
        let req_id = i32::from_le_bytes(header[4..8].try_into().ok()?);
        let ptype = i32::from_le_bytes(header[8..12].try_into().ok()?);
        let mut body = vec![0u8; (length - 8).max(0) as usize];
        if !body.is_empty() {
            s.read_exact(&mut body).ok()?;
        }
        if body.ends_with(&[0u8, 0u8]) {
            body.truncate(body.len() - 2);
        }
        Some((req_id, ptype, String::from_utf8_lossy(&body).to_string()))
    }

    let addr = format!("{host}:{port}");
    let mut s = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(e) => {
            println!("minimo: no se pudo conectar a {addr}: {e}");
            return;
        }
    };
    let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = s.set_write_timeout(Some(Duration::from_secs(5)));

    s.write_all(&pack(1, 3, password)).ok();
    match leer(&mut s) {
        Some((_, ptype, body)) => println!("minimo: auth -> tipo={ptype} cuerpo={body:?}"),
        None => println!("minimo: no llego la respuesta del auth"),
    }

    let comandos = [
        "/silent-command rcon.print('listo')",
        "/silent-command rcon.print('hola')",
        "/silent-command remote.call('xfactorvr', 'get_world_snapshot', 32, 0, 0)",
    ];
    for (i, cmd) in comandos.iter().enumerate() {
        s.write_all(&pack(i as i32 + 2, 2, cmd)).ok();
        match leer(&mut s) {
            Some((_, _, body)) => {
                let muestra: String = body.chars().take(160).collect();
                println!("minimo: {cmd} -> {muestra:?}");
            }
            None => println!("minimo: {cmd} -> NO LLEGO NADA"),
        }
    }
}

fn main() {
    let argumentos: Vec<String> = std::env::args().collect();
    let direccion = argumentos
        .get(1)
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:27015".to_string());
    let password = argumentos
        .get(2)
        .cloned()
        .unwrap_or_else(|| "xfactorvr".to_string());
    let (host, puerto) = match direccion.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap_or(27015)),
        None => (direccion.clone(), 27015),
    };
    println!("--- cliente RCON minimo contra {direccion} ---");
    rcon_minimo(&host, puerto, &password);
}
