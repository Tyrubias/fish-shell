//! A wrapper around the terminfo library to expose the functionality that fish needs.
//! Note that this is, on the whole, extremely little, and in practice terminfo
//! barely matters anymore. Even the few terminals in use that don't use "xterm-256color"
//! do not differ much.

use crate::common::ToCString;
use crate::FLOGF;
use std::env;
use std::ffi::{CStr, CString};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;

/// The [`Term`] singleton. Initialized via a call to [`setup()`] and surfaced to the outside world via [`term()`].
///
/// It isn't guaranteed that fish will ever be able to successfully call `setup()`, so this must
/// remain an `Option` instead of returning `Term` by default and just panicking if [`term()`] was
/// called before `setup()`.
///
/// We can't just use an [`AtomicPtr<Arc<Term>>`](std::sync::atomic::AtomicPtr) here because there's a race condition when the old Arc
/// gets dropped - we would obtain the current (non-null) value of `TERM` in [`term()`] but there's
/// no guarantee that a simultaneous call to [`setup()`] won't result in this refcount being
/// decremented to zero and the memory being reclaimed before we can clone it, since we can only
/// atomically *read* the value of the pointer, not clone the `Arc` it points to.
pub static TERM: Mutex<Option<Arc<Term>>> = Mutex::new(None);

/// Returns a reference to the global [`Term`] singleton or `None` if not preceded by a successful
/// call to [`terminal::setup()`](setup).
pub fn term() -> Option<Arc<Term>> {
    TERM.lock()
        .expect("Mutex poisoned!")
        .as_ref()
        .map(Arc::clone)
}

/// The safe wrapper around terminfo functionality, initialized by a successful call to [`setup()`]
/// and obtained thereafter by calls to [`term()`].
#[allow(dead_code)]
#[derive(Default)]
pub struct Term {
    // String capabilities. Any Some value is confirmed non-empty.
    pub enter_bold_mode: Option<CString>,
    pub enter_italics_mode: Option<CString>,
    pub exit_italics_mode: Option<CString>,
    pub enter_dim_mode: Option<CString>,
    pub enter_underline_mode: Option<CString>,
    pub exit_underline_mode: Option<CString>,
    pub enter_reverse_mode: Option<CString>,
    pub enter_standout_mode: Option<CString>,
    pub exit_standout_mode: Option<CString>,
    pub enter_blink_mode: Option<CString>,
    pub enter_protected_mode: Option<CString>,
    pub enter_shadow_mode: Option<CString>,
    pub exit_shadow_mode: Option<CString>,
    pub enter_secure_mode: Option<CString>,
    pub enter_alt_charset_mode: Option<CString>,
    pub exit_alt_charset_mode: Option<CString>,
    pub set_a_foreground: Option<CString>,
    pub set_foreground: Option<CString>,
    pub set_a_background: Option<CString>,
    pub set_background: Option<CString>,
    pub exit_attribute_mode: Option<CString>,
    pub set_title: Option<CString>,
    pub clear_screen: Option<CString>,
    pub cursor_up: Option<CString>,
    pub cursor_down: Option<CString>,
    pub cursor_left: Option<CString>,
    pub cursor_right: Option<CString>,
    pub parm_left_cursor: Option<CString>,
    pub parm_right_cursor: Option<CString>,
    pub clr_eol: Option<CString>,
    pub clr_eos: Option<CString>,

    // Number capabilities
    pub max_colors: Option<usize>,
    pub init_tabs: Option<usize>,

    // Flag/boolean capabilities
    pub eat_newline_glitch: bool,
    pub auto_right_margin: bool,
}

