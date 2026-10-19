#version 320 es
// model.vert — Renderizado de mallas 3D GLTF de Factorio sobre la mesa con instancing e IK
precision highp float;

layout(location = 0) in vec3 in_position;
layout(location = 1) in vec3 in_normal;
layout(location = 2) in vec2 in_uv;
layout(location = 3) in vec4 in_joints;
layout(location = 4) in vec4 in_weights;
// Instanced matrix (para miles de ítems y tramos de cintas en un solo draw call):
layout(location = 5) in mat4 in_instance_model;

layout(std140) uniform ModelCB {
    mat4 u_mvp;
    mat4 u_model;
    vec4 u_base_color;
    vec4 u_opts;     // x: has_tex, y: morph_weight, z: pass (0:lit, 1:shadow, 2:wire), w: skinned
    vec4 u_light;    // xyz: dirección del sol
    vec4 u_misc;     // xy: uv scroll (treads de cintas), z: alpha global
    vec4 u_clipv;    // planos de corte de uniones
    vec4 u_mat;      // x: metallic, y: roughness, z: emissive, w: night
    vec4 u_emissive; // xyz: color emisivo, w: has_emissive_tex
    vec4 u_cam;      // xyz: posición de la cámara / ojos, w: exposición
    vec4 u_sky;      // xyz: ambiente cielo, w: fuerza
    vec4 u_ground;   // xyz: rebote suelo, w: fuerza de borde
    vec4 u_sun;      // xyz: color sol, w: fuerza sol
    vec4 u_params;   // x: piso nocturno, y: escala specular, z: reflejo env, w: parpadeo
    vec4 u_pbr;      // x: normal_map, y: mr_map, z: is_orm, w: tiempo
    vec4 u_clipy;    // x: altura máxima de recorte, y: instanced (>0.5)
};

// Matriz que ancla la maqueta a la mesa física real en OpenXR
uniform mat4 u_table_model;

// Paleta de huesos para entidades articuladas (articulaciones de inserters / robots)
uniform mat4 u_bones[96];

out vec3 v_normal;
out vec2 v_uv;
out vec3 v_world_pos;

void main() {
    vec3 p = in_position;
    vec3 n = in_normal;

    // Skinning / Esqueleto para brazos de inserters y personajes
    if (u_opts.w > 0.5) {
        vec3 sp = vec3(0.0);
        vec3 sn = vec3(0.0);
        for (int k = 0; k < 4; ++k) {
            float w = in_weights[k];
            if (w > 0.0) {
                int j_idx = int(in_joints[k]);
                mat4 b = u_bones[j_idx];
                sp += w * (b * vec4(p, 1.0)).xyz;
                sn += w * (mat3(b) * n);
            }
        }
        p = sp;
        n = sn;
    }

    vec4 world_pos;
    vec3 world_normal;

    // Dibujo instanciado para cintas de transporte e ítems en masa
    if (u_clipy.y > 0.5) {
        world_pos = in_instance_model * vec4(p, 1.0);
        world_normal = mat3(in_instance_model) * n;
    } else {
        world_pos = u_model * vec4(p, 1.0);
        world_normal = mat3(u_model) * n;
    }

    // Transformación final a espacio de habitación/mesa
    vec4 final_table_pos = u_table_model * world_pos;
    v_world_pos = final_table_pos.xyz;
    v_normal = normalize(mat3(u_table_model) * world_normal);

    vec2 uv = in_uv;
    // Animación de cintas transportadoras: desplazamiento de textura en caras superiores
    if (v_normal.y > 0.5) {
        uv += u_misc.xy;
    }
    v_uv = uv;

    gl_Position = u_mvp * final_table_pos;
}
