//! Lazy OPF parse cache for multi-language adapters (WASM / C FFI).
//!
//! Adapters previously used `Option<EpubBook>`: on parse failure the cache stayed
//! `None`, so every subsequent API call re-ran the full OPF parse.  That is
//! wasteful for permanent failures and obscures whether a parse was never tried
//! versus already failed.
//!
//! [`LazyBook`] records `Unparsed | Ready | Failed` so:
//! - success is cached once;
//! - failure is sticky (same error returned without re-parsing);
//! - there is never a half-filled book left in the ready slot.

use crate::model::EpubBook;

/// Cache for a single OPF → [`EpubBook`] parse.
///
/// `Ready` boxes the book so the enum stays small on the handle stack
/// (`EpubBook` is large; only one variant is live at a time).
#[derive(Debug, Default)]
pub(crate) enum LazyBook {
    #[default]
    Unparsed,
    Ready(Box<EpubBook>),
    /// Cached error message from the first failed parse attempt.
    Failed(String),
}

impl LazyBook {
    /// Whether a parse has not been attempted yet.
    pub(crate) fn is_unparsed(&self) -> bool {
        matches!(self, Self::Unparsed)
    }

    /// Record the outcome of a first parse attempt.
    ///
    /// Call only while [`is_unparsed`] is true. Adapters parse via a sibling
    /// field (`archive`) first, then store here — avoiding simultaneous
    /// mutable borrows of two fields through a single `self` closure.
    pub(crate) fn store(&mut self, result: Result<EpubBook, String>) {
        debug_assert!(self.is_unparsed(), "store only while Unparsed");
        *self = match result {
            Ok(book) => Self::Ready(Box::new(book)),
            Err(err) => Self::Failed(err),
        };
    }

    /// After [`store`] (or a prior ensure), return the book or a sticky error.
    pub(crate) fn get(&self) -> Result<&EpubBook, &str> {
        match self {
            Self::Ready(book) => Ok(book),
            Self::Failed(err) => Err(err.as_str()),
            Self::Unparsed => Err("EPUB has not been parsed yet"),
        }
    }

    /// Borrow the cached book after a successful parse.
    pub(crate) fn as_book(&self) -> Option<&EpubBook> {
        match self {
            Self::Ready(book) => Some(book),
            Self::Unparsed | Self::Failed(_) => None,
        }
    }

    /// Clear the cache so a new OPF parse can run (e.g. multi-rendition switch).
    ///
    /// Callers must also drop any derived state keyed to the previous book
    /// (position index, etc.). After reset, [`store`] is valid again.
    pub(crate) fn reset(&mut self) {
        *self = Self::Unparsed;
    }

    /// Whether a successful parse is already cached.
    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        matches!(self, Self::Ready(_))
    }

    /// Whether a previous parse failed and the error is sticky.
    #[cfg(test)]
    pub(crate) fn is_failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Metadata;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn sample_book() -> EpubBook {
        EpubBook {
            metadata: Metadata {
                title: Some("Lazy".into()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Adapter-shaped ensure: only parse while unparsed, then get.
    fn ensure(
        lazy: &mut LazyBook,
        parse: impl FnOnce() -> Result<EpubBook, String>,
    ) -> Result<(), String> {
        if lazy.is_unparsed() {
            lazy.store(parse());
        }
        lazy.get().map(|_| ()).map_err(str::to_owned)
    }

    #[test]
    fn unparsed_success_becomes_ready() {
        let mut lazy = LazyBook::Unparsed;
        ensure(&mut lazy, || Ok(sample_book())).expect("parse should succeed");
        assert_eq!(
            lazy.as_book().unwrap().metadata.title.as_deref(),
            Some("Lazy")
        );
        assert!(lazy.is_ready());
    }

    #[test]
    fn ready_does_not_reparse() {
        let calls = AtomicUsize::new(0);
        let mut lazy = LazyBook::Unparsed;
        ensure(&mut lazy, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(sample_book())
        })
        .unwrap();
        ensure(&mut lazy, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(sample_book())
        })
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_is_sticky_without_retry() {
        let calls = AtomicUsize::new(0);
        let mut lazy = LazyBook::Unparsed;
        let err1 = ensure(&mut lazy, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Err("boom".into())
        })
        .unwrap_err();
        assert_eq!(err1, "boom");
        assert!(lazy.is_failed());

        let err2 = ensure(&mut lazy, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(sample_book())
        })
        .unwrap_err();
        assert_eq!(err2, "boom");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "failed state must not re-invoke parse even if a later closure would succeed"
        );
    }

    #[test]
    fn as_book_only_when_ready() {
        assert!(LazyBook::Unparsed.as_book().is_none());
        assert!(LazyBook::Failed("x".into()).as_book().is_none());
        let mut lazy = LazyBook::Unparsed;
        ensure(&mut lazy, || Ok(sample_book())).unwrap();
        assert!(lazy.as_book().is_some());
    }

    #[test]
    fn reset_allows_reparse() {
        let calls = AtomicUsize::new(0);
        let mut lazy = LazyBook::Unparsed;
        ensure(&mut lazy, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(sample_book())
        })
        .unwrap();
        lazy.reset();
        assert!(lazy.is_unparsed());
        ensure(&mut lazy, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(sample_book())
        })
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
