#version 320 es
// warp.vert — Proyección del terreno 2D de Factorio como un plano 3D horizontal sobre la mesa
precision highp float;

layout(location = 0) in vec2 in_pos; // [0..1] o generado por VertexID

layout(std140) uniform CameraCB {
    mat4 u_view_proj;
    float u_aspect;       // FBO width / height
    float u_plane_scale;  // Factor de escala del mundo
    float u_tex_w;        // Tamaño de píxeles del FBO
    float u_tex_h;
    vec4 u_sky;           // x = factor de noche (0..1)
};

// Configuración de la mesa física (Tabletop Volume)
uniform mat4 u_table_model;

out vec2 v_uv;
out vec3 v_world_pos;

void main() {
    // Si se dibuja con glDrawArrays(GL_TRIANGLE_STRIP, 0, 4) sin VBO:
    vec2 uv = vec2(float(gl_VertexID & 1), float(gl_VertexID >> 1));
    v_uv = uv;

    // Posición en el plano horizontal (Y = 0) de la fábrica
    vec3 local_pos = vec3(
        (uv.x - 0.5) * 2.0 * u_aspect * u_plane_scale,
        0.0,
        (0.5 - uv.y) * 2.0 * u_plane_scale
    );

    // Transformar a coordenadas del espacio de la mesa / habitación
    vec4 world_pos = u_table_model * vec4(local_pos, 1.0);
    v_world_pos = world_pos.xyz;

    gl_Position = u_view_proj * world_pos;
}
