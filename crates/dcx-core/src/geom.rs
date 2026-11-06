//! Math helpers shared by effects. Kept dependency-free on purpose.

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn clamp01(t: f32) -> f32 {
    t.clamp(0.0, 1.0)
}

pub fn smoothstep(t: f32) -> f32 {
    let t = clamp01(t);
    t * t * (3.0 - 2.0 * t)
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    let u = 1.0 - t;
    1.0 - u * u * u
}

pub fn ease_in_out_cubic(t: f32) -> f32 {
    let t = clamp01(t);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        let u = -2.0 * t + 2.0;
        1.0 - (u * u * u) / 2.0
    }
}

pub fn ease_in_quad(t: f32) -> f32 {
    let t = clamp01(t);
    t * t
}

pub fn ease_out_quad(t: f32) -> f32 {
    let t = clamp01(t);
    1.0 - (1.0 - t) * (1.0 - t)
}
