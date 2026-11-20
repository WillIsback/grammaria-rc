//! Global string interning pool.
//!
//! The rule set holds ~68 000 short strings (literals, postags, regex sources)
//! whose combined text is only ~2.2 MB. Storing each as its own `String` costs
//! a 24-byte header plus a separate heap allocation per string — that is the
//! dominant share of the rule table's memory.
//!
//! [`Sym`] is a 4-byte handle into a process-wide pool instead: identical
//! postags such as `D m s` (repeated tens of thousands of times) are stored
//! once, and comparisons become integer compares.
//!
//! The pool is append-only and never freed, which matches how the rule set is
//! used (loaded once, kept for the whole process lifetime).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A 4-byte interned string handle. `Sym::EMPTY` is the empty string.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Sym(u32);

impl Default for Sym {
    fn default() -> Sym {
        Sym::EMPTY
    }
}

impl Sym {
    /// The empty string. `as_str()` never panics, so callers do not need a
    /// special case for "no value".
    pub const EMPTY: Sym = Sym(0);

    #[inline]
    pub fn as_str(self) -> &'static str {
        match pool().lock() {
            Ok(g) => g.get(self.0 as usize),
            Err(e) => e.into_inner().get(self.0 as usize),
        }
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0 || self.as_str().is_empty()
    }

    #[inline]
    pub fn as_u32(self) -> u32 {
        self.0
    }

    #[inline]
    pub fn from_u32(v: u32) -> Sym {
        Sym(v)
    }
}

struct Pool {
    /// `&'static str` slices into `blob`, 8 bytes each.
    strings: Vec<&'static str>,
    /// Backing storage; never reallocated after startup in practice, but the
    /// slices are created from `Box<str>` so they stay valid even if `Vec`
    /// grows its own index array.
    blob: Vec<Box<str>>,
    index: HashMap<&'static str, u32>,
}

impl Pool {
    #[inline]
    fn get(&self, i: usize) -> &'static str {
        self.strings.get(i).copied().unwrap_or("")
    }
}

fn pool() -> &'static Mutex<Pool> {
    static POOL: OnceLock<Mutex<Pool>> = OnceLock::new();
    POOL.get_or_init(|| {
        let mut p = Pool {
            strings: Vec::with_capacity(1024),
            blob: Vec::new(),
            index: HashMap::new(),
        };
        // Slot 0 is the empty string.
        p.strings.push("");
        Mutex::new(p)
    })
}

/// Intern `s`, returning its handle.
pub fn intern(s: &str) -> Sym {
    let mut p = match pool().lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    };
    if let Some(&id) = p.index.get(s) {
        return Sym(id);
    }
    let boxed: Box<str> = s.to_owned().into_boxed_str();
    // Leak the box so the resulting `&'static str` is valid for the process
    // lifetime; the pool owns it conceptually and never frees it.
    let leaked: &'static str = Box::leak(boxed);
    let id = p.strings.len() as u32;
    p.strings.push(leaked);
    p.index.insert(leaked, id);
    Sym(id)
}

/// Number of distinct strings in the pool (diagnostics only).
pub fn pool_len() -> usize {
    match pool().lock() {
        Ok(g) => g.strings.len(),
        Err(e) => e.into_inner().strings.len(),
    }
}
