use std::fmt;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use libloading::{Library, Symbol};

/// Lowest supported ABI version of a grammar.
// WARNING: update when updating vendored c sources
// `TREE_SITTER_MIN_COMPATIBLE_LANGUAGE_VERSION`
pub const MIN_COMPATIBLE_ABI_VERSION: u32 = 13;
// `TREE_SITTER_LANGUAGE_VERSION`
pub const ABI_VERSION: u32 = 15;

// opaque pointer
enum GrammarData {}

#[repr(transparent)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Grammar {
    ptr: NonNull<GrammarData>,
}

unsafe impl Send for Grammar {}
unsafe impl Sync for Grammar {}

impl std::fmt::Debug for Grammar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grammar").finish_non_exhaustive()
    }
}

impl Grammar {
    /// Loads a shared library containing a tree sitter grammar with name `name`
    // from `library_path`.
    ///
    /// # Safety
    ///
    /// `library_path` must be a valid tree sitter grammar
    pub unsafe fn new(name: &str, library_path: &Path) -> Result<Grammar, Error> {
        let library = unsafe {
            Library::new(library_path).map_err(|err| Error::DlOpen {
                err,
                path: library_path.to_owned(),
            })?
        };
        let language_fn_name = format!("tree_sitter_{}", name.replace('-', "_"));
        let grammar = unsafe {
            let language_fn: Symbol<unsafe extern "C" fn() -> NonNull<GrammarData>> = library
                .get(language_fn_name.as_bytes())
                .map_err(|err| Error::DlSym {
                    err,
                    symbol: name.to_owned(),
                })?;
            Grammar { ptr: language_fn() }
        };
        let version = grammar.version();
        if (MIN_COMPATIBLE_ABI_VERSION..=ABI_VERSION).contains(&version) {
            std::mem::forget(library);
            Ok(grammar)
        } else {
            Err(Error::IncompatibleVersion { version })
        }
    }

    pub fn version(self) -> u32 {
        unsafe { ts_language_version(self) }
    }

    pub fn node_kind_is_visible(self, kind_id: u16) -> bool {
        let symbol_type = unsafe { ts_language_symbol_type(self, kind_id) };
        symbol_type <= (SymbolType::Anonymous as u32)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Error opening dynamic library {path:?}")]
    DlOpen {
        #[source]
        err: libloading::Error,
        path: PathBuf,
    },
    #[error("Failed to load symbol {symbol}")]
    DlSym {
        #[source]
        err: libloading::Error,
        symbol: String,
    },
    #[error("Tried to load grammar with incompatible ABI {version}.")]
    IncompatibleVersion { version: u32 },
}

/// An error that occurred when trying to assign an incompatible [`Grammar`] to
/// a [`crate::parser::Parser`].
#[derive(Debug, PartialEq, Eq)]
pub struct IncompatibleGrammarError {
    version: u32,
}

impl fmt::Display for IncompatibleGrammarError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "Tried to load grammar with incompatible ABI {}.",
            self.version,
        )
    }
}
impl std::error::Error for IncompatibleGrammarError {}

#[repr(u32)]
#[allow(dead_code)]
pub enum SymbolType {
    Regular,
    Anonymous,
    Supertype,
    Auxiliary,
}

extern "C" {
    /// Get the ABI version number for this language. This version number
    /// is used to ensure that languages were generated by a compatible version of
    /// Tree-sitter. See also `ts_parser_set_language`.
    pub fn ts_language_version(grammar: Grammar) -> u32;

    /// Checks whether the given node type belongs to named nodes, anonymous nodes, or hidden
    /// nodes.
    ///
    /// See also `ts_node_is_named`. Hidden nodes are never returned from the API.
    pub fn ts_language_symbol_type(grammar: Grammar, symbol: u16) -> u32;
}
