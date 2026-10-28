//! 3D blend curves keep their Bézier control polygon.
//!
//! A drawn 3D curve (`sketch.wires[].bezier`) may now carry the control polygon of a blend
//! curve. Older designs have no `bezier` field and read back as before: their wires stay fit
//! point curves, an exact arc, or a linked projection, and nothing about them changes.
//!
//! The field only appears when a blend is made, so a design that never used `sketch.blend3d`
//! is written exactly as it was. There is no rewrite step: the absence of the field is the
//! older meaning, and reading is already correct without one.
//!
//! A release that predates this change refuses the newer format number rather than reading a
//! blend as a fit-point curve, so a blend is never silently flattened by an older build.