impl Term {
    /// Initialize a new `Term` instance, prepopulating the values of all the terminfo string
    /// capabilities we care about in the process.
    fn new(db: terminfo::Database) -> Self {
        Term {
            // String capabilities
            enter_bold_mode: get_str_cap(&db, "md"),
            enter_italics_mode: get_str_cap(&db, "ZH"),
            exit_italics_mode: get_str_cap(&db, "ZR"),
            enter_dim_mode: get_str_cap(&db, "mh"),
            enter_underline_mode: get_str_cap(&db, "us"),
            exit_underline_mode: get_str_cap(&db, "ue"),
            enter_reverse_mode: get_str_cap(&db, "mr"),
            enter_standout_mode: get_str_cap(&db, "so"),
            exit_standout_mode: get_str_cap(&db, "se"),
            enter_blink_mode: get_str_cap(&db, "mb"),
            enter_protected_mode: get_str_cap(&db, "mp"),
            enter_shadow_mode: get_str_cap(&db, "ZM"),
            exit_shadow_mode: get_str_cap(&db, "ZU"),
            enter_secure_mode: get_str_cap(&db, "mk"),
            enter_alt_charset_mode: get_str_cap(&db, "as"),
            exit_alt_charset_mode: get_str_cap(&db, "ae"),
            set_a_foreground: get_str_cap(&db, "AF"),
            set_foreground: get_str_cap(&db, "Sf"),
            set_a_background: get_str_cap(&db, "AB"),
            set_background: get_str_cap(&db, "Sb"),
            exit_attribute_mode: get_str_cap(&db, "me"),
            set_title: get_str_cap(&db, "ts"),
            clear_screen: get_str_cap(&db, "cl"),
            cursor_up: get_str_cap(&db, "up"),
            cursor_down: get_str_cap(&db, "do"),
            cursor_left: get_str_cap(&db, "le"),
            cursor_right: get_str_cap(&db, "nd"),
            parm_left_cursor: get_str_cap(&db, "LE"),
            parm_right_cursor: get_str_cap(&db, "RI"),
            clr_eol: get_str_cap(&db, "ce"),
            clr_eos: get_str_cap(&db, "cd"),

            // Number capabilities
            max_colors: get_num_cap(&db, "Co"),
            init_tabs: get_num_cap(&db, "it"),

            // Flag/boolean capabilities
            eat_newline_glitch: get_flag_cap(&db, "xn"),
            auto_right_margin: get_flag_cap(&db, "am"),
        }
    }
}

/// Initializes our database with the provided `$TERM` value `term` (or None).
/// Returns a reference to the newly initialized [`Term`] singleton on success or `None` if this failed.
///
/// The `configure` parameter may be set to a callback that takes an `&mut Term` reference to
/// override any capabilities before the `Term` is permanently made immutable.
///
/// Any existing references from `terminal::term()` will be invalidated by this call!
pub fn setup<F>(term: Option<&str>, configure: F) -> Option<Arc<Term>>
where
    F: Fn(&mut Term),
{
    let mut global_term = TERM.lock().expect("Mutex poisoned!");

    let res = if let Some(term) = term {
        terminfo::Database::from_name(term)
    } else {
        // For historical reasons getting "None" means to get it from the environment.
        terminfo::Database::from_env()
    }
    .or_else(|x| {
        // Try some more paths
        let t = if let Some(term) = term {
            term.to_string()
        } else if let Ok(name) = env::var("TERM") {
            name
        } else {
            return Err(x);
        };
        let first_char = t.chars().next().unwrap().to_string();
        for dir in [
            "/run/current-system/sw/share/terminfo", // Nix
            "/usr/pkg/share/terminfo",               // NetBSD
        ] {
            let mut path = PathBuf::from(dir);
            path.push(first_char.clone());
            path.push(t.clone());
            FLOGF!(term_support, "Trying path '%ls'", path.to_str().unwrap());
            if let Ok(db) = terminfo::Database::from_path(path) {
                return Ok(db);
            }
        }
        Err(x)
    });

    // Safely store the new Term instance or replace the old one. We have the lock so it's safe to
    // drop the old TERM value and have its refcount decremented - no one will be cloning it.
    if let Ok(result) = res {
        // Create a new `Term` instance, prepopulate the capabilities we care about, and allow the
        // caller to override any as needed.
        let mut term = Term::new(result);
        (configure)(&mut term);

        let term = Arc::new(term);
        *global_term = Some(term.clone());
        Some(term)
    } else {
        *global_term = None;
        None
    }
}

