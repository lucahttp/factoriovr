//! model.rs — Mallas 3D instanciadas de las maquinas de Factorio.

// En la edicion 2021 el cuerpo de una `unsafe fn` ya es un bloque
// `unsafe`: los bloques de aqui son redundantes (y en la 2024 harian falta).
#![allow(unused_unsafe)]
//!
//! Cada GLB del mod `factorio-3d-mod` se sube UNA vez a la GPU (VAO + VBO)
//! y se dibuja con `glDrawArraysInstanced`: una matriz por entidad.  Todas
//! las entidades que comparten prototipo salen en la MISMA llamada, asi que
//! una fabrica entera son unas 30 llamadas, no una por maquina.
//!
//! El vertex format es el mismo que usa el diorama (`DioramaVertex`), pero
//! en vez de un atlas de texturas el color va horneado en el vertice
//! (el GLB optimizado trae COLOR_0), asi que no hay ni una sola textura.

use crate::renderer::gl::*;
use crate::renderer::shader::ShaderProgram;
use crate::tabletop::DioramaVertex;
use anyhow::{anyhow, Result};
use glam::Mat4;
use std::collections::HashMap;

/// Floats por vertice: posicion(3) + normal(3) + uv(2) + color(4).
pub const FLOATS_POR_VERTICE: usize = 12;

/// Una malla subida a la GPU.
pub struct GpuMesh {
    pub vao: GLuint,
    pub vbo: GLuint,
    /// Buffer de indices.
    pub ebo: GLuint,
    /// Numero de vertices unicos de la malla.
    pub vertex_count: i32,
    /// Numero de indices (3 por triangulo).
    pub index_count: i32,
}

/// Luces y ajustes del render de las maquinas.
#[derive(Clone, Copy, Debug)]
pub struct Ajustes {
    /// Direccion HACIA la luz.
    pub sol_dir: [f32; 3],
    /// Color del sol.
    pub sol_color: [f32; 3],
    /// Ambiente de cielo.
    pub amb_cielo: [f32; 3],
    /// Rebote del suelo.
    pub amb_suelo: [f32; 3],
    /// Posicion de la camara en el espacio de la MESA.
    pub ojos: [f32; 3],
    /// Exposicion.
    pub exposicion: f32,
    /// Recorte a la mesa fisica: (min_x, min_z, max_x, max_z).
    pub recorte: [f32; 4],
    /// Si el recorte esta activo.
    pub recorte_activo: bool,
    /// Altura de la base de la mesa (por debajo se descarta).
    pub y_min: f32,
}

impl Default for Ajustes {
    fn default() -> Self {
        Self {
            sol_dir: [-0.35, 0.85, 0.4],
            sol_color: [0.95, 0.9, 0.82],
            amb_cielo: [0.32, 0.4, 0.5],
            amb_suelo: [0.16, 0.15, 0.13],
            ojos: [0.0, 0.4, 0.0],
            exposicion: 1.15,
            recorte: [0.0, 0.0, 0.0, 0.0],
            recorte_activo: false,
            y_min: 0.0,
        }
    }
}

/// Renderer de maquinas: programa, buffer de instancias y cache de mallas.
pub struct ModelRenderer {
    shader: ShaderProgram,
    /// VBO con las matrices de instancia (una mat4 = 16 floats por entidad).
    instance_vbo: GLuint,
    /// Capacidad del buffer de instancias (en matrices).
    capacidad: usize,
    /// Mallas ya subidas, por fichero GLB.
    mallas: HashMap<String, GpuMesh>,
}

impl ModelRenderer {
    /// Crea el renderer (compila el programa de las maquinas).
    pub fn new(gl: &Gl) -> Result<Self> {
        let shader = ShaderProgram::new(
            gl,
            &crate::renderer::shader::vert_modelo(),
            &crate::renderer::shader::frag_modelo(),
        )?;
        unsafe {
            let mut vbo: GLuint = 0;
            (gl.GenBuffers)(1, &mut vbo);
            if vbo == 0 {
                return Err(anyhow!("no se pudo crear el VBO de instancias"));
            }
            Ok(ModelRenderer {
                shader,
                instance_vbo: vbo,
                capacidad: 0,
                mallas: HashMap::new(),
            })
        }
    }

