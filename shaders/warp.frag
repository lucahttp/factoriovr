#version 320 es
// warp.frag — Muestreo nítido del suelo de Factorio con recorte en los bordes de la mesa física
precision highp float;

in vec2 v_uv;
in vec3 v_world_pos;

layout(std140) uniform CameraCB {
    mat4 u_view_proj;
    float u_aspect;
    float u_plane_scale;
    float u_tex_w;
    float u_tex_h;
    vec4 u_sky;
};

// Límites de la mesa física en metros (min_x, min_z, max_x, max_z)
uniform vec4 u_table_bounds;
// Posición central de la mesa física
uniform vec3 u_table_anchor;

uniform sampler2D u_scene_tex;

out vec4 frag_color;

void main() {
    // 1. Recorte en los bordes de la mesa (Stencil Box):
    // La maqueta no se dibuja fuera del área física del escritorio.
    if (v_world_pos.x < u_table_bounds.x || v_world_pos.x > u_table_bounds.z ||
        v_world_pos.z < u_table_bounds.y || v_world_pos.z > u_table_bounds.w) {
        discard;
    }

    // 2. Filtro Bilineal Definido (Sharp-Bilinear):
    // Evita que los sprites pixelados del terreno 2D se vuelvan borrosos al acercar la cabeza.
    float sharpen = max(u_plane_scale * 4.0, 6.0);
    vec2 ts = vec2(u_tex_w, u_tex_h);
    vec2 p = v_uv * ts;
    vec2 tf = floor(p);
    vec2 cd = (p - tf) - 0.5;
    vec2 rr = 0.5 - 0.5 / sharpen;
    vec2 f = (cd - clamp(cd, -rr, rr)) * sharpen + 0.5;

    vec4 color = texture(u_scene_tex, (tf + f) / ts);

    // 3. Suavizado suave en el perímetro de la mesa:
    float edge_dist_x = min(v_world_pos.x - u_table_bounds.x, u_table_bounds.z - v_world_pos.x);
    float edge_dist_z = min(v_world_pos.z - u_table_bounds.y, u_table_bounds.w - v_world_pos.z);
    float edge_dist = min(edge_dist_x, edge_dist_z);

    // Suavizado en los últimos 2 cm del borde de la mesa
    float edge_fade = clamp(edge_dist / 0.02, 0.0, 1.0);
    color.a *= edge_fade;

    frag_color = color;
}
