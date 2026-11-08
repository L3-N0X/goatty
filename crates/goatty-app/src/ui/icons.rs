//! Icons: small stroke SVGs embedded in the binary and served to GPUI's
//! `svg()` element through [`Assets`].
//!
//! GPUI draws an SVG as a mask in the element's text color, so the files'
//! own colors don't matter. They are drawn on a 24×24 grid with 2px round
//! strokes; add new ones in the same style to `assets/icons/` and here.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString, Styled, Svg, svg};

macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon { $($variant),* }

        impl Icon {
            pub const ALL: &[Icon] = &[$(Icon::$variant),*];

            /// The asset path `svg().path(…)` takes.
            pub fn path(self) -> &'static str {
                match self { $(Icon::$variant => concat!("icons/", $file)),* }
            }

            fn bytes(self) -> &'static [u8] {
                match self { $(Icon::$variant => include_bytes!(concat!("../../assets/icons/", $file))),* }
            }
        }
    };
}

icons! {
    CircleCheck => "circle-check.svg",
    CircleX => "circle-x.svg",
    Info => "info.svg",
    TriangleAlert => "triangle-alert.svg",
    Loader => "loader.svg",
    Close => "x.svg",
}

impl Icon {
    /// An `svg()` element showing this icon (size and color are the caller's).
    pub fn svg(self) -> Svg {
        svg().path(self.path()).flex_none()
    }
}

/// goatty's embedded assets, installed with `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL.iter().find(|i| i.path() == path).map(|i| Cow::Borrowed(i.bytes())))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Icon::ALL.iter().map(|i| i.path()).filter(|p| p.starts_with(path)).map(SharedString::from).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_loads_and_is_svg() {
        for icon in Icon::ALL {
            let bytes = Assets.load(icon.path()).unwrap().unwrap_or_else(|| panic!("{icon:?} missing"));
            assert!(std::str::from_utf8(&bytes).unwrap().starts_with("<svg"), "{icon:?}");
        }
        assert!(Assets.load("icons/nope.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons/").unwrap().len(), Icon::ALL.len());
    }
}
