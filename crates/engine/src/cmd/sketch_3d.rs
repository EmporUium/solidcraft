//! 3D sketch drawing: with the sketch's 3D Sketch option on (Sketch Palette), lines, arcs,
//! splines and points may leave the sketch plane. They are kept as drawn 3D curves (wires with
//! fit points, exact arcs keeping their analytic form): drawn and snapped to, movable point by
//! point, and usable as sweep and pipe paths.

use serde_json::{Value, json};
use solidcraft_geom::Vec3;
use solidcraft_sketch::WireSpace;

use super::sketch::{edit, target_sketch};
use super::{CommandSpec, in_sketch};
use crate::params::{bad, req_vec3, vec3};
use crate::{Result, Session};

pub static COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("sketch.line3d", "3D Line", line3d).at("SKETCH", "3D").icon("line").enabled(in_sketch_3d).params(concat!(
        "points: [[x,y,z]…] world points (2 or more): one 3D line per segment, joined end to end (needs 3D Sketch on)",
        created_doc!()
    )),
    CommandSpec::new("sketch.spline3d", "3D Spline", spline3d)
        .at("SKETCH", "3D")
        .icon("spline")
        .enabled(in_sketch_3d)
        .params(concat!("points: [[x,y,z]…] world fit points (2 or more): a smooth 3D curve through them (needs 3D Sketch on)", created_doc!())),
    CommandSpec::new("sketch.blend3d", "3D Blend Curve", blend3d)
        .at("SKETCH", "3D")
        .icon("blend")
        .enabled(can_blend3d)
        .params(concat!(
            "a, b: curve end points as `w1.start` or `w1.end`, continuity?: g0 | g1 (default g1), a_tension?, b_tension?: handle length at each joint as a multiple of a third of the span (default 1, 0.01 to 100): one 3D curve joined to both, tangent to each at the joint when g1 (needs 3D Sketch on)",
            created_doc!()
        )),
    CommandSpec::new("sketch.arc3d", "3D Arc", arc3d)
        .at("SKETCH", "3D")
        .icon("arc3")
        .enabled(in_sketch_3d)
        .params(concat!(
            "start, through, end: [x,y,z] world points: an exact circular arc through them, from start through through to end (needs 3D Sketch on). Stored exactly (centre, radius, angles), so path consumers can treat it as a true arc",
            created_doc!()
        )),
    CommandSpec::new("sketch.point3d", "3D Point", point3d)
        .at("SKETCH", "3D")
        .icon("point")
        .enabled(in_sketch_3d)
        .params(concat!("point: [x,y,z] world point (needs 3D Sketch on)", created_doc!())),
    CommandSpec::new("sketch.move3d", "Move 3D Point", move3d)
        .icon("sketch_move")
        .enabled(in_sketch)
        .params("wire: drawn 3D curve or point id, index?: fit point (default 0), to: [x,y,z] | by: [dx,dy,dz]; points joined to it move too"),
];

fn points(cmd: &str, p: &Value, min: usize) -> Result<Vec<Vec3>> {
    let a = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`points` must list [x,y,z] points"))?;
    if a.len() < min || a.len() > solidcraft_sketch::MAX_FIT_POINTS {
        return Err(bad(cmd, format!("`points` needs {min} to {} points", solidcraft_sketch::MAX_FIT_POINTS)));
    }
    a.iter().map(|v| vec3(v).ok_or_else(|| bad(cmd, "a point must be [x, y, z]"))).collect()
}

fn needs_3d(sk: &solidcraft_sketch::Sketch, cmd: &str) -> Result<()> {
    if sk.view.three_d { Ok(()) } else { Err(bad(cmd, "turn on 3D Sketch (sketch.options sketch_3d: true) to draw off the sketch plane")) }
}

/// The 3D Arc button starts an interactive tool, so it is only offered while a 3D sketch is
/// being edited; otherwise the tool could only fail on its third click.
fn in_sketch_3d(s: &Session) -> std::result::Result<(), String> {
    let id = s.active_sketch.ok_or("no sketch is being edited (Create Sketch or Edit Sketch first)")?;
    match s.doc.sketch(id) {
        Ok(sk) if sk.view.three_d => Ok(()),
        Ok(_) => Err("turn on 3D Sketch (sketch.options sketch_3d: true) to draw 3D arcs".into()),
        Err(_) => Err("no sketch is being edited (Create Sketch or Edit Sketch first)".into()),
    }
}

