//! Graphic properties: colour, lineweight, and the ByLayer/ByBlock indirection.

use crate::id::ObjectId;
use serde::{Deserialize, Serialize};

/// A colour, kept in the three forms drawings actually use.
///
/// The 256-entry index palette is not legacy trivia to be normalised away: pen
/// tables, plot style tables and decades of layer standards are written in terms
/// of colour numbers, and collapsing them to RGB on import loses the plotting
/// intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum Color {
    #[default]
    ByLayer,
    ByBlock,
    /// AutoCAD Color Index, 1..=255.
    Index(u8),
    Rgb {
        r: u8,
        g: u8,
        b: u8,
    },
}

impl Color {
    pub const RED: Self = Self::Index(1);
    pub const YELLOW: Self = Self::Index(2);
    pub const GREEN: Self = Self::Index(3);
    pub const CYAN: Self = Self::Index(4);
    pub const BLUE: Self = Self::Index(5);
    pub const MAGENTA: Self = Self::Index(6);
    /// Index 7 renders black on white paper and white on a dark canvas.
    pub const FOREGROUND: Self = Self::Index(7);

    #[must_use]
    pub fn is_resolved(self) -> bool {
        matches!(self, Color::Index(_) | Color::Rgb { .. })
    }
}

/// Plotted line width in hundredths of a millimetre, matching the DWG/DXF
/// convention so the value survives a round trip exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", content = "v", rename_all = "snake_case")]
pub enum LineWeight {
    #[default]
    ByLayer,
    ByBlock,
    /// Whatever the output device defaults to.
    Default,
    Hundredths(u16),
}

impl LineWeight {
    #[must_use]
    pub fn millimetres(self) -> Option<f64> {
        match self {
            LineWeight::Hundredths(h) => Some(f64::from(h) / 100.0),
            _ => None,
        }
    }
}

/// How an entity is drawn, before ByLayer/ByBlock resolution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphicStyle {
    pub color: Color,
    pub lineweight: LineWeight,
    /// `None` means ByLayer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linetype: Option<ObjectId>,
    /// Multiplier applied to the linetype pattern for this entity.
    pub linetype_scale: f64,
    /// 0 = opaque, 90 = nearly invisible, matching the DXF transparency range.
    pub transparency: u8,
}

impl Default for GraphicStyle {
    fn default() -> Self {
        Self {
            color: Color::ByLayer,
            lineweight: LineWeight::ByLayer,
            linetype: None,
            linetype_scale: 1.0,
            transparency: 0,
        }
    }
}

/// The style actually used for drawing, once indirection is gone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedStyle {
    pub color: Color,
    pub lineweight: LineWeight,
    pub transparency: u8,
}

impl GraphicStyle {
    /// Resolves against the owning layer and, when nested, the containing block
    /// reference. ByBlock outside a block reference falls back to the layer,
    /// which is what every CAD system does and what importers expect.
    #[must_use]
    pub fn resolve(&self, layer: &ResolvedStyle, block: Option<&ResolvedStyle>) -> ResolvedStyle {
        let color = match self.color {
            Color::ByLayer => layer.color,
            Color::ByBlock => block.map_or(layer.color, |b| b.color),
            other => other,
        };
        let lineweight = match self.lineweight {
            LineWeight::ByLayer => layer.lineweight,
            LineWeight::ByBlock => block.map_or(layer.lineweight, |b| b.lineweight),
            other => other,
        };
        ResolvedStyle {
            color,
            lineweight,
            transparency: self.transparency,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer() -> ResolvedStyle {
        ResolvedStyle {
            color: Color::Index(3),
            lineweight: LineWeight::Hundredths(25),
            transparency: 0,
        }
    }

    fn block() -> ResolvedStyle {
        ResolvedStyle {
            color: Color::Index(5),
            lineweight: LineWeight::Hundredths(50),
            transparency: 0,
        }
    }

    #[test]
    fn bylayer_takes_the_layer_style() {
        let r = GraphicStyle::default().resolve(&layer(), Some(&block()));
        assert_eq!(r.color, Color::Index(3));
        assert_eq!(r.lineweight, LineWeight::Hundredths(25));
    }

    #[test]
    fn byblock_takes_the_block_style_when_nested() {
        let s = GraphicStyle {
            color: Color::ByBlock,
            lineweight: LineWeight::ByBlock,
            ..Default::default()
        };
        let r = s.resolve(&layer(), Some(&block()));
        assert_eq!(r.color, Color::Index(5));
        assert_eq!(r.lineweight, LineWeight::Hundredths(50));
    }

    #[test]
    fn byblock_outside_a_block_falls_back_to_the_layer() {
        let s = GraphicStyle {
            color: Color::ByBlock,
            ..Default::default()
        };
        assert_eq!(s.resolve(&layer(), None).color, Color::Index(3));
    }

    #[test]
    fn an_explicit_colour_wins_over_everything() {
        let s = GraphicStyle {
            color: Color::Rgb { r: 255, g: 0, b: 0 },
            ..Default::default()
        };
        assert_eq!(
            s.resolve(&layer(), Some(&block())).color,
            Color::Rgb { r: 255, g: 0, b: 0 }
        );
    }

    #[test]
    fn lineweight_converts_to_millimetres() {
        assert_eq!(LineWeight::Hundredths(35).millimetres(), Some(0.35));
        assert_eq!(LineWeight::ByLayer.millimetres(), None);
    }
}
