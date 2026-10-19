#version 320 es
// model.frag — Iluminación PBR GGX, ciclo día/noche y recorte de mesa física para modelos 3D
precision highp float;

in vec3 v_normal;
in vec2 v_uv;
in vec3 v_world_pos;

layout(std140) uniform ModelCB {
    mat4 u_mvp;
    mat4 u_model;
    vec4 u_base_color;
    vec4 u_opts;     // x: has_tex, y: morph_weight, z: pass (0:lit, 1:shadow, 2:wire), w: skinned
    vec4 u_light;    // xyz: dirección del sol
    vec4 u_misc;     // xy: uv scroll, z: alpha global
    vec4 u_clipv;    // planos de corte
    vec4 u_mat;      // x: metallic, y: roughness, z: emissive, w: night
    vec4 u_emissive; // xyz: color emisivo, w: has_emissive_tex
    vec4 u_cam;      // xyz: posición de la cámara / ojos, w: exposición
    vec4 u_sky;      // xyz: ambiente cielo, w: fuerza
    vec4 u_ground;   // xyz: rebote suelo, w: fuerza de borde
    vec4 u_sun;      // xyz: color sol, w: fuerza sol
    vec4 u_params;   // x: piso nocturno, y: escala specular, z: reflejo env, w: parpadeo
    vec4 u_pbr;      // x: normal_map, y: mr_map, z: is_orm, w: tiempo
    vec4 u_clipy;    // x: altura máxima de recorte
};

uniform vec4 u_table_bounds; // Límites físicos de la mesa (min_x, min_z, max_x, max_z)
uniform vec3 u_table_anchor;

uniform sampler2D u_albedo_tex;
uniform sampler2D u_emissive_tex;
uniform sampler2D u_normal_tex;
uniform sampler2D u_mr_tex;

out vec4 frag_color;

vec3 toLinear(vec3 c) { return pow(max(c, vec3(0.0)), vec3(2.2)); }
vec3 toSRGB(vec3 c)   { return pow(max(c, vec3(0.0)), vec3(1.0 / 2.2)); }

// Mapeo tonal ACES filmic
vec3 aces(vec3 x) {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}

void main() {
    // 1. Recorte en los bordes físicos de la mesa:
    if (v_world_pos.x < u_table_bounds.x || v_world_pos.x > u_table_bounds.z ||
        v_world_pos.z < u_table_bounds.y || v_world_pos.z > u_table_bounds.w) {
        discard;
    }

    // 2. Ocultar geometría subterránea (por debajo del tablero de la mesa)
    if (v_world_pos.y < u_table_anchor.y - 0.005) {
        discard;
    }

    vec3 N = normalize(v_normal);

    // Pase de sombra proyectada sobre la mesa (plano oscuro translúcido)
    if (u_opts.z > 0.5 && u_opts.z < 1.5) {
        frag_color = u_base_color;
        return;
    }

    // Pase de cables eléctricos suspendidos
    if (u_opts.z > 1.5) {
        vec3 L = normalize(-u_light.xyz);
        float d = clamp(dot(N, L), 0.0, 1.0) * 0.4 + 0.6;
        vec3 c = toSRGB(aces(toLinear(u_base_color.rgb) * d * u_cam.w));
        frag_color = vec4(c, u_misc.z);
        return;
    }

    vec4 albedo4 = u_base_color;
    if (u_opts.x > 0.5) {
        albedo4 *= texture(u_albedo_tex, v_uv);
        if (albedo4.a < 0.5) discard;
    }
    vec3 albedo = toLinear(albedo4.rgb);

    float metallic = clamp(u_mat.x, 0.0, 1.0);
    float rough = u_mat.y;
    float ao = 1.0;

    if (u_pbr.y > 0.5) {
        vec3 mr = texture(u_mr_tex, v_uv).rgb;
        rough *= mr.g;
        metallic = clamp(metallic * mr.b, 0.0, 1.0);
        if (u_pbr.z > 0.5) ao = mr.r;
    }
    rough = clamp(rough, 0.05, 1.0);

    float night = clamp(u_mat.w, 0.0, 1.0);
    float day = 1.0 - night;

    vec3 L = normalize(-u_light.xyz);
    vec3 V = normalize(u_cam.xyz - v_world_pos);
    vec3 H = normalize(L + V);

    // Luz ambiental hemisférica
    float hemi = N.y * 0.5 + 0.5;
    vec3 ambCol = mix(u_ground.xyz, u_sky.xyz, hemi) * u_sky.w * ao;
    ambCol *= mix(u_params.x, 1.0, day);

    // Difusa solar con terminador suave
    float wrap = clamp((dot(N, L) + 0.35) / 1.35, 0.0, 1.0);
    vec3 sunCol = u_sun.xyz * u_sun.w * day;

    // Especular PBR GGX
    vec3 F0 = mix(vec3(0.04), albedo, metallic);
    float ndv = clamp(dot(N, V), 0.0, 1.0);
    vec3 Fr = F0 + (max(vec3(1.0 - rough), F0) - F0) * pow(1.0 - ndv, 5.0);

    float ndh = clamp(dot(N, H), 0.0, 1.0);
    float ndl = clamp(dot(N, L), 0.0, 1.0);
    float a = rough * rough;
    float a2 = a * a;
    float dd = ndh * ndh * (a2 - 1.0) + 1.0;
    float D = a2 / max(3.14159 * dd * dd, 1e-6);
    float k = a * 0.5;
    float vis = 0.25 / max((ndv * (1.0 - k) + k) * (ndl * (1.0 - k) + k), 1e-4);
    vec3 specular = Fr * (D * vis) * ndl * sunCol * u_params.y;

    // Conservación de energía
    vec3 kd = (vec3(1.0) - Fr) * mix(vec3(1.0), vec3(0.2), metallic);
    vec3 diffuse = albedo * (ambCol + sunCol * wrap) * kd;

    // Emisivo (luces de máquinas y hornos con parpadeo orgánico)
    vec3 emis = u_emissive.xyz;
    if (u_emissive.w > 0.5) {
        emis *= toLinear(texture(u_emissive_tex, v_uv).rgb);
    }
    emis *= u_mat.z * (1.0 + night * 2.0);
    emis *= 1.0 + u_params.w * night * (0.55 * sin(u_pbr.w * 9.7) + 0.45 * sin(u_pbr.w * 23.3));

    vec3 final_color = diffuse + specular + emis;
    final_color = aces(final_color * u_cam.w);
    final_color = toSRGB(final_color);

    frag_color = vec4(final_color, u_misc.z);
}
