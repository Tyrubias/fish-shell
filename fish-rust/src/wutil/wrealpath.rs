use crate::wchar::{wstr, WString};

pub fn wrealpath(pathname: &wstr) -> Option<WString> {
    if pathname.is_empty() {
        return None;
    }

    todo!()
}
