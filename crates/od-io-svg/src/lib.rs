//! Rendering a drawing to SVG (F-205).
//!
//! The wgpu canvas of Phase 1 is what a drawing will eventually be *edited* in.
//! This is how it becomes visible now: SVG needs no GPU, no build step and no
//! plugin, it renders in anything, and it is a text format that can be diffed
//! and checked into a repository. For a plan at the size a person reviews, that
//! is enough — and "the drawing is on screen" is the whole point of Phase 1.
//!
//! Rendering goes through the spatial index, so a window over a large drawing
//! costs what is in the window rather than what is in the file.
//!
//! ```
//! use od_core::{ActorId, Database, Entity, Geometry, Point3};
//!
//! let mut db = Database::new(ActorId::SYSTEM);
//! let layer = db.ensure_layer("M-DUCT-SA");
//! let space = db.model_space();
//! db.insert_entity(Entity::new(layer, space, Geometry::Line {
//!     a: Point3::ORIGIN,
//!     b: Point3::new(5000.0, 2000.0, 0.0),
//! }))?;
//!
//! let svg = od_io_svg::to_svg(&db, &Default::default());
//! assert!(svg.starts_with("<svg"));
//! assert!(svg.contains("<line"));
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod color;
mod render;

use od_core::{Database, ObjectId};
use od_geom3d::Aabb3;

pub use render::ViewBox;

/// What to paint behind the drawing.
///
/// This is not only decoration: colour 7 means "the opposite of the paper", so
/// the background decides whether it is black or white. Getting it wrong
/// produces a drawing that is technically correct and completely unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Background {
    /// White, as plotted.
    #[default]
    Paper,
    /// The dark canvas a CAD application usually shows.
    Dark,
    /// Nothing — for embedding in a page that paints its own ground.
    None,
}

impl Background {
    #[must_use]
    pub fn fill(self) -> Option<&'static str> {
        render::background_fill(self)
    }

    #[must_use]
    pub fn foreground(self) -> (u8, u8, u8) {
        render::background_foreground(self)
    }

    #[must_use]
    pub fn is_dark(self) -> bool {
        matches!(self, Background::Dark)
    }
}

#[derive(Debug, Clone)]
pub struct SvgOptions {
    /// Which space to draw. Defaults to model space.
    pub space: Option<ObjectId>,
    /// Only draw what meets this window. Defaults to the whole drawing.
    pub window: Option<Aabb3>,
    pub background: Background,
    /// Pixel width for the `width`/`height` attributes. Without it the SVG is
    /// resolution-independent and fills whatever box it is placed in.
    pub width_px: Option<u32>,
    /// Restrict to these layer names, case-insensitively.
    pub layers: Option<Vec<String>>,
    /// A safety valve: a browser asked to lay out ten million elements stops
    /// responding, and a truncated drawing with a warning is more useful than
    /// a hung tab.
    pub max_entities: usize,
    pub font_family: String,
    /// Dash pattern applied to every stroke, in units scaled by the entity's
    /// linetype factor. Per-linetype patterns arrive with the linetype table.
    pub dash_pattern: Option<Vec<f64>>,
}

impl Default for SvgOptions {
    fn default() -> Self {
        Self {
            space: None,
            window: None,
            background: Background::default(),
            width_px: None,
            layers: None,
            max_entities: 200_000,
            // A CJK-capable stack: drawing annotation is Japanese, and a
            // fallback that cannot render it produces a page of tofu.
            font_family: "Hiragino Sans, Noto Sans JP, sans-serif".to_owned(),
            dash_pattern: None,
        }
    }
}

impl SvgOptions {
    #[must_use]
    pub fn with_background(mut self, background: Background) -> Self {
        self.background = background;
        self
    }

    #[must_use]
    pub fn with_window(mut self, window: Aabb3) -> Self {
        self.window = Some(window);
        self
    }

    #[must_use]
    pub fn with_layers(mut self, layers: Vec<String>) -> Self {
        self.layers = Some(layers);
        self
    }
}

/// Renders a drawing.
#[must_use]
pub fn to_svg(db: &Database, options: &SvgOptions) -> String {
    render::render(db, options).0
}

/// Renders a drawing and reports the `viewBox` it chose — for a caller (an
/// editing canvas) that needs to map a click on the image back to a drawing
/// coordinate, which the raw drawing extents alone cannot give it once
/// padding and the degenerate-extent fallbacks are accounted for.
#[must_use]
pub fn to_svg_with_view_box(db: &Database, options: &SvgOptions) -> (String, ViewBox) {
    render::render(db, options)
}

/// Renders a drawing to a file.
pub fn write_file(
    db: &Database,
    options: &SvgOptions,
    path: impl AsRef<std::path::Path>,
) -> std::io::Result<()> {
    std::fs::write(path, to_svg(db, options))
}