pub fn setup_fallback_term() -> Arc<Term> {
    let mut global_term = TERM.lock().expect("Mutex poisoned!");
    // These values extracted from xterm-256color from ncurses 6.4
    let term = Term {
        enter_bold_mode: Some(CString::new("\x1b[1m").unwrap()),
        enter_italics_mode: Some(CString::new("\x1b[3m").unwrap()),
        exit_italics_mode: Some(CString::new("\x1b[23m").unwrap()),
        enter_dim_mode: Some(CString::new("\x1b[2m").unwrap()),
        enter_underline_mode: Some(CString::new("\x1b[4m").unwrap()),
        exit_underline_mode: Some(CString::new("\x1b[24m").unwrap()),
        enter_reverse_mode: Some(CString::new("\x1b[7m").unwrap()),
        enter_standout_mode: Some(CString::new("\x1b[7m").unwrap()),
        exit_standout_mode: Some(CString::new("\x1b[27m").unwrap()),
        enter_blink_mode: Some(CString::new("\x1b[5m").unwrap()),
        enter_secure_mode: Some(CString::new("\x1b[8m").unwrap()),
        enter_alt_charset_mode: Some(CString::new("\x1b(0").unwrap()),
        exit_alt_charset_mode: Some(CString::new("\x1b(B").unwrap()),
        set_a_foreground: Some(
            CString::new("\x1b[%?%p1%{8}%<%t3%p1%d%e%p1%{16}%<%t9%p1%{8}%-%d%e38;5;%p1%d%;m")
                .unwrap(),
        ),
        set_a_background: Some(
            CString::new("\x1b[%?%p1%{8}%<%t4%p1%d%e%p1%{16}%<%t10%p1%{8}%-%d%e48;5;%p1%d%;m")
                .unwrap(),
        ),
        exit_attribute_mode: Some(CString::new("\x1b(B\x1b[m").unwrap()),
        clear_screen: Some(CString::new("\x1b[H\x1b[2J").unwrap()),
        cursor_up: Some(CString::new("\x1b[A").unwrap()),
        cursor_down: Some(CString::new("\n").unwrap()),
        cursor_left: Some(CString::new("\x08").unwrap()),
        cursor_right: Some(CString::new("\x1b[C").unwrap()),
        parm_left_cursor: Some(CString::new("\x1b[%p1%dD").unwrap()),
        parm_right_cursor: Some(CString::new("\x1b[%p1%dC").unwrap()),
        clr_eol: Some(CString::new("\x1b[K").unwrap()),
        clr_eos: Some(CString::new("\x1b[J").unwrap()),
        max_colors: Some(256),
        init_tabs: Some(8),
        eat_newline_glitch: true,
        auto_right_margin: true,
        ..Default::default()
    };
    let term = Arc::new(term);
    *global_term = Some(term.clone());
    term
}

/// Return a nonempty String capability from termcap, or None if missing or empty.
/// Panics if the given code string does not contain exactly two bytes.
fn get_str_cap(db: &terminfo::Database, code: &str) -> Option<CString> {
    db.raw(code).map(|cap| match cap {
        terminfo::Value::True => "1".to_string().as_bytes().to_cstring(),
        terminfo::Value::Number(n) => n.to_string().as_bytes().to_cstring(),
        terminfo::Value::String(s) => s.clone().to_cstring(),
    })
}

/// Return a number capability from termcap, or None if missing.
/// Panics if the given code string does not contain exactly two bytes.
fn get_num_cap(db: &terminfo::Database, code: &str) -> Option<usize> {
    match db.raw(code) {
        Some(terminfo::Value::Number(n)) if *n >= 0 => Some(*n as usize),
        _ => None,
    }
}

/// Return a flag capability from termcap, or false if missing.
/// Panics if the given code string does not contain exactly two bytes.
fn get_flag_cap(db: &terminfo::Database, code: &str) -> bool {
    db.raw(code)
        .map(|cap| matches!(cap, terminfo::Value::True))
        .unwrap_or(false)
}

/// Covers over tparm() with one parameter.
pub fn tparm1(cap: &CStr, param1: i32) -> Option<CString> {
    assert!(!cap.to_bytes().is_empty());
    let cap = cap.to_bytes();
    terminfo::expand!(cap; param1).ok().map(|x| x.to_cstring())
}
