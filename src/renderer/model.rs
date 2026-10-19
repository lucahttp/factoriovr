// model.rs — Cargador de modelos GLTF y renderizador 3D instanciado para máquinas de Factorio
use anyhow::{Context, Result};
use glow::HasContext;
use glam::{Mat4, Vec4};
use std::path::Path;
use crate::renderer::shader::ShaderProgram;
use crate::tabletop::TabletopConfig;

#[allow(dead_code)]
pub struct GpuMesh {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    index_count: i32,
    albedo_tex: Option<glow::Texture>,
}

pub struct ModelRenderer {
    shader: ShaderProgram,
    #[allow(dead_code)]
    instance_vbo: glow::Buffer,
}

impl ModelRenderer {
    pub fn new(gl: &glow::Context) -> Result<Self> {
        let vert_src = include_str!("../../shaders/model.vert");
        let frag_src = include_str!("../../shaders/model.frag");
        let shader = ShaderProgram::new(gl, vert_src, frag_src)?;

        unsafe {
            let instance_vbo = gl.create_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Self { shader, instance_vbo })
        }
    }

    /// Carga una malla simple desde un archivo .glb
    pub fn load_glb<P: AsRef<Path>>(&self, gl: &glow::Context, path: P) -> Result<GpuMesh> {
        let (document, buffers, images) = gltf::import(path.as_ref())
            .with_context(|| format!("Error al importar GLB: {}", path.as_ref().display()))?;

        let mut positions: Vec<[f32; 3]> = Vec::new();
        let mut normals: Vec<[f32; 3]> = Vec::new();
        let mut uvs: Vec<[f32; 2]> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        for mesh in document.meshes() {
            for primitive in mesh.primitives() {
                let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));
                if let Some(pos_iter) = reader.read_positions() {
                    positions.extend(pos_iter);
                }
                if let Some(norm_iter) = reader.read_normals() {
                    normals.extend(norm_iter);
                }
                if let Some(tex_iter) = reader.read_tex_coords(0) {
                    uvs.extend(tex_iter.into_f32());
                }
                if let Some(idx_iter) = reader.read_indices() {
                    indices.extend(idx_iter.into_u32());
                }
            }
        }

        // Si faltan normales o uvs, rellenar valores por defecto
        if normals.len() < positions.len() {
            normals.resize(positions.len(), [0.0, 1.0, 0.0]);
        }
        if uvs.len() < positions.len() {
            uvs.resize(positions.len(), [0.0, 0.0]);
        }

        // Crear Vertex Array y Buffers
        unsafe {
            let vao = gl.create_vertex_array().map_err(|e| anyhow::anyhow!("{e}"))?;
            let vbo = gl.create_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;
            let ebo = gl.create_buffer().map_err(|e| anyhow::anyhow!("{e}"))?;

            gl.bind_vertex_array(Some(vao));

            // Interleaved data: pos (3), norm (3), uv (2) = 8 floats per vertex
            let mut vertex_data: Vec<f32> = Vec::with_capacity(positions.len() * 8);
            for i in 0..positions.len() {
                vertex_data.extend_from_slice(&positions[i]);
                vertex_data.extend_from_slice(&normals[i]);
                vertex_data.extend_from_slice(&uvs[i]);
            }

            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(
                glow::ARRAY_BUFFER,
                bytemuck_slice(&vertex_data),
                glow::STATIC_DRAW,
            );

            let stride = 8 * std::mem::size_of::<f32>() as i32;
            // Location 0: Position
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);

            // Location 1: Normal
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 3 * 4);

            // Location 2: UV
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, 6 * 4);

            // Element Buffer
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            gl.buffer_data_u8_slice(
                glow::ELEMENT_ARRAY_BUFFER,
                bytemuck_slice_u32(&indices),
                glow::STATIC_DRAW,
            );

            gl.bind_vertex_array(None);

            let mut albedo_tex = None;
            if let Some(img) = images.first() {
                let tex = gl.create_texture().map_err(|e| anyhow::anyhow!("{e}"))?;
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA as i32,
                    img.width as i32,
                    img.height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    Some(&img.pixels),
                );
                gl.generate_mipmap(glow::TEXTURE_2D);
                albedo_tex = Some(tex);
            }

            Ok(GpuMesh {
                vao,
                vbo,
                ebo,
                index_count: indices.len() as i32,
                albedo_tex,
            })
        }
    }

    /// Dibuja múltiples instancias de un modelo en la escena sobre la mesa
    pub fn draw_instances(
        &self,
        gl: &glow::Context,
        mesh: &GpuMesh,
        instance_transforms: &[Mat4],
        view_proj: &Mat4,
        table_config: &TabletopConfig,
        table_model: &Mat4,
    ) {
        if instance_transforms.is_empty() {
            return;
        }

        unsafe {
            self.shader.bind(gl);

            self.shader.set_mat4(gl, "u_mvp", view_proj);
            self.shader.set_mat4(gl, "u_table_model", table_model);

            // Parámetros de mesa física (Stencil Box)
            let half_w = table_config.table_dimensions.x * 0.5;
            let half_d = table_config.table_dimensions.y * 0.5;
            let bounds = Vec4::new(
                table_config.anchor_position.x - half_w,
                table_config.anchor_position.z - half_d,
                table_config.anchor_position.x + half_w,
                table_config.anchor_position.z + half_d,
            );
            self.shader.set_vec4(gl, "u_table_bounds", &bounds);
            self.shader.set_vec3(gl, "u_table_anchor", &table_config.anchor_position);

            // Albedo texture
            if let Some(tex) = mesh.albedo_tex {
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                self.shader.set_int(gl, "u_albedo_tex", 0);
                self.shader.set_vec4(gl, "u_opts", &Vec4::new(1.0, 0.0, 0.0, 0.0));
            } else {
                self.shader.set_vec4(gl, "u_opts", &Vec4::new(0.0, 0.0, 0.0, 0.0));
            }

            // Material y luz solar
            self.shader.set_vec4(gl, "u_base_color", &Vec4::ONE);
            self.shader.set_vec4(gl, "u_light", &Vec4::new(0.5, -1.0, 0.3, 0.0));
            self.shader.set_vec4(gl, "u_sun", &Vec4::new(1.0, 0.95, 0.9, 1.0));
            self.shader.set_vec4(gl, "u_sky", &Vec4::new(0.4, 0.6, 0.9, 0.5));
            self.shader.set_vec4(gl, "u_ground", &Vec4::new(0.2, 0.2, 0.15, 0.3));
            self.shader.set_vec4(gl, "u_mat", &Vec4::new(0.1, 0.5, 0.0, 0.0));
            self.shader.set_vec4(gl, "u_params", &Vec4::new(0.1, 1.0, 0.5, 0.0));
            self.shader.set_vec4(gl, "u_pbr", &Vec4::ZERO);
            self.shader.set_vec4(gl, "u_misc", &Vec4::new(0.0, 0.0, 1.0, 0.0));
            self.shader.set_vec4(gl, "u_cam", &Vec4::new(table_config.anchor_position.x, table_config.anchor_position.y + 0.5, table_config.anchor_position.z + 0.8, 1.0));

            // Dibujar cada instancia
            gl.bind_vertex_array(Some(mesh.vao));
            for transform in instance_transforms {
                self.shader.set_mat4(gl, "u_model", transform);
                self.shader.set_vec4(gl, "u_clipy", &Vec4::ZERO); // Non-instanced single draw
                gl.draw_elements(glow::TRIANGLES, mesh.index_count, glow::UNSIGNED_INT, 0);
            }
            gl.bind_vertex_array(None);
        }
    }
}

fn bytemuck_slice(data: &[f32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            data.as_ptr() as *const u8,
            data.len() * std::mem::size_of::<f32>(),
        )
    }
}

fn bytemuck_slice_u32(data: &[u32]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            data.as_ptr() as *const u8,
            data.len() * std::mem::size_of::<u32>(),
        )
    }
}
