#version 320 es
// modelo_visor.frag — Iluminacion sencilla (sol + ambiente) de las maquinas.
//
// El color sale del vertex (COLOR_0 horneado en el GLB optimizado), asi que
// aqui solo hay luz: un sol direccional y un ambiente hemisferico.  El
// recorte a los bordes de la mesa es opcional (u_recorte_activo).
precision highp float;

in vec3 v_normal;
in vec2 v_uv;
in vec4 v_color;
in vec3 v_mundo;

uniform vec3 u_sol_dir;    // direccion HACIA la luz
uniform vec3 u_sol_color;  // color del sol
uniform vec3 u_amb_cielo;  // ambiente de cielo
uniform vec3 u_amb_suelo;  // rebote del suelo
uniform vec3 u_ojos;       // posicion de la camara en el espacio de la mesa
uniform float u_exposicion;

// (min_x, min_z, max_x, max_z) de la mesa fisica
uniform vec4 u_recorte;
uniform float u_recorte_activo;
// Altura de la base de la mesa: por debajo se descarta (geometria enterrada).
uniform float u_y_min;

out vec4 frag_color;

void main() {
    // 1. Recorte en los bordes fisicos de la mesa.
    if (u_recorte_activo > 0.5) {
        if (v_mundo.x < u_recorte.x || v_mundo.x > u_recorte.z ||
            v_mundo.z < u_recorte.y || v_mundo.z > u_recorte.w) {
            discard;
        }
    }
    // 2. Nada por debajo del tablero.
    if (v_mundo.y < u_y_min - 0.002) {
        discard;
    }

    vec3 N = normalize(v_normal);
    vec3 albedo = pow(max(v_color.rgb, vec3(0.0)), vec3(2.2));

    vec3 L = normalize(u_sol_dir);
    vec3 V = normalize(u_ojos - v_mundo);
    if (dot(V, V) < 1e-6) {
        V = vec3(0.0, 0.0, 1.0);
    }

    // Ambiente hemisferico.
    float hemi = N.y * 0.5 + 0.5;
    vec3 ambiente = mix(u_amb_suelo, u_amb_cielo, hemi);

    // Difusa con terminador suave.
    float wrap = clamp((dot(N, L) + 0.35) / 1.35, 0.0, 1.0);
    vec3 difusa = albedo * (ambiente + u_sol_color * wrap);

    // Especular simple (Blinn-Phong).
    vec3 H = normalize(L + V);
    float spec = pow(clamp(dot(N, H), 0.0, 1.0), 32.0) * 0.12;

    vec3 color = difusa + u_sol_color * spec;
    color = pow(clamp(color * u_exposicion, 0.0, 1.0), vec3(1.0 / 2.2));

    frag_color = vec4(color, v_color.a);
}
