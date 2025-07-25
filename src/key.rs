use libc::VERASE;

use crate::{
    common::{escape_string, EscapeFlags, EscapeStringStyle},
    fallback::fish_wcwidth,
    flog::FloggableDebug,
    future_feature_flags::{test as feature_test, FeatureFlag},
    reader::safe_get_terminal_mode_on_startup,
    wchar::{decode_byte_from_char, prelude::*},
    wutil::{fish_is_pua, fish_wcstoul},
};

pub(crate) const Backspace: char = '\u{F500}'; // below ENCODE_DIRECT_BASE
pub(crate) const Delete: char = '\u{F501}';
pub(crate) const Escape: char = '\u{F502}';
pub(crate) const Enter: char = '\u{F503}';
pub(crate) const Up: char = '\u{F504}';
pub(crate) const Down: char = '\u{F505}';
pub(crate) const Left: char = '\u{F506}';
pub(crate) const Right: char = '\u{F507}';
pub(crate) const PageUp: char = '\u{F508}';
pub(crate) const PageDown: char = '\u{F509}';
pub(crate) const Home: char = '\u{F50A}';
pub(crate) const End: char = '\u{F50B}';
pub(crate) const Insert: char = '\u{F50C}';
pub(crate) const Tab: char = '\u{F50D}';
pub(crate) const Space: char = '\u{F50E}';
pub(crate) const Menu: char = '\u{F50F}';
pub(crate) const PrintScreen: char = '\u{F510}';
pub(crate) const MAX_FUNCTION_KEY: u32 = 12;
pub(crate) fn function_key(n: u32) -> char {
    assert!((1..=MAX_FUNCTION_KEY).contains(&n));
    char::from_u32(u32::from('\u{F5FF}') - MAX_FUNCTION_KEY + (n - 1)).unwrap()
}
pub(crate) const Invalid: char = '\u{F5FF}';

pub(crate) const KEY_NAMES: &[(char, &wstr)] = &[
    ('-', L!("minus")),
    (',', L!("comma")),
    (Backspace, L!("backspace")),
    (Delete, L!("delete")),
    (Escape, L!("escape")),
    (Enter, L!("enter")),
    (Up, L!("up")),
    (Down, L!("down")),
    (Left, L!("left")),
    (Right, L!("right")),
    (PageUp, L!("pageup")),
    (PageDown, L!("pagedown")),
    (Home, L!("home")),
    (End, L!("end")),
    (Insert, L!("insert")),
    (Tab, L!("tab")),
    (Space, L!("space")),
    (Menu, L!("menu")),
    (PrintScreen, L!("printscreen")),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub sup: bool,
}

impl Modifiers {
    const fn new() -> Self {
        Modifiers {
            ctrl: false,
            alt: false,
            shift: false,
            sup: false,
        }
    }
    #[cfg(test)]
    pub(crate) const CTRL: Self = {
        let mut m = Self::new();
        m.ctrl = true;
        m
    };
    pub(crate) const ALT: Self = {
        let mut m = Self::new();
        m.alt = true;
        m
    };
    pub(crate) const SHIFT: Self = {
        let mut m = Self::new();
        m.shift = true;
        m
    };
    pub(crate) fn is_some(&self) -> bool {
        *self != Self::new()
    }
    pub(crate) fn is_none(&self) -> bool {
        *self == Self::new()
    }
}