/// Blending is offered while a 3D sketch is edited, and also when an existing blend is selected:
/// that selection is the edit, so its sketch need not be open.
fn can_blend3d(s: &Session) -> std::result::Result<(), String> {
    if in_sketch_3d(s).is_ok() {
        return Ok(());
    }
    let state = s.model.state();
    let picked = s.selection.iter().any(|sel| {
        let crate::Sel::SketchCurve { id, sketch } = sel else { return false };
        state
            .sketches
            .iter()
            .filter(|ss| sketch.is_none_or(|k| ss.feature == k))
            .any(|ss| ss.sketch.wires.iter().any(|w| &w.id == id && (w.blend.is_some() || w.bezier.is_some())))
    });
    if picked {
        Ok(())
    } else {
        Err(match in_sketch_3d(s) {
            Err(e) => format!("{e}, or select a blend curve to edit it"),
            Ok(()) => "select a blend curve to edit it".into(),
        })
    }
}

/// World command inputs belong to the target sketch's owner, which may differ from the
/// active component when a script supplies an explicit sketch.
fn inverse_owner_frame(s: &Session, p: &Value, cmd: &str) -> Result<solidcraft_doc::Mat> {
    let id = target_sketch(s, p, cmd)?;
    let owner = s.doc.feature(id).map(|f| f.component).ok_or_else(|| bad(cmd, "target sketch is missing"))?;
    if owner != 0 && owner != s.active_component {
        if s.doc.occurrences.len() > 100_000 {
            return Err(bad(cmd, "sketch owner placement expansion exceeds supported bounds"));
        }
        let mut children: std::collections::BTreeMap<u64, Vec<&solidcraft_doc::Occurrence>> = std::collections::BTreeMap::new();
        for occurrence in &s.doc.occurrences {
            children.entry(occurrence.parent).or_default().push(occurrence);
        }
        let mut stack = vec![(0, 0usize)];
        let mut count = 0;
        let mut visited = 0;
        while let Some((component, depth)) = stack.pop() {
            for o in children.get(&component).into_iter().flatten() {
                visited += 1;
                if depth >= 64 || visited > 100_000 {
                    return Err(bad(cmd, "sketch owner placement expansion exceeds supported bounds"));
                }
                if o.component == owner {
                    count += 1;
                    if count > 1 {
                        return Err(bad(cmd, "activate the desired sketch owner occurrence before editing world 3D points"));
                    }
                }
                stack.push((o.component, depth + 1));
            }
        }
    }
    let frame = if owner == s.active_component { s.active_component_frame() } else { s.placed_document().component_transform(owner) };
    solidcraft_doc::mat_inverse(&frame).ok_or_else(|| bad(cmd, "the sketch owner placement is not invertible"))
}

fn line3d(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.line3d";
    let inverse = inverse_owner_frame(s, p, cmd)?;
    let pts: Vec<Vec3> = points(cmd, p, 2)?.into_iter().map(|at| solidcraft_doc::apply_point(&inverse, at)).collect();
    let (ids, info) = edit(s, p, cmd, false, |sk, _| {
        needs_3d(sk, cmd)?;
        pts.windows(2).map(|w| sk.add_drawn_wire_in(w, WireSpace::Component).map_err(Into::into)).collect::<Result<Vec<String>>>()
    })?;
    Ok(json!({"curves": ids, "sketch": info}))
}

fn spline3d(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.spline3d";
    let inverse = inverse_owner_frame(s, p, cmd)?;
    let pts: Vec<Vec3> = points(cmd, p, 2)?.into_iter().map(|at| solidcraft_doc::apply_point(&inverse, at)).collect();
    let (id, info) = edit(s, p, cmd, false, |sk, _| {
        needs_3d(sk, cmd)?;
        Ok(sk.add_drawn_wire_in(&pts, WireSpace::Component)?)
    })?;
    Ok(json!({"curves": [id], "sketch": info}))
}

