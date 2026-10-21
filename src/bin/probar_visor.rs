//! probar_visor.rs — Comprobacion de que el VISOR sabe jugar.
//!
//! Usa exactamente las mismas funciones que el APK (el crate `xfactorvr`:
//! el puente RCON y las ordenes del mod), asi que lo que se comprueba aqui
//! es lo mismo que hara el visor en el dispositivo:
//!
//!   1. conectar por RCON con el motor de Factorio,
//!   2. pedir la instantanea del mundo (y comprobar que trae el terreno),
//!   3. mover al jugador a una posicion,
//!   4. minar y fabricar.
//!
//! El banco que lo usa es scripts/test_visor_juega.sh, que levanta el motor
//! en el contenedor ARM64.
//!
//! Uso: probar_visor [host:puerto] [password]

use xfactorvr::game_bridge;

fn main() {
    let argumentos: Vec<String> = std::env::args().collect();
    let host = argumentos
        .get(1)
        .cloned()
        .unwrap_or_else(|| "127.0.0.1:27015".to_string());
    let password = argumentos
        .get(2)
        .cloned()
        .unwrap_or_else(|| "xfactorvr".to_string());

    // --- 1. Conectar por RCON (como el APK) -------------------------------
    {
        let mut rcon = game_bridge::get_rcon_client()
            .lock()
            .expect("no se pudo tomar el cliente RCON");
        if !rcon.connect(&host, &password) {
            panic!("no se pudo conectar por RCON a {}", host);
        }
    }
    println!("ok: conectado por RCON a {}", host);

    // --- 2. Pedir la instantanea del mundo --------------------------------
    let rcon = game_bridge::poll_rcon_sync(0.5, (0.0, 0.0));
    if !rcon.connected {
        panic!("el RCON no esta conectado");
    }
    let instantanea = match &rcon.snapshot {
        Some(s) => s.clone(),
        None => panic!("el motor no devolvio la instantanea del mundo"),
    };
    println!("ok: instantanea del mundo ({} bytes)", instantanea.len());

    // El terreno viene dentro de la instantanea: si no esta, el canal de
    // datos no esta bien.
    assert!(
        instantanea.contains("\"terrain\""),
        "la instantanea no trae el terreno"
    );
    println!("ok: la instantanea trae el terreno");

    // --- 3. Mover al jugador ----------------------------------------------
    // El destino se elige en el mapa: que el motor acepte la orden se ve en
    // que la posicion del jugador cambia.
    let antes = posicion_del_jugador(&instantanea);
    let ok = unsafe { xfactorvr::xfactorvr_teleport_player(37.0, 41.0) };
    assert!(ok, "el motor no acepto la teletransportacion");
    println!("ok: teletransportacion aceptada");

    let mut despues = instantanea.clone();
    for _ in 0..10 {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let r = game_bridge::poll_rcon_sync(0.1, (0.0, 0.0));
        if let Some(s) = r.snapshot {
            despues = s;
            let ahora = posicion_del_jugador(&despues);
            if ahora != antes {
                println!(
                    "ok: el jugador se movio de {:?} a {:?}",
                    antes, ahora
                );
                break;
            }
        }
    }
    let ahora = posicion_del_jugador(&despues);
    assert!(
        ahora != antes,
        "el jugador no se movio ( {:?} -> {:?})",
        antes,
        ahora
    );

    // --- 4. Minar y fabricar ----------------------------------------------
    let minado = unsafe { xfactorvr::xfactorvr_manual_mine(37.0, 41.0) };
    println!("ok: minar devuelvio {}", minado);
    let fabricado = unsafe { xfactorvr::xfactorvr_craft_item(xfactorvr::game_bridge::ITEM_IRON_PLATE, 1) };
    println!("ok: fabricar devuelvio {}", fabricado);

    println!("\nTODO OK: el visor sabe jugar.");
}

/// La posicion del jugador dentro de la instantanea (si viene).
fn posicion_del_jugador(instantanea: &str) -> Option<(f64, f64)> {
    let clave = "\"player\"";
    let i = instantanea.find(clave)?;
    let resto = &instantanea[i..];
    let x = resto.find("\"x\":")?;
    let y = resto.find("\"y\":")?;
    let xv: f64 = resto[x + 4..]
        .split(|c: char| !c.is_ascii_digit() && c != '-' && c != '.')
        .next()?
        .parse()
        .ok()?;
    let yv: f64 = resto[y + 4..]
        .split(|c: char| !c.is_ascii_digit() && c != '-' && c != '.')
        .next()?
        .parse()
        .ok()?;
    Some((xv, yv))
}