    /// Sube una malla a la GPU (o la saca de la cache).
    ///
    /// `verts` son los vertices unicos y `indices` los triangulos: se sube
    /// INDEXADO (3 veces menos vertices que procesar en la GPU, que en VR
    /// se nota).
    pub fn subir_malla(
        &mut self,
        gl: &Gl,
        archivo: &str,
        verts: &[DioramaVertex],
        indices: &[u32],
    ) -> Result<()> {
        if self.mallas.contains_key(archivo) {
            return Ok(());
        }
        if verts.is_empty() || indices.is_empty() {
            return Err(anyhow!("la malla {archivo} no tiene geometria"));
        }
        // Vuelca los vertices a un buffer plano (pos, nrm, uv, color).
        let mut datos: Vec<f32> = Vec::with_capacity(verts.len() * FLOATS_POR_VERTICE);
        for v in verts {
            datos.extend_from_slice(&[v.x, v.y, v.z, v.nx, v.ny, v.nz, v.u, v.v, v.r, v.g, v.b, v.a]);
        }
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                datos.as_ptr() as *const u8,
                datos.len() * std::mem::size_of::<f32>(),
            )
        };
        unsafe {
            let mut vao: GLuint = 0;
            let mut vbo: GLuint = 0;
            let mut ebo: GLuint = 0;
            (gl.GenVertexArrays)(1, &mut vao);
            (gl.GenBuffers)(1, &mut vbo);
            (gl.GenBuffers)(1, &mut ebo);
            (gl.BindVertexArray)(vao);

            (gl.BindBuffer)(ARRAY_BUFFER, vbo);
            (gl.BufferData)(
                ARRAY_BUFFER,
                bytes.len() as GLsizeiptr,
                bytes.as_ptr() as *const std::os::raw::c_void,
                STATIC_DRAW,
            );

            let stride = (FLOATS_POR_VERTICE * std::mem::size_of::<f32>()) as GLsizei;
            // 0: posicion, 1: normal, 2: uv, 3: color
            (gl.EnableVertexAttribArray)(0);
            (gl.VertexAttribPointer)(
                0,
                3,
                FLOAT,
                FALSE_,
                stride,
                0 as *const std::os::raw::c_void,
            );
            (gl.EnableVertexAttribArray)(1);
            (gl.VertexAttribPointer)(
                1,
                3,
                FLOAT,
                FALSE_,
                stride,
                (3 * 4) as *const std::os::raw::c_void,
            );
            (gl.EnableVertexAttribArray)(2);
            (gl.VertexAttribPointer)(
                2,
                2,
                FLOAT,
                FALSE_,
                stride,
                (6 * 4) as *const std::os::raw::c_void,
            );
            (gl.EnableVertexAttribArray)(3);
            (gl.VertexAttribPointer)(
                3,
                4,
                FLOAT,
                FALSE_,
                stride,
                (8 * 4) as *const std::os::raw::c_void,
            );
            // Los indices (el EBO SI forma parte del VAO).
            (gl.BindBuffer)(ELEMENT_ARRAY_BUFFER, ebo);
            let ibytes: &[u8] = unsafe {
                std::slice::from_raw_parts(
                    indices.as_ptr() as *const u8,
                    indices.len() * std::mem::size_of::<u32>(),
                )
            };
            (gl.BufferData)(
                ELEMENT_ARRAY_BUFFER,
                ibytes.len() as GLsizeiptr,
                ibytes.as_ptr() as *const std::os::raw::c_void,
                STATIC_DRAW,
            );

            (gl.BindVertexArray)(0);
            (gl.BindBuffer)(ARRAY_BUFFER, 0);
            (gl.BindBuffer)(ELEMENT_ARRAY_BUFFER, 0);

            self.mallas.insert(
                archivo.to_string(),
                GpuMesh {
                    vao,
                    vbo,
                    ebo,
                    vertex_count: verts.len() as i32,
                    index_count: indices.len() as i32,
                },
            );
        }
        Ok(())
    }

    /// Numero de mallas en la GPU.
    pub fn mallas_en_gpu(&self) -> usize {
        self.mallas.len()
    }

    /// Dibuja una malla con varias instancias (una matriz por entidad).
    ///
    /// `mvp` es vista * tabla (la tabla ya coloca la maqueta en la mesa
    /// fisica de la habitacion).
    pub fn dibujar_instancias(
        &mut self,
        gl: &Gl,
        archivo: &str,
        instancias: &[Mat4],
        mvp: &Mat4,
        ajustes: &Ajustes,
    ) -> u32 {
        if instancias.is_empty() {
            return 0;
        }
        let Some(mesh) = self.mallas.get(archivo) else {
            return 0;
        };
        let n = instancias.len();

        // El buffer de instancias: 16 floats por matriz.
        let mut planos: Vec<f32> = Vec::with_capacity(n * 16);
        for m in instancias {
            planos.extend_from_slice(&m.to_cols_array());
        }
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                planos.as_ptr() as *const u8,
                planos.len() * std::mem::size_of::<f32>(),
            )
        };
        unsafe {
            (gl.BindBuffer)(ARRAY_BUFFER, self.instance_vbo);
            if n > self.capacidad {
                (gl.BufferData)(
                    ARRAY_BUFFER,
                    bytes.len() as GLsizeiptr,
                    bytes.as_ptr() as *const std::os::raw::c_void,
                    DYNAMIC_DRAW,
                );
                self.capacidad = n;
            } else {
                (gl.BufferSubData)(
                    ARRAY_BUFFER,
                    0,
                    bytes.len() as GLsizeiptr,
                    bytes.as_ptr() as *const std::os::raw::c_void,
                );
            }

            // Los atributos de instancia (4..7) viven en el VAO de la malla.
            (gl.BindVertexArray)(mesh.vao);
            (gl.BindBuffer)(ARRAY_BUFFER, self.instance_vbo);
            let stride = 16 * std::mem::size_of::<f32>() as GLsizei;
            for i in 0..4u32 {
                let loc = 4 + i;
                (gl.EnableVertexAttribArray)(loc);
                (gl.VertexAttribPointer)(
                    loc,
                    4,
                    FLOAT,
                    FALSE_,
                    stride,
                    (i as usize * 4 * std::mem::size_of::<f32>())
                        as *const std::os::raw::c_void,
                );
                (gl.VertexAttribDivisor)(loc, 1);
            }

            self.shader.bind(gl);
            self.shader.set_mat4(gl, "u_mvp", mvp);
            self.shader.set_v3(gl, "u_sol_dir", ajustes.sol_dir);
            self.shader.set_v3(gl, "u_sol_color", ajustes.sol_color);
            self.shader.set_v3(gl, "u_amb_cielo", ajustes.amb_cielo);
            self.shader.set_v3(gl, "u_amb_suelo", ajustes.amb_suelo);
            self.shader.set_v3(gl, "u_ojos", ajustes.ojos);
            self.shader.set_f(gl, "u_exposicion", ajustes.exposicion);
            self.shader.set_v4(gl, "u_recorte", ajustes.recorte);
            self.shader
                .set_f(gl, "u_recorte_activo", if ajustes.recorte_activo { 1.0 } else { 0.0 });
            self.shader.set_f(gl, "u_y_min", ajustes.y_min);

            (gl.DrawElementsInstanced)(
                TRIANGLES,
                mesh.index_count,
                UNSIGNED_INT,
                std::ptr::null(),
                n as GLsizei,
            );

            (gl.BindVertexArray)(0);
            (gl.BindBuffer)(ARRAY_BUFFER, 0);
        }
        n as u32
    }
}

/// Calcula la posicion de la camara en el espacio de la MESA a partir de su
/// posicion en la habitacion y de la matriz de la tabla.
///
/// `tabla` coloca la maqueta (metros) en la habitacion; su inversa lleva
/// la camara de vuelta al espacio de la maqueta.
pub fn ojos_en_mesa(tabla: &Mat4, ojos_habitacion: [f32; 3]) -> [f32; 3] {
    let inv = tabla.inverse();
    let v = inv * glam::Vec4::new(ojos_habitacion[0], ojos_habitacion[1], ojos_habitacion[2], 1.0);
    [v.x, v.y, v.z]
}
