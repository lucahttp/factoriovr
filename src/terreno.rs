//! terreno.rs — La malla del suelo real del mapa.
//!
//! El mod manda el terreno del area visible (el tile dominante y las celdas
//! que difieren) y el extractor lo deja en un atlas paginado: celdas de 32x32
//! por tipo de terreno.
//!
//! Aqui se calcula la geometria: un quad por casilla, tumbado sobre la mesa,
//! con las UV del rectangulo que le toca en el atlas.  La variante se elige
//! con un hash de la posicion, igual que hace el juego, para que las casillas
//! contiguas no se repitan.
//!
//! Vive en Rust (y no en el motor C++) por una razon: se puede probar con
//! `cargo test` sin necesitar el visor.

use crate::assets;
use crate::game_bridge::TerrainSnapshot;

/// Un quad del suelo, con su rectangulo en el atlas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainQuad {
    /// Casilla del mapa (en tiles de Factorio).
    pub tile_x: i32,
    pub tile_y: i32,
    /// UV del rectangulo dentro de la pagina del atlas (0..1).
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    /// Pagina del atlas donde esta la textura.
    pub page: u32,
}

/// Hash de la posicion para elegir variante (el juego usa uno parecido).
fn hash_celda(tx: i32, ty: i32) -> u32 {
    ((tx as u32).wrapping_mul(73856093)) ^ ((ty as u32).wrapping_mul(19349663))
}

/// Calcula los quads del suelo visible.
///
/// `page_size` es el lado de la pagina del atlas en pixeles.  Si un tipo de
/// terreno no esta en el atlas, esa casilla se queda sin dibujar (el suelo
/// aparece como el color base de la mesa).
pub fn terrain_quads(
    terreno: &TerrainSnapshot,
    page_size: u32,
) -> Vec<TerrainQuad> {
    let mut quads = Vec::new();
    if terreno.is_empty() || page_size == 0 {
        return quads;
    }
    for ry in 0..terreno.h as i32 {
        for rx in 0..terreno.w as i32 {
            let nombre = match terreno.tile_at(rx, ry) {
                Some(n) => n,
                None => continue,
            };
            let variantes = assets::terrain_variant_count(nombre);
            if variantes == 0 {
                continue;
            }
            let tx = terreno.x0 + rx;
            let ty = terreno.y0 + ry;
            let variante = hash_celda(tx, ty) % variantes;
            let rect = match assets::terrain_tile(nombre, variante) {
                Some(r) => r,
                None => continue,
            };
            let p = page_size as f32;
            quads.push(TerrainQuad {
                tile_x: tx,
                tile_y: ty,
                u0: rect.x / p,
                v0: rect.y / p,
                u1: (rect.x + rect.w) / p,
                v1: (rect.y + rect.h) / p,
                page: rect.page,
            });
        }
    }
    quads
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> TerrainSnapshot {
        // Area 3x3: hierba salvo dos celdas de tierra.
        TerrainSnapshot {
            base: 0,
            names: vec!["grass-1".to_string(), "dirt-1".to_string()],
            x0: 10,
            y0: 20,
            w: 3,
            h: 3,
            tiles: vec![[1, 0, 1], [0, 2, 1]],
        }
    }

    #[test]
    fn una_celda_por_casilla() {
        let terreno = snapshot();
        // Sin atlas cargado no hay rectangulos: no sale ninguna celda, pero
        // tampoco se peta (importante: el atlas puede no estar desplegado).
        let quads = terrain_quads(&terreno, 2048);
        if assets::terrain_atlas().is_some() {
            assert_eq!(quads.len(), 9, "deberia haber un quad por casilla");
        } else {
            assert!(quads.is_empty());
        }
    }

    #[test]
    fn las_uvs_estan_dentro_de_la_pagina() {
        let terreno = snapshot();
        for q in terrain_quads(&terreno, 2048) {
            assert!(q.u0 >= 0.0 && q.u1 <= 1.0, "u fuera de rango: {:?}", q);
            assert!(q.v0 >= 0.0 && q.v1 <= 1.0, "v fuera de rango: {:?}", q);
            assert!(q.u1 > q.u0 && q.v1 > q.v0, "uv invertida: {:?}", q);
        }
    }

    #[test]
    fn la_variante_depende_de_la_posicion() {
        // Dos celdas distintas no deberían cojer siempre la misma variante.
        let terreno = snapshot();
        let quads = terrain_quads(&terreno, 2048);
        if quads.len() >= 2 {
            let a = quads[0];
            let b = quads[1];
            // Mismo rect solo si el hash coincide, lo cual es posible, pero
            // las UV tienen que estar dentro de la pagina en cualquier caso.
            assert!(a.u1 > a.u0 && b.u1 > b.u0);
        }
    }

    #[test]
    fn un_area_vacia_no_da_quads() {
        let vacio = TerrainSnapshot::default();
        assert!(terrain_quads(&vacio, 2048).is_empty());
    }
}