/// The joinable ends of the sketch, in component coordinates.
///
/// Args:
///     sk: The sketch being edited.
///
/// Returns:
///     One `(ref, point, direction into the curve)` per end, for drawn 3D wires and 2D curves
///     alike. `ref` is what the command takes, such as `w1.end` or `l4.start`.
fn sketch_ends(sk: &solidcraft_sketch::Sketch) -> Vec<(String, Vec3, Vec3)> {
    let mut out = Vec::with_capacity(2 * (sk.wires.len() + sk.curves.len()));
    for w in &sk.wires {
        let n = w.fit.len();
        if n < 2 {
            continue;
        }
        out.push((format!("{}.start", w.id), w.fit[0], w.fit[1] - w.fit[0]));
        out.push((format!("{}.end", w.id), w.fit[n - 1], w.fit[n - 2] - w.fit[n - 1]));
    }
    for (ci, c) in sk.curves.iter().enumerate() {
        let poly = sk.polyline(ci);
        let n = poly.len();
        if n < 2 {
            continue;
        }
        let v = |q: solidcraft_geom::Vec2| Vec3::new(q.x, q.y, 0.0);
        out.push((format!("{}.start", c.id), v(poly[0]), v(poly[1] - poly[0])));
        out.push((format!("{}.end", c.id), v(poly[n - 1]), v(poly[n - 2] - poly[n - 1])));
    }
    out
}

/// Look a named end up among [`sketch_ends`].
///
/// Args:
///     ends: The sketch's ends, as returned by [`sketch_ends`].
///     reference: An end reference such as `w1.start` or `l2.end`.
///     cmd: The command id, for error messages.
///
/// Returns:
///     The joint's position and the direction from it into the curve.
fn named_end(ends: &[(String, Vec3, Vec3)], reference: &str, cmd: &str) -> Result<(Vec3, Vec3)> {
    if !reference.contains('.') {
        return Err(bad(cmd, format!("`{reference}` must be a curve end such as `w1.end` or `l2.start`")));
    }
    ends.iter()
        .find(|(id, _, _)| id == reference)
        .map(|(_, at, into)| (*at, *into))
        .ok_or_else(|| bad(cmd, format!("no curve ends at `{reference}`")))
}

/// The edge ends of the evaluated model nearest to each of `points`, in world coordinates.
///
/// One pass over every body's edges answers every point, so picking two ends costs one walk of
/// the model rather than one per end.
///
/// Args:
///     st: The model as it stood before the sketch being edited.
///     points: World points to find the nearest edge end to.
///
/// Returns:
///     For each point, the nearest `(position, direction into the edge)`, or `None` when the
///     model has no edges.
fn nearest_model_ends(st: &solidcraft_doc::ModelState, points: &[Vec3]) -> Vec<Option<(Vec3, Vec3)>> {
    let mut best: Vec<Option<(f64, Vec3, Vec3)>> = vec![None; points.len()];
    if points.is_empty() {
        return Vec::new();
    }
    for body in &st.bodies {
        let m = body.mesh();
        for e in &m.edges {
            let n = e.len();
            if n < 2 {
                continue;
            }
            for (at, into) in [(e[0], e[1] - e[0]), (e[n - 1], e[n - 2] - e[n - 1])] {
                for (i, p) in points.iter().enumerate() {
                    let d = at.dist(*p);
                    if best[i].as_ref().is_none_or(|(bd, _, _)| d < *bd) {
                        best[i] = Some((d, at, into));
                    }
                }
            }
        }
    }
    best.into_iter().map(|b| b.map(|(_, at, into)| (at, into))).collect()
}

/// The direction of the polyline nearest to `at`, with its distance from it.
///
/// Args:
///     pts: The polyline to search, in the same space as `at`.
///     at: The point to measure from.
///
/// Returns:
///     The distance to the nearest segment and that segment's unit direction, or `None` when the
///     polyline has fewer than two points.
fn nearest_segment_dir(pts: &[Vec3], at: Vec3) -> Option<(f64, Vec3)> {
    let mut best: Option<(f64, Vec3)> = None;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let seg = b - a;
        let len2 = seg.len() * seg.len();
        if len2 < 1e-18 {
            continue;
        }
        let t = ((at - a).dot(seg) / len2).clamp(0.0, 1.0);
        let d = (at - (a + seg * t)).len();
        if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
            best = unit(seg).map(|u| (d, u)).or(Some((d, Vec3::ZERO)));
        }
    }
    best
}

