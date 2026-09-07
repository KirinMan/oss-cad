//! The AutoCAD Colour Index.
//!
//! Colour numbers are not decoration in a drawing — they are how pen tables,
//! plot styles and thirty years of office layer standards say "this is a
//! centreline" or "this plots at 0.13 mm". Rendering them approximately makes
//! a drawing that looks wrong to the person who set the standard, so the
//! mapping matters.
//!
//! Indices 1–9 and 250–255 are fixed and listed. 10–249 are regular: 24 hues,
//! each in 10 shades, which is cheaper and less error-prone to compute than to
//! transcribe 240 triples.

/// One ACI entry as sRGB.
#[must_use]
pub fn aci_to_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        // 0 is ByBlock and 256 is ByLayer; neither reaches here once resolved,
        // but a malformed file can still carry 0.
        0 => (255, 255, 255),
        1 => (255, 0, 0),
        2 => (255, 255, 0),
        3 => (0, 255, 0),
        4 => (0, 255, 255),
        5 => (0, 0, 255),
        6 => (255, 0, 255),
        // 7 is the foreground colour: black on paper, white on a dark canvas.
        // Callers resolve it against the background; this is the paper answer.
        7 => (0, 0, 0),
        8 => (128, 128, 128),
        9 => (192, 192, 192),
        250 => (51, 51, 51),
        251 => (91, 91, 91),
        252 => (132, 132, 132),
        253 => (173, 173, 173),
        254 => (214, 214, 214),
        255 => (255, 255, 255),
        i => wheel(i),
    }
}

/// Indices 10–249: hue steps of 15°, ten shades each.
fn wheel(index: u8) -> (u8, u8, u8) {
    let offset = u32::from(index) - 10;
    let hue = f64::from(offset / 10) * 15.0;
    let shade = offset % 10;

    // Even shades are fully saturated; odd ones are the washed-out pair. Value
    // steps down through the five levels.
    let saturation = if shade % 2 == 0 { 1.0 } else { 0.65 };
    let value = match shade / 2 {
        0 => 1.0,
        1 => 0.76,
        2 => 0.61,
        3 => 0.46,
        _ => 0.30,
    };
    hsv_to_rgb(hue, saturation, value)
}

fn hsv_to_rgb(h: f64, s: f64, v: f64) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    // The wheel only ever calls this with h in [0, 360), so the sextant is
    // always 0..=5; anything else falls back to the last sextant rather than
    // panicking on an out-of-range hue.
    let sextant = (h / 60.0).floor();
    let (r, g, b) = if sextant < 1.0 {
        (c, x, 0.0)
    } else if sextant < 2.0 {
        (x, c, 0.0)
    } else if sextant < 3.0 {
        (0.0, c, x)
    } else if sextant < 4.0 {
        (0.0, x, c)
    } else if sextant < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (to_byte(r + m), to_byte(g + m), to_byte(b + m))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "clamped to 0..=255 before the cast"
)]
fn to_byte(v: f64) -> u8 {
    (v * 255.0).round().clamp(0.0, 255.0) as u8
}

/// `#rrggbb`.
#[must_use]
pub fn hex(rgb: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", rgb.0, rgb.1, rgb.2)
}

/// Perceived lightness, 0–1. Used to decide whether a colour will vanish into
/// the background — colour 7 is meant to contrast with the paper, and a drawing
/// rendered on a dark ground needs it flipped.
#[must_use]
pub fn luminance(rgb: (u8, u8, u8)) -> f64 {
    (0.2126 * f64::from(rgb.0) + 0.7152 * f64::from(rgb.1) + 0.0722 * f64::from(rgb.2)) / 255.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fixed_colours_are_the_ones_drawings_expect() {
        assert_eq!(aci_to_rgb(1), (255, 0, 0), "1 is red");
        assert_eq!(aci_to_rgb(3), (0, 255, 0), "3 is green");
        assert_eq!(aci_to_rgb(5), (0, 0, 255), "5 is blue");
        assert_eq!(aci_to_rgb(7), (0, 0, 0), "7 plots black on paper");
    }

    #[test]
    fn the_wheel_covers_every_remaining_index() {
        for i in 10..=249u8 {
            let (r, g, b) = aci_to_rgb(i);
            // Nothing in the wheel is pure white or pure black; those live in
            // the fixed range.
            assert!(
                !(r == 255 && g == 255 && b == 255),
                "index {i} should not be white"
            );
        }
    }

    #[test]
    fn a_hue_group_holds_its_hue_across_shades() {
        // 10..19 are all the same hue at different shades, so all ten should be
        // red-dominant.
        for i in 10..20u8 {
            let (r, g, b) = aci_to_rgb(i);
            assert!(
                r >= g && r >= b,
                "index {i} = ({r},{g},{b}) should be reddish"
            );
        }
    }

    #[test]
    fn shades_get_darker_within_a_group() {
        let bright = luminance(aci_to_rgb(10));
        let dark = luminance(aci_to_rgb(18));
        assert!(dark < bright, "later shades are darker");
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex((0, 0, 0)), "#000000");
        assert_eq!(hex((255, 170, 15)), "#ffaa0f");
    }

    #[test]
    fn luminance_orders_greys() {
        assert!(luminance((0, 0, 0)) < luminance((128, 128, 128)));
        assert!(luminance((128, 128, 128)) < luminance((255, 255, 255)));
        // Green reads brighter than blue at the same value, as the eye sees it.
        assert!(luminance((0, 255, 0)) > luminance((0, 0, 255)));
    }
}
