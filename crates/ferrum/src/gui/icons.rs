/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Iconos de la interfaz, dibujados como vectores: nítidos a cualquier escala
//! y con el mismo grosor de trazo, sin depender de fuentes de emojis.

use std::f32::consts::{FRAC_1_SQRT_2, PI, TAU};

use egui::epaint::{CubicBezierShape, PathShape, PathStroke};
use egui::{Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Back,
    Forward,
    Reload,
    Home,
    Close,
    Plus,
    Lock,
    Warning,
    Shield,
    Globe,
    Search,
    Minimize,
    Maximize,
    Restore,
    Copy,
    Cut,
    Paste,
    Link,
    Image,
    OpenNew,
    SelectAll,
}

/// Dibuja `icon` centrado en `rect`. Las coordenadas internas van de -1 a 1.
pub fn paint(painter: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let center = rect.center();
    let half = rect.width().min(rect.height()) / 2.0;
    let p = |x: f32, y: f32| center + vec2(x * half, y * half);
    let width = (half * 0.2).clamp(1.2, 2.2);
    let stroke = Stroke::new(width, color);
    let line = |points: Vec<Pos2>| {
        painter.add(PathShape::line(points, PathStroke::new(width, color)));
    };

    match icon {
        Icon::Back | Icon::Forward => {
            let d = if icon == Icon::Back { 1.0 } else { -1.0 };
            line(vec![p(0.75 * d, 0.0), p(-0.7 * d, 0.0)]);
            line(vec![
                p(-0.05 * d, -0.65),
                p(-0.72 * d, 0.0),
                p(-0.05 * d, 0.65),
            ]);
        },
        Icon::Reload => {
            // Arco de 290° en sentido horario con la punta arriba a la izquierda.
            let radius = half * 0.62;
            let from = -PI / 2.0 + 0.5;
            let to = from + PI * 1.6;
            line(arc(center, radius, from, to - 0.25, 24));
            let end = center + vec2(to.cos(), to.sin()) * radius;
            let tangent = vec2(-to.sin(), to.cos());
            let normal = vec2(to.cos(), to.sin());
            let head = half * 0.58;
            painter.add(Shape::convex_polygon(
                vec![
                    end + tangent * head * 0.7,
                    end + normal * head * 0.62 - tangent * head * 0.35,
                    end - normal * head * 0.62 - tangent * head * 0.35,
                ],
                color,
                Stroke::NONE,
            ));
        },
        Icon::Home => {
            line(vec![p(-0.85, -0.02), p(0.0, -0.78), p(0.85, -0.02)]);
            line(vec![
                p(-0.15, 0.78),
                p(-0.15, 0.3),
                p(0.15, 0.3),
                p(0.15, 0.78),
            ]);
            line(vec![p(-0.15, 0.78), p(-0.58, 0.78), p(-0.58, -0.22)]);
            line(vec![p(0.15, 0.78), p(0.58, 0.78), p(0.58, -0.22)]);
        },
        Icon::Close => {
            line(vec![p(-0.55, -0.55), p(0.55, 0.55)]);
            line(vec![p(0.55, -0.55), p(-0.55, 0.55)]);
        },
        Icon::Plus => {
            line(vec![p(-0.7, 0.0), p(0.7, 0.0)]);
            line(vec![p(0.0, -0.7), p(0.0, 0.7)]);
        },
        Icon::Lock => {
            let shackle = arc(p(0.0, -0.2), half * 0.36, PI, TAU, 12);
            let mut path = vec![p(-0.36, 0.05)];
            path.extend(shackle);
            path.push(p(0.36, 0.05));
            line(path);
            painter.rect_filled(
                Rect::from_min_max(p(-0.62, -0.02), p(0.62, 0.8)),
                CornerRadius::same((half * 0.22) as u8),
                color,
            );
        },
        Icon::Warning => {
            painter.add(PathShape::closed_line(
                vec![p(0.0, -0.82), p(0.88, 0.72), p(-0.88, 0.72)],
                PathStroke::new(width, color),
            ));
            line(vec![p(0.0, -0.28), p(0.0, 0.22)]);
            painter.circle_filled(p(0.0, 0.47), width * 0.75, color);
        },
        Icon::Shield => {
            let top = p(0.0, -0.86);
            let outline = vec![top, p(0.72, -0.6), p(0.68, 0.05)];
            let mut points = outline.clone();
            points.extend(bezier(
                p(0.68, 0.05),
                p(0.6, 0.5),
                p(0.25, 0.75),
                p(0.0, 0.88),
            ));
            points.extend(bezier(
                p(0.0, 0.88),
                p(-0.25, 0.75),
                p(-0.6, 0.5),
                p(-0.68, 0.05),
            ));
            points.push(p(-0.72, -0.6));
            painter.add(PathShape::closed_line(
                points,
                PathStroke::new(width, color),
            ));
            line(vec![p(-0.3, 0.02), p(-0.06, 0.26), p(0.34, -0.2)]);
        },
        Icon::Globe => {
            painter.circle_stroke(center, half * 0.78, stroke);
            painter.add(Shape::ellipse_stroke(
                center,
                vec2(half * 0.34, half * 0.78),
                stroke,
            ));
            line(vec![p(-0.78, 0.0), p(0.78, 0.0)]);
        },
        Icon::Search => {
            painter.circle_stroke(p(-0.14, -0.14), half * 0.5, stroke);
            line(vec![p(0.24, 0.24), p(0.72, 0.72)]);
        },
        Icon::Minimize => {
            let thin = Stroke::new(1.0, color);
            painter.line_segment([p(-1.0, 0.0), p(1.0, 0.0)], thin);
        },
        Icon::Maximize => {
            let thin = Stroke::new(1.0, color);
            painter.rect_stroke(
                Rect::from_min_max(p(-0.9, -0.9), p(0.9, 0.9)),
                CornerRadius::same(2),
                thin,
                egui::StrokeKind::Middle,
            );
        },
        Icon::Restore => {
            let thin = Stroke::new(1.0, color);
            painter.rect_stroke(
                Rect::from_min_max(p(-0.9, -0.5), p(0.5, 0.9)),
                CornerRadius::same(2),
                thin,
                egui::StrokeKind::Middle,
            );
            line_thin(
                painter,
                vec![p(-0.5, -0.9), p(0.9, -0.9), p(0.9, 0.5)],
                color,
            );
        },
        Icon::Copy => {
            painter.rect_stroke(
                Rect::from_min_max(p(-0.3, -0.38), p(0.72, 0.8)),
                CornerRadius::same((half * 0.2) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            line(vec![p(-0.72, 0.38), p(-0.72, -0.8), p(0.32, -0.8)]);
        },
        Icon::Cut => {
            painter.circle_stroke(p(-0.42, 0.56), half * 0.25, stroke);
            painter.circle_stroke(p(0.42, 0.56), half * 0.25, stroke);
            line(vec![p(-0.28, 0.36), p(0.42, -0.82)]);
            line(vec![p(0.28, 0.36), p(-0.42, -0.82)]);
        },
        Icon::Paste => {
            painter.rect_stroke(
                Rect::from_min_max(p(-0.66, -0.62), p(0.66, 0.84)),
                CornerRadius::same((half * 0.18) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.rect_filled(
                Rect::from_min_max(p(-0.32, -0.86), p(0.32, -0.44)),
                CornerRadius::same((half * 0.1) as u8),
                color,
            );
            line(vec![p(-0.32, 0.05), p(0.32, 0.05)]);
            line(vec![p(-0.32, 0.42), p(0.18, 0.42)]);
        },
        Icon::Link => {
            // Dos eslabones de cadena en diagonal.
            let along = vec2(FRAC_1_SQRT_2, -FRAC_1_SQRT_2);
            let a = p(-0.24, 0.24);
            let b = p(0.24, -0.24);
            painter.add(PathShape::closed_line(
                capsule(a, along, half * 0.34, half * 0.22),
                PathStroke::new(width, color),
            ));
            painter.add(PathShape::closed_line(
                capsule(b, along, half * 0.34, half * 0.22),
                PathStroke::new(width, color),
            ));
        },
        Icon::Image => {
            painter.rect_stroke(
                Rect::from_min_max(p(-0.82, -0.66), p(0.82, 0.66)),
                CornerRadius::same((half * 0.16) as u8),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.circle_filled(p(0.36, -0.26), half * 0.14, color);
            line(vec![
                p(-0.82, 0.5),
                p(-0.26, -0.06),
                p(0.14, 0.34),
                p(0.38, 0.12),
                p(0.82, 0.52),
            ]);
        },
        Icon::OpenNew => {
            line(vec![
                p(0.0, -0.72),
                p(-0.78, -0.72),
                p(-0.78, 0.78),
                p(0.72, 0.78),
                p(0.72, 0.0),
            ]);
            line(vec![p(-0.12, 0.12), p(0.8, -0.8)]);
            line(vec![p(0.24, -0.8), p(0.8, -0.8), p(0.8, -0.24)]);
        },
        Icon::SelectAll => {
            for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                line(vec![
                    p(0.8 * x, 0.35 * y),
                    p(0.8 * x, 0.8 * y),
                    p(0.35 * x, 0.8 * y),
                ]);
            }
            line(vec![p(-0.36, -0.2), p(0.36, -0.2)]);
            line(vec![p(-0.36, 0.2), p(0.16, 0.2)]);
        },
    }
}

/// Contorno de una cápsula (rectángulo con extremos redondos) centrada en
/// `center`, alargada en la dirección `along` (unitaria).
fn capsule(center: Pos2, along: egui::Vec2, half_length: f32, radius: f32) -> Vec<Pos2> {
    let angle = along.y.atan2(along.x);
    let mut points = arc(
        center + along * half_length,
        radius,
        angle - PI / 2.0,
        angle + PI / 2.0,
        10,
    );
    points.extend(arc(
        center - along * half_length,
        radius,
        angle + PI / 2.0,
        angle + PI * 1.5,
        10,
    ));
    points
}

/// Indicador de carga: un arco que gira según `time` (segundos).
pub fn spinner(painter: &Painter, rect: Rect, time: f64, color: Color32) {
    let half = rect.width().min(rect.height()) / 2.0;
    let start = (time * 5.0) as f32 % TAU;
    let points = arc(rect.center(), half * 0.75, start, start + PI * 1.4, 20);
    painter.add(PathShape::line(points, PathStroke::new(half * 0.24, color)));
}

fn line_thin(painter: &Painter, points: Vec<Pos2>, color: Color32) {
    painter.add(PathShape::line(points, PathStroke::new(1.0, color)));
}

fn arc(center: Pos2, radius: f32, from: f32, to: f32, steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|i| {
            let angle = from + (to - from) * i as f32 / steps as f32;
            center + vec2(angle.cos(), angle.sin()) * radius
        })
        .collect()
}

fn bezier(a: Pos2, b: Pos2, c: Pos2, d: Pos2) -> Vec<Pos2> {
    CubicBezierShape::from_points_stroke([a, b, c, d], false, Color32::TRANSPARENT, Stroke::NONE)
        .flatten(Some(0.1))
        .into_iter()
        .skip(1)
        .collect()
}