/// The tangent an edge or curve supplies at one joint.
///
/// A corner can have several edges meeting it, so the reference names the chosen one rather than
/// relying on which is nearest: a sketch curve by id (`l3`), or a model edge by body and index
/// (`{body, edge}`). The direction is taken at the point of that edge nearest the joint.
///
/// Args:
///     sk: The sketch being edited.
///     st: The model as it stood before the sketch.
///     inverse: World-to-sketch transform.
///     forward: Sketch-to-world transform, for measuring model edges.
///     reference: `l3`, or `{"body": "Body1", "edge": 4}`.
///     joint: The joint position, in the sketch's coordinates.
///     cmd: The command id, for error messages.
///
/// Returns:
///     The unit direction of that edge at the joint, in sketch coordinates.
fn referenced_tangent(
    sk: &solidcraft_sketch::Sketch,
    st: &solidcraft_doc::ModelState,
    inverse: &solidcraft_doc::Mat,
    forward: &solidcraft_doc::Mat,
    reference: &Value,
    joint: Vec3,
    cmd: &str,
) -> Result<Vec3> {
    if let Some(id) = reference.as_str() {
        // A 3D drawn curve is a wire, not a 2D curve: it is measured from its own samples.
        if let Some(w) = sk.wires.iter().find(|w| w.id == id) {
            return nearest_segment_dir(&w.pts, joint)
                .and_then(|(_, dir)| unit(dir))
                .ok_or_else(|| bad(cmd, format!("`{id}` has no direction at the joint")));
        }
        let ci = sk.curve_index(id).ok_or_else(|| bad(cmd, format!("no sketch curve `{id}` to take the tangent from")))?;
        let poly: Vec<Vec3> = sk.polyline(ci).into_iter().map(|q| Vec3::new(q.x, q.y, 0.0)).collect();
        return nearest_segment_dir(&poly, joint)
            .and_then(|(_, dir)| unit(dir))
            .ok_or_else(|| bad(cmd, format!("`{id}` has no direction at the joint")));
    }
    let body = reference.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "a tangent must be a sketch curve id or {body, edge}"))?;
    let edge = reference
        .get("edge")
        .and_then(Value::as_u64)
        .and_then(|v| usize::try_from(v).ok())
        .ok_or_else(|| bad(cmd, "`edge` must be an edge index"))?;
    let m =
        st.bodies.iter().find(|b| b.name == body).map(|b| b.mesh()).ok_or_else(|| bad(cmd, format!("no body `{body}` to take the tangent from")))?;
    let e = m.edges.get(edge).ok_or_else(|| bad(cmd, format!("`{body}` has no edge {edge}")))?;
    nearest_segment_dir(e, solidcraft_doc::apply_point(forward, joint))
        .and_then(|(_, dir)| unit(solidcraft_doc::apply_vector(inverse, dir)))
        .ok_or_else(|| bad(cmd, format!("edge {edge} of `{body}` has no direction at the joint")))
}

/// Where a stored blend end joins, and the direction its picked edge supplies there.
///
/// Args:
///     sk: The sketch being edited.
///     st: The model as it stood before the sketch.
///     inverse: World-to-sketch transform.
///     forward: Sketch-to-world transform.
///     ends: The sketch's ends, as returned by [`sketch_ends`].
///     end: One stored end of a blend.
///     cmd: The command id, for error messages.
///
/// Returns:
///     The joint position and the tangent direction, both in the sketch's coordinates.
fn stored_end(
    sk: &solidcraft_sketch::Sketch,
    st: &solidcraft_doc::ModelState,
    inverse: &solidcraft_doc::Mat,
    forward: &solidcraft_doc::Mat,
    ends: &[(String, Vec3, Vec3)],
    end: &solidcraft_sketch::BlendEnd,
    cmd: &str,
) -> Result<(Vec3, Vec3)> {
    use solidcraft_sketch::{BlendPlace, BlendTangent};
    let at = match &end.at {
        BlendPlace::Curve(r) => named_end(ends, r, cmd)?.0,
        BlendPlace::Point(id) => {
            let i = sk.point_index(id).ok_or_else(|| bad(cmd, format!("no sketch point `{id}`")))?;
            let q = sk.point(i).ok_or_else(|| bad(cmd, format!("no sketch point `{id}`")))?;
            Vec3::new(q.x, q.y, 0.0)
        }
        BlendPlace::Vertex { point, .. } => *point,
        BlendPlace::World(v) => *v,
    };
    let reference = match &end.tangent {
        BlendTangent::Curve(id) => Value::String(id.clone()),
        BlendTangent::Edge { body, edge } => json!({"body": body, "edge": edge}),
        BlendTangent::Handle(dir) => return Ok((at, *dir)),
    };
    let dir = referenced_tangent(sk, st, inverse, forward, &reference, at, cmd)?;
    Ok((at, dir))
}

