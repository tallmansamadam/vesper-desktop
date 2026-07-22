//! Glossy "media-player skin" painting helpers.
//!
//! egui has no gradient fill, so panels and buttons get their brushed-chrome
//! look from a two-stop vertical gradient mesh, a translucent gloss band over
//! the top, and light/dark bevel edges. Everything here returns or paints
//! plain `Shape`s so callers can drop a panel background into a reserved paint
//! slot (behind content) or paint a button inline.

use egui::{pos2, Color32, Mesh, Painter, Pos2, Rect, Rounding, Shape, Stroke};

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A vertical two-stop gradient across `rect`, as a mesh (square corners).
pub fn vgrad(rect: Rect, top: Color32, bottom: Color32) -> Shape {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    Shape::Mesh(mesh)
}

/// A vertically-shaded filled disc (top lighter, bottom darker) as a triangle
/// fan - a round metallic bezel that leaves the corners transparent so it
/// blends into whatever is behind it, unlike the square [`vgrad`].
pub fn vgrad_circle(center: Pos2, radius: f32, top: Color32, bottom: Color32) -> Shape {
    let seg = 48;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center, lerp_color(top, bottom, 0.5));
    for i in 0..=seg {
        let a = std::f32::consts::TAU * i as f32 / seg as f32;
        let p = pos2(center.x + radius * a.cos(), center.y + radius * a.sin());
        let t = (p.y - (center.y - radius)) / (2.0 * radius); // 0 top .. 1 bottom
        mesh.colored_vertex(p, lerp_color(top, bottom, t));
    }
    for i in 1..=seg as u32 {
        mesh.add_triangle(0, i, i + 1);
    }
    Shape::Mesh(mesh)
}

/// A glossy beveled panel background bundled as one `Shape`, suitable for a
/// reserved paint slot so it renders behind the panel's content:
///
/// ```ignore
/// let slot = ui.painter().add(Shape::Noop);
/// let r = Frame::none().inner_margin(m).show(ui, |ui| { ... });
/// ui.painter().set(slot, skin::panel(r.response.rect, top, bot, edge));
/// ```
pub fn panel(rect: Rect, top: Color32, bottom: Color32, edge: Color32) -> Shape {
    let gloss = Rect::from_min_max(
        rect.min,
        pos2(rect.max.x, rect.min.y + rect.height() * 0.42),
    );
    Shape::Vec(vec![
        vgrad(rect, top, bottom),
        // sheen across the top
        vgrad(
            gloss,
            Color32::from_white_alpha(24),
            Color32::from_white_alpha(0),
        ),
        // bright top bevel, dark bottom bevel
        Shape::line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0, Color32::from_white_alpha(38)),
        ),
        Shape::line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0, Color32::from_black_alpha(130)),
        ),
        Shape::rect_stroke(rect, Rounding::same(3.0), Stroke::new(1.0, edge)),
    ])
}

/// Paint a glossy, beveled, rectangular button in `rect` (period-authentic
/// media-player buttons are square with bevels, which also sidesteps the
/// gradient mesh's square corners).
pub fn button(
    painter: &Painter,
    rect: Rect,
    top: Color32,
    bottom: Color32,
    edge: Color32,
    gloss_alpha: u8,
    glow: Option<Color32>,
) {
    painter.add(vgrad(rect, top, bottom));
    let gloss = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.center().y));
    painter.add(vgrad(
        gloss,
        Color32::from_white_alpha(gloss_alpha),
        Color32::from_white_alpha(0),
    ));
    painter.line_segment(
        [rect.left_top(), rect.right_top()],
        Stroke::new(1.0, Color32::from_white_alpha(gloss_alpha.saturating_add(24))),
    );
    painter.line_segment(
        [rect.left_bottom(), rect.right_bottom()],
        Stroke::new(1.0, Color32::from_black_alpha(140)),
    );
    if let Some(g) = glow {
        painter.rect_stroke(rect.expand(1.0), Rounding::ZERO, Stroke::new(1.0, g));
    }
    painter.rect_stroke(rect, Rounding::ZERO, Stroke::new(1.0, edge));
}
