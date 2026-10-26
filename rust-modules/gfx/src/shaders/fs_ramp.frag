// THE TWO-STOP RAMP — a square, unfocused rect whose top and bottom colours differ. `gfx::draw_rect`
// routes it here instead of through `fs_src.frag`'s flat fast path (`if (u_radius < 0.5 && u_radR <
// 0.5 && u_focus < 0.001) { gl_FragColor = mix(u_colTop, u_colBot, vy); }`), and the expression is
// that path's own, character for character, so the pixels are the same ones.
//
// A small program avoids reserving registers for the general shader's unused SDF paths.
// UNDITHERED, exactly like the flat exit it replaces. Device performance remains to be measured.
//
// PRECISION mirrors `fs_src.frag` line for line: a highp `v_uv` (the vertex shader's own), the
// colours at the file default (mediump), and `vy` a deliberate mediump copy so the mix stays fp16.
//
// The vertex half is `vs_src.vert`, so `u_rect`/`u_screen` mean what they mean everywhere else.
precision mediump float;
varying highp vec2 v_uv;
uniform vec4 u_colTop;
uniform vec4 u_colBot;
void main(){
  float vy = v_uv.y;
  gl_FragColor = mix(u_colTop, u_colBot, vy);
}