/// Unit vector, or `None` when `v` is too short to have a direction.
fn unit(v: Vec3) -> Option<Vec3> {
    let l = v.len();
    (l > 1e-12).then(|| v * (1.0 / l))
}

/// Parse one joint parameter, which the dialog sends as `{"at": [x,y,z]}` or `{"point": "p3"}`
/// and a script may send as a bare world point.
fn joint_param(p: &Value, k: &str, cmd: &str) -> Result<JointInput> {
    let v = p.get(k).ok_or_else(|| bad(cmd, format!("`{k}` is missing: give a point or vertex")))?;
    if let Some(at) = vec3(v) {
        return Ok(JointInput::At(at, None));
    }
    if let Some(id) = v.get("point").and_then(Value::as_str) {
        return Ok(JointInput::Point(id.to_string()));
    }
    v.get("at")
        .and_then(vec3)
        .map(|at| JointInput::At(at, v.get("body").and_then(Value::as_str).map(str::to_string)))
        .ok_or_else(|| bad(cmd, format!("`{k}` must be [x,y,z], {{at: [x,y,z]}} or {{point: id}}")))
}

/// A joint as the caller gave it: either a place or the id of a sketch point.
enum JointInput {
    At(Vec3, Option<String>),
    Point(String),
}

/// Join two curve or model-edge ends with one 3D curve, tangent to each at its joint.
///
/// Each end is either named (`a` / `b`, such as `w1.end`) or given as the world point the user
/// clicked (`a_at` / `b_at`), in which case the nearest sketch curve end or model edge end is
/// used. A tension above or below 1 lengthens or shortens that joint's handle.
///
/// Args:
///     s: The session to edit.
///     p: Command parameters — `a`, `b` or `a_at`, `b_at`, plus optional `continuity`,
///         `a_tension` and `b_tension`.
///
/// Returns:
///     `{curves: [id], sketch: …}` with the blend's id.
fn blend3d(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.blend3d";
    // Editing an existing blend: its joints and tangents are already recorded, so only the
    // tension changes are read from the parameters.
    let edit_id = p.get("edit").and_then(Value::as_str).map(str::to_string);
    // Each joint may run its tangent the other way.
    // Each joint may run its tangent the other way. Left unset, an edit keeps what was recorded.
    let (fa, fb) = (p.get("a_flip").and_then(Value::as_bool), p.get("b_flip").and_then(Value::as_bool));
    let cont = p.get("continuity").and_then(Value::as_str).unwrap_or("g1").to_ascii_lowercase();
    if !matches!(cont.as_str(), "g0" | "g1") {
        return Err(bad(cmd, format!("continuity `{cont}` is not supported: a drawn 3D curve stores no curvature, so only g0 and g1 can be met")));
    }
    // Handle length at each joint, as a multiple of the third of the span. 1 is the neutral blend.
    let tension = |k: &str| -> Result<f64> {
        let t = p.get(k).and_then(Value::as_f64).unwrap_or(1.0);
        if !(0.01..=100.0).contains(&t) {
            return Err(bad(cmd, format!("`{k}` must be between 0.01 and 100")));
        }
        Ok(t)
    };
    let (ta, tb) = (tension("a_tension")?, tension("b_tension")?);
    // Either name the two ends, or give the two world points the user clicked.
    let ra = p.get("a").and_then(Value::as_str).map(str::to_string);
    let rb = p.get("b").and_then(Value::as_str).map(str::to_string);
    let pa_at = if ra.is_none() && p.get("a_at").is_some() { Some(joint_param(p, "a_at", cmd)?) } else { None };
    let pb_at = if rb.is_none() && p.get("b_at").is_some() { Some(joint_param(p, "b_at", cmd)?) } else { None };
    // The tangent at each joint is its own pick — the specific edge the user chose, since a
    // corner can have several. Without one the curve at the joint supplies its own direction.
    let ta_ref = p.get("a_tangent").filter(|v| !v.is_null()).cloned();
    let tb_ref = p.get("b_tangent").filter(|v| !v.is_null()).cloned();
    let inverse = inverse_owner_frame(s, p, cmd)?;
    let forward = solidcraft_doc::mat_inverse(&inverse).unwrap_or_default();
    // The model as it stands before this sketch: the edges the user picked on a solid. Both
    // clicks are answered from a single walk of it, outside the edit.
    let state = s.model.state_before(target_sketch(s, p, cmd)?);
    let clicks: Vec<Vec3> = [&pa_at, &pb_at]
        .into_iter()
        .filter_map(|j| match j {
            Some(JointInput::At(at, _)) => Some(*at),
            _ => None,
        })
        .collect();
    let model_hits = nearest_model_ends(&state, &clicks);
    let model_of = |at: &Option<JointInput>| -> Option<(Vec3, Vec3)> {
        let at = match at {
            Some(JointInput::At(at, _)) => *at,
            _ => return None,
        };
        let i = clicks.iter().position(|c| c.dist(at) < 1e-12)?;
        model_hits.get(i).copied().flatten()
    };
    let (ma, mb) = (model_of(&pa_at), model_of(&pb_at));
    let (id, info) = edit(s, p, cmd, false, |sk, _| {
        needs_3d(sk, cmd)?;
        let ends = sketch_ends(sk);
        // A click may land on a sketch curve or on an edge of the model. Whichever end is nearer
        // to the click wins; both are compared in the sketch's own coordinates.
        let pick = |at: Vec3, model: Option<(Vec3, Vec3)>| -> Result<(Vec3, Vec3)> {
            let comp = solidcraft_doc::apply_point(&inverse, at);
            let sketch_hit = ends.iter().map(|(_, at, into)| (at.dist(comp), *at, *into)).min_by(|a, b| a.0.total_cmp(&b.0));
            let model_hit = model.map(|(at, into)| {
                let (cq, cd) = (solidcraft_doc::apply_point(&inverse, at), solidcraft_doc::apply_vector(&inverse, into));
                (cq.dist(comp), cq, cd)
            });
            match (sketch_hit, model_hit) {
                (Some(s), Some(m)) if m.0 < s.0 => Ok((m.1, m.2)),
                (Some(s), _) => Ok((s.1, s.2)),
                (None, Some(m)) => Ok((m.1, m.2)),
                (None, None) => Err(bad(cmd, "nothing to blend here: the sketch has no curves and the model no edges")),
            }
        };
        let resolve = |r: &Option<String>, at: &Option<JointInput>, model: Option<(Vec3, Vec3)>| -> Result<(Vec3, Vec3)> {
            match (r, at) {
                (Some(r), _) => named_end(&ends, r, cmd),
                (None, Some(JointInput::Point(id))) => {
                    let i = sk.point_index(id).ok_or_else(|| bad(cmd, format!("no sketch point `{id}`")))?;
                    let q = sk.point(i).ok_or_else(|| bad(cmd, format!("no sketch point `{id}`")))?;
                    // A named point carries no direction of its own; its tangent is the pick.
                    Ok((Vec3::new(q.x, q.y, 0.0), Vec3::ZERO))
                }
                (None, Some(JointInput::At(at, _))) => pick(*at, model),
                (None, None) => Err(bad(cmd, "give `a` and `b`, or `a_at` and `b_at`")),
            }
        };
        use solidcraft_sketch::{Blend, BlendEnd, BlendPlace, BlendTangent};
        let place = |r: &Option<String>, at: &Option<JointInput>, p: Vec3| match (r, at) {
            (Some(r), _) => BlendPlace::Curve(r.clone()),
            (None, Some(JointInput::Point(id))) => BlendPlace::Point(id.clone()),
            (None, Some(JointInput::At(_, Some(body)))) => BlendPlace::Vertex { body: body.clone(), point: p },
            _ => BlendPlace::World(p),
        };
        // A joint named by a curve end needs no tangent pick: that curve is its tangent.
        let tangent = |r: &Option<Value>, place: &BlendPlace| -> Result<BlendTangent> {
            match r {
                Some(Value::String(id)) => Ok(BlendTangent::Curve(id.clone())),
                Some(v) => {
                    let body =
                        v.get("body").and_then(Value::as_str).ok_or_else(|| bad(cmd, "a tangent must be a sketch curve id or {body, edge}"))?;
                    let edge = v
                        .get("edge")
                        .and_then(Value::as_u64)
                        .and_then(|e| usize::try_from(e).ok())
                        .ok_or_else(|| bad(cmd, "`edge` must be an edge index"))?;
                    Ok(BlendTangent::Edge { body: body.to_string(), edge })
                }
                None => match place {
                    BlendPlace::Curve(reference) => Ok(BlendTangent::Curve(reference.split('.').next().unwrap_or(reference).to_string())),
                    _ => Err(bad(cmd, "a joint given by a point needs the edge that sets its tangent")),
                },
            }
        };
        // The joints come from this run's picks when it has any; an edit that only changes the
        // tensions and directions reuses the joints and tangents the curve recorded.
        let picked = pa_at.is_some() || pb_at.is_some();
        let (spec, pa, pb, dir_a, dir_b) = if picked || edit_id.is_none() {
            let (pa, in_a) = resolve(&ra, &pa_at, ma)?;
            let (pb, in_b) = resolve(&rb, &pb_at, mb)?;
            let (at_a, at_b) = (place(&ra, &pa_at, pa), place(&rb, &pb_at, pb));
            let (tan_a, tan_b) = (tangent(&ta_ref, &at_a)?, tangent(&tb_ref, &at_b)?);
            let dir = |r: &Option<Value>, at: Vec3, fallback: Vec3| -> Result<Vec3> {
                match r {
                    Some(r) => referenced_tangent(sk, &state, &inverse, &forward, r, at, cmd),
                    None => unit(fallback).ok_or_else(|| bad(cmd, "the joint has no tangent: pick an edge or curve for it")),
                }
            };
            let dir_a = dir(&ta_ref, pa, in_a * -1.0)?;
            let dir_b = dir(&tb_ref, pb, in_b)?;
            let spec = Blend {
                a: BlendEnd { at: at_a, tangent: tan_a, tension: ta, flip: fa.unwrap_or(false) },
                b: BlendEnd { at: at_b, tangent: tan_b, tension: tb, flip: fb.unwrap_or(false) },
                continuity: if cont == "g0" { 0 } else { 1 },
            };
            (spec, pa, pb, dir_a, dir_b)
        } else {
            let id = edit_id.clone().unwrap_or_default();
            let stored = sk
                .wire_index(&id)
                .and_then(|i| sk.wires.get(i))
                .map(|w| (w.blend.clone(), w.bezier.clone()))
                .ok_or_else(|| bad(cmd, format!("no curve `{id}`")))?;
            // A blend made before its inputs were recorded still has its control polygon: its
            // handles give the joints and directions, so it stays editable.
            let spec = match stored {
                (Some(b), _) => b,
                (None, Some(control)) => {
                    let n = control.len();
                    if n < 2 {
                        return Err(bad(cmd, format!("`{id}` is not a blend curve")));
                    }
                    let end =
                        |at: Vec3, dir: Vec3| BlendEnd { at: BlendPlace::World(at), tangent: BlendTangent::Handle(dir), tension: 1.0, flip: false };
                    let (a, b) = (control[0], control[n - 1]);
                    Blend {
                        a: end(a, unit(control[1] - a).unwrap_or(Vec3::ZERO)),
                        b: end(b, unit(control[n - 2] - b).unwrap_or(Vec3::ZERO)),
                        continuity: if n == 2 { 0 } else { 1 },
                    }
                }
                (None, None) => return Err(bad(cmd, format!("`{id}` is not a blend curve"))),
            };
            let spec = Blend {
                a: BlendEnd { tension: ta, flip: fa.unwrap_or(spec.a.flip), ..spec.a },
                b: BlendEnd { tension: tb, flip: fb.unwrap_or(spec.b.flip), ..spec.b },
                continuity: spec.continuity,
            };
            let (pa, dir_a) = stored_end(sk, &state, &inverse, &forward, &ends, &spec.a, cmd)?;
            let (pb, dir_b) = stored_end(sk, &state, &inverse, &forward, &ends, &spec.b, cmd)?;
            (spec, pa, pb, dir_a, dir_b)
        };
        let l = pa.dist(pb);
        if l < 1e-9 {
            return Err(bad(cmd, "the two ends must differ"));
        }
        let control: Vec<Vec3> = if spec.continuity == 0 {
            vec![pa, pb]
        } else {
            // A picked tangent wins; otherwise the curve runs on out of `a` and into `b`. Either
            // way it is oriented to leave one joint and reach the other, then scaled by the span,
            // the joint's tension, and the direction its toggle asks for.
            let span = pb - pa;
            let orient = |d: Vec3| if d.dot(span) < 0.0 { d * -1.0 } else { d };
            let turn = |d: Vec3, flip: bool| if flip { d * -1.0 } else { d };
            let (d0, d1) =
                (turn(orient(dir_a), spec.a.flip) * (l / 3.0 * spec.a.tension), turn(orient(dir_b), spec.b.flip) * (l / 3.0 * spec.b.tension));
            solidcraft_geom::hermite_bezier3(pa, pb, &[d0], &[d1])
                .ok_or_else(|| bad(cmd, "the blend curve could not be built from the tangents at both ends"))?
        };
        match &edit_id {
            Some(id) => {
                sk.set_blend(id, &control, spec)?;
                Ok(id.clone())
            }
            None => {
                let id = sk.add_drawn_bezier_in(&control, WireSpace::Component)?;
                sk.set_blend(&id, &control, spec)?;
                Ok(id)
            }
        }
    })?;
    Ok(json!({ "curves": [id], "sketch": info }))
}