/// Position in terminal coordinates, i.e. not starting from the prompt
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportPosition {
    pub x: usize,
    pub y: usize,
}
impl FloggableDebug for ViewportPosition {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key {
    pub modifiers: Modifiers,
    pub codepoint: char,
}

impl Key {
    pub(crate) const fn new(modifiers: Modifiers, codepoint: char) -> Self {
        Self {
            modifiers,
            codepoint,
        }
    }
    pub(crate) fn from_raw(codepoint: char) -> Self {
        Self::new(Modifiers::default(), codepoint)
    }
}

pub(crate) const fn ctrl(codepoint: char) -> Key {
    let mut modifiers = Modifiers::new();
    modifiers.ctrl = true;
    Key::new(modifiers, codepoint)
}

pub(crate) const fn alt(codepoint: char) -> Key {
    let mut modifiers = Modifiers::new();
    modifiers.alt = true;
    Key::new(modifiers, codepoint)
}

pub(crate) const fn shift(codepoint: char) -> Key {
    let mut modifiers = Modifiers::new();
    modifiers.shift = true;
    Key::new(modifiers, codepoint)
}

impl Key {
    pub fn from_single_char(c: char) -> Self {
        u8::try_from(c)
            .map(Key::from_single_byte)
            .unwrap_or(Key::from_raw(c))
    }
    pub fn from_single_byte(c: u8) -> Self {
        canonicalize_control_char(c).unwrap_or(Key::from_raw(char::from(c)))
    }
}

pub fn canonicalize_control_char(c: u8) -> Option<Key> {
    let codepoint = canonicalize_keyed_control_char(char::from(c));
    if u32::from(codepoint) > 255 {
        return Some(Key::from_raw(codepoint));
    }

    if c < 32 {
        return Some(ctrl(canonicalize_unkeyed_control_char(c)));
    }

    None
}

fn ascii_control(c: char) -> char {
    char::from_u32(u32::from(c) & 0o37).unwrap()
}

pub(crate) fn canonicalize_keyed_control_char(c: char) -> char {
    if c == ascii_control('m') {
        return Enter;
    }
    if c == ascii_control('i') {
        return Tab;
    }
    if c == ' ' {
        return Space;
    }
    if let Some(tm) = safe_get_terminal_mode_on_startup() {
        if c == char::from(tm.c_cc[VERASE]) {
            return Backspace;
        }
    }
    if c == char::from(127) {
        // when it's not backspace
        return Delete;
    }
    if c == '\x1b' {
        return Escape;
    }
    c
}

pub(crate) fn canonicalize_unkeyed_control_char(c: u8) -> char {
    if c == 0 {
        // For legacy terminals we have to make a decision here; they send NUL on Ctrl-2,
        // Ctrl-Shift-2 or Ctrl-Backtick, but the most straightforward way is Ctrl-Space.
        return Space;
    }
    // Represent Ctrl-letter combinations in lower-case, to be clear
    // that Shift is not involved.
    if c < 27 {
        return char::from(c - 1 + b'a');
    }
    // Represent Ctrl-symbol combinations in "upper-case", as they are
    // traditionally-rendered.
    assert!(c < 32);
    return char::from(c - 1 + b'A');
}

pub(crate) fn canonicalize_key(mut key: Key) -> Result<Key, WString> {
    // Leave raw escapes to disambiguate from named escape.
    if key.codepoint != '\x1b' {
        key.codepoint = canonicalize_keyed_control_char(key.codepoint);
        if key.codepoint < ' ' {
            key.codepoint = canonicalize_unkeyed_control_char(u8::try_from(key.codepoint).unwrap());
            if key.modifiers.ctrl {
                return Err(wgettext_fmt!(
                    "Cannot add control modifier to control character '%s'",
                    key
                ));
            }
            key.modifiers.ctrl = true;
        }
    }
    Ok(key)
}

pub const KEY_SEPARATOR: char = ',';

fn escape_nonprintables(key_name: &wstr) -> WString {
    escape_string(
        key_name,
        EscapeStringStyle::Script(EscapeFlags::NO_PRINTABLES | EscapeFlags::NO_QUOTED),
    )
}

#[allow(clippy::nonminimal_bool)]
pub(crate) fn parse_keys(value: &wstr) -> Result<Vec<Key>, WString> {
    let mut res = vec![];
    if value.is_empty() {
        return Ok(res);
    }
    let first = value.as_char_slice()[0];
    if value.len() == 1 {
        // Hack: allow singular comma.
        res.push(canonicalize_key(Key::from_raw(first)).unwrap());
    } else if ((2..=3).contains(&value.len())
        && !value.contains('-')
        && !value.contains(KEY_SEPARATOR)
        && !KEY_NAMES.iter().any(|(_codepoint, name)| name == value)
        && value.as_char_slice()[0] != 'F'
        && !(value.as_char_slice()[0] == 'f' && value.char_at(1).is_ascii_digit()))
        || first < ' '
    {
        // Hack: treat as legacy syntax (meaning: not comma separated) if
        // 1. it doesn't contain '-' or ',' and is short enough to probably not be a key name.
        // 2. it starts with an ASCII control character. This can be either a multi-key binding
        //    or a single-key that is sent as escape sequence (starting with \e).
        for c in value.chars() {
            res.push(canonicalize_key(Key::from_raw(c)).unwrap());
        }
    } else {
        for full_key_name in value.split(KEY_SEPARATOR) {
            if full_key_name == "-" {
                // Hack: allow singular minus.
                res.push(canonicalize_key(Key::from_raw('-')).unwrap());
                continue;
            }
            let mut modifiers = Modifiers::default();
            let num_keys = full_key_name.split('-').count();
            let mut components = full_key_name.split('-');
            for _i in 0..num_keys.checked_sub(1).unwrap() {
                let modifier = components.next().unwrap();
                match modifier {
                    _ if modifier == "ctrl" => modifiers.ctrl = true,
                    _ if modifier == "alt" => modifiers.alt = true,
                    _ if modifier == "shift" => modifiers.shift = true,
                    _ if modifier == "super" => modifiers.sup = true,
                    _ => {
                        return Err(wgettext_fmt!(
                            "unknown modifier '%s' in '%s'",
                            modifier,
                            escape_nonprintables(full_key_name)
                        ))
                    }
                }
            }
            let key_name = components.next().unwrap();
            let codepoint = KEY_NAMES
                .iter()
                .find_map(|(codepoint, name)| (name == key_name).then_some(*codepoint))
                .or_else(|| (key_name.len() == 1).then(|| key_name.as_char_slice()[0]));
            let key = if let Some(codepoint) = codepoint {
                canonicalize_key(Key::new(modifiers, codepoint))?
            } else if codepoint.is_none() && key_name.starts_with('f') && key_name.len() <= 3 {
                let num = key_name.strip_prefix('f').unwrap();
                let codepoint = match fish_wcstoul(num) {
                    Ok(n) if (1..=u64::from(MAX_FUNCTION_KEY)).contains(&n) => {
                        function_key(u32::try_from(n).unwrap())
                    }
                    _ => {
                        return Err(wgettext_fmt!(
                            "only f1 through f%d are supported, not 'f%s'",
                            MAX_FUNCTION_KEY,
                            num,
                        ));
                    }
                };
                Key::new(modifiers, codepoint)
            } else {
                return Err(wgettext_fmt!(
                    "cannot parse key '%s'",
                    escape_nonprintables(full_key_name)
                ));
            };
            res.push(key);
        }
    }
    Ok(canonicalize_raw_escapes(res))
}

pub(crate) fn canonicalize_raw_escapes(keys: Vec<Key>) -> Vec<Key> {
    // Historical bindings use \ek to mean alt-k. Canonicalize them.
    if !keys.iter().any(|key| key.codepoint == '\x1b') {
        return keys;
    }
    let mut canonical = vec![];
    let mut had_literal_escape = false;
    for mut key in keys {
        if had_literal_escape {
            had_literal_escape = false;
            if key.modifiers.alt {
                canonical.push(Key::from_raw(Escape));
            } else {
                key.modifiers.alt = true;
                if key.codepoint == '\x1b' {
                    key.codepoint = Escape;
                }
            }
        } else if key.codepoint == '\x1b' {
            had_literal_escape = true;
            continue;
        }
        canonical.push(key);
    }
    if had_literal_escape {
        canonical.push(Key::from_raw(Escape));
    }
    canonical
}

impl Key {
    pub(crate) fn codepoint_text(&self) -> Option<char> {
        if self.modifiers.is_some() {
            return None;
        }
        let c = self.codepoint;
        if c == Space {
            return Some(' ');
        }
        if c == Enter {
            return Some('\n');
        }
        if c == Tab {
            return Some('\t');
        }
        if fish_is_pua(c) || u32::from(c) <= 27 {
            return None;
        }
        Some(c)
    }
}

impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        WString::from(*self).fmt(f)
    }
}

