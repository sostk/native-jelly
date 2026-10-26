// THE PLAIN TEXTURE — the sampled texel times the tint, nothing else. `gfx::draw_tex_core_in` routes
// a draw here when `fs_img.frag` would leave every fragment of it through its `u_card.z < 0.5` exit:
// a square-cornered picture (the Home hero's photographs, a cached layer's composite) and the
// INTERIOR piece of a card drawn in pieces.
//
// A small program avoids reserving registers for the general card shader's unused paths.
// Device performance remains to be measured.
//
// The body is `fs_img.frag`'s plain exit, statement for statement and with the same precision
// qualifiers (the unit test `the_plain_texture_program_is_the_card_shaders_plain_exit` pins both),
// so the picture is the one the image program wrote. The vertex half is `vs_img.vert`, so `u_trect`,
// `u_tscreen` and `u_uvrect` mean what they mean for every other image draw; `v_p` is written there
// and never read here.
precision mediump float;
varying highp vec2 v_cuv;
uniform sampler2D u_tex;
uniform vec4 u_tint;
void main(){
  vec4 c = texture2D(u_tex, v_cuv);
  vec3 tex = c.rgb*u_tint.rgb;
  float ta = c.a*u_tint.a;
  gl_FragColor = vec4(tex, ta);
}