fn arc3d(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.arc3d";
    let inverse = inverse_owner_frame(s, p, cmd)?;
    let pts: Vec<Vec3> = ["start", "through", "end"]
        .iter()
        .map(|k| req_vec3(cmd, p, k))
        .collect::<Result<Vec<Vec3>>>()?
        .into_iter()
        .map(|at| solidcraft_doc::apply_point(&inverse, at))
        .collect();
    let (id, info) = edit(s, p, cmd, false, |sk, _| {
        needs_3d(sk, cmd)?;
        Ok(sk.add_drawn_arc_in(&pts, WireSpace::Component)?)
    })?;
    Ok(json!({"curves": [id], "sketch": info}))
}

fn point3d(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.point3d";
    let inverse = inverse_owner_frame(s, p, cmd)?;
    let at = solidcraft_doc::apply_point(&inverse, req_vec3(cmd, p, "point")?);
    let (id, info) = edit(s, p, cmd, false, |sk, _| {
        needs_3d(sk, cmd)?;
        Ok(sk.add_drawn_wire_in(&[at], WireSpace::Component)?)
    })?;
    Ok(json!({"point": id, "sketch": info}))
}

fn move3d(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "sketch.move3d";
    let wire = p.get("wire").and_then(Value::as_str).ok_or_else(|| bad(cmd, "`wire` must be a 3D curve or point id"))?.to_string();
    let index = match p.get("index") {
        None => 0,
        Some(v) => v
            .as_u64()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|v| *v < solidcraft_sketch::MAX_FIT_POINTS)
            .ok_or_else(|| bad(cmd, "`index` must be a fit point index below 500"))?,
    };
    let parse = |name| p.get(name).map(|v| vec3(v).ok_or_else(|| bad(cmd, format!("`{name}` must be [x,y,z]")))).transpose();
    let to = parse("to")?;
    let by = parse("by")?;
    let inverse = inverse_owner_frame(s, p, cmd)?;
    let (n, info) = edit(s, p, cmd, false, |sk, _| {
        let drawn = sk
            .wire_index(&wire)
            .and_then(|w| sk.wires.get(w))
            .filter(|w| !w.fit.is_empty() && w.link.is_none())
            .ok_or_else(|| bad(cmd, format!("`{wire}` is not an editable drawn 3D curve")))?;
        let from = drawn.fit.get(index).copied().ok_or_else(|| bad(cmd, format!("`{wire}` has no drawn point {index}")))?;
        let to = match (to, by) {
            (Some(t), _) => {
                if drawn.space == WireSpace::Component {
                    solidcraft_doc::apply_point(&inverse, t)
                } else {
                    t
                }
            }
            (None, Some(d)) => from + if drawn.space == WireSpace::Component { solidcraft_doc::apply_vector(&inverse, d) } else { d },
            _ => return Err(bad(cmd, "give `to` or `by`")),
        };
        Ok(sk.move_drawn_point(&wire, index, to)?)
    })?;
    Ok(json!({"moved": n, "sketch": info}))
}

#[cfg(test)]
#[path = "sketch_3d_frame_tests.rs"]
mod frame_tests;