impl fish_printf::ToArg<'static> for Key {
    fn to_arg(self) -> fish_printf::Arg<'static> {
        fish_printf::Arg::WString(self.into())
    }
}

impl From<Key> for WString {
    fn from(key: Key) -> Self {
        let name = KEY_NAMES
            .iter()
            .find_map(|&(codepoint, name)| (codepoint == key.codepoint).then(|| name.to_owned()))
            .or_else(|| {
                (function_key(1)..=function_key(MAX_FUNCTION_KEY))
                    .contains(&key.codepoint)
                    .then(|| {
                        sprintf!(
                            "f%d",
                            u32::from(key.codepoint) - u32::from(function_key(1)) + 1
                        )
                    })
            });
        let mut res =
            name.unwrap_or_else(|| char_to_symbol(key.codepoint, key.modifiers.is_none()));

        if key.modifiers.shift {
            res.insert_utfstr(0, L!("shift-"));
        }
        if key.modifiers.alt {
            res.insert_utfstr(0, L!("alt-"));
        }
        if key.modifiers.ctrl {
            res.insert_utfstr(0, L!("ctrl-"));
        }
        if key.modifiers.sup {
            res.insert_utfstr(0, L!("super-"));
        }

        res
    }
}

fn ctrl_to_symbol(buf: &mut WString, c: char) {
    // Most ascii control characters like \x01 are canonicalized as ctrl-a, except
    // 1. if we are explicitly given a codepoint < 32 via CSI u.
    // 2. key names that are given as raw escape sequence (\e123); those we want to display
    // similar to how they are given.

    let c = u8::try_from(c).unwrap();
    let symbolic_name = match c {
        9 => L!("\\t"),
        13 => L!("\\r"),
        27 => L!("\\e"),
        _ => return sprintf!(=> buf, "\\x%02x", c),
    };
    buf.push_utfstr(symbolic_name);
}

