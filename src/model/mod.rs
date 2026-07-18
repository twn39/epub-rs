//! EPUB Domain Models

pub mod a11y;
pub mod base;
pub mod book;
pub mod guide;
pub mod manifest;
pub mod metadata;
pub mod navigation;
pub mod position;
pub mod smil;
pub mod spine;

// Re-export all basic types and enums
pub use base::{EpubVersion, LayoutType, PageSpread, ReadingProgression, RenditionInfo};

// Re-export manifest items
pub use manifest::ManifestItem;

// Re-export spine items
pub use spine::SpineItem;

// Re-export navigation models
pub use navigation::{NavigationDocument, TocEntry};

// Re-export position locators
pub use position::{ContentElement, Position};

// Re-export metadata declarations
pub use metadata::{AltIdentifier, BelongsTo, Creator, Metadata, TitleEntry};

// Re-export SMIL models
pub use smil::{MediaOverlayMetadata, SmilDocument, SmilObject};

// Re-export accessibility models
pub use a11y::{
    A11yAccessMode, A11yCertification, A11yExemption, A11yFeature, A11yHazard,
    A11yPrimaryAccessMode, A11yProfile, Accessibility,
};

// Re-export guide references (EPUB 2)
pub use guide::GuideReference;

// Re-export central book structure
pub use book::EpubBook;