/// Return true if the character must be escaped when used in the sequence of chars to be bound in
/// a `bind` command.
fn must_escape(is_first_in_token: bool, c: char) -> bool {
    "[]()<>{}*\\$;&|'\"".contains(c)
        || (is_first_in_token && "~#".contains(c))
        || (c == '?' && !feature_test(FeatureFlag::qmark_noglob))
}

fn ascii_printable_to_symbol(buf: &mut WString, is_first_in_token: bool, c: char) {
    if must_escape(is_first_in_token, c) {
        sprintf!(=> buf, "\\%c", c);
    } else {
        sprintf!(=> buf, "%c", c);
    }
}

/// Convert a wide-char to a symbol that can be used in our output.
pub fn char_to_symbol(c: char, is_first_in_token: bool) -> WString {
    let mut buff = WString::new();
    let buf = &mut buff;
    if c <= ' ' || c == '\x7F' {
        ctrl_to_symbol(buf, c);
    } else if c < '\u{80}' {
        // ASCII characters that are not control characters
        ascii_printable_to_symbol(buf, is_first_in_token, c);
    } else if let Some(byte) = decode_byte_from_char(c) {
        sprintf!(=> buf, "\\x%02x", byte);
    } else if ('\u{e000}'..='\u{f8ff}').contains(&c) {
        // Unmapped key from https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
        sprintf!(=> buf, "\\u%04X", u32::from(c));
    } else if fish_wcwidth(c) > 0 {
        sprintf!(=> buf, "%lc", c);
    } else if c <= '\u{FFFF}' {
        // BMP Unicode character
        sprintf!(=> buf, "\\u%04X", u32::from(c));
    } else {
        sprintf!(=> buf, "\\U%06X", u32::from(c));
    }
    buff
}
