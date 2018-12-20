//! editline port

use libc::*;

#[link(name = "readline")] extern "C" {
    fn using_history();
    fn add_history(p: *const c_char) -> c_int;
    fn clear_history();
    fn read_history(f: *const c_char) -> c_int;
    fn write_history(f: *const c_char) -> c_int;

    fn readline(prompt: *const c_char) -> *mut c_char;
}

use std::ptr::null;
use std::ffi::{CString, CStr};
use std::str::Utf8Error;

pub struct MallocStr(*mut c_char);
impl Drop for MallocStr {
    fn drop(&mut self) { unsafe { free(self.0 as *mut _) } }
}
impl MallocStr {
    pub fn as_str(&self) -> Result<&str, Utf8Error> {
        unsafe { CStr::from_ptr(self.0).to_str() }
    }
}

pub struct Readline(Option<CString>);
impl Readline {
    pub fn init(historyfile: Option<&str>) -> Self {
        let historyfile_c = historyfile.map(|p| CString::new(p).unwrap());

        unsafe {
            using_history();
            read_history(historyfile_c.as_ref().map(|x| x.as_ptr()).unwrap_or_else(null));
        }

        Readline(historyfile_c)
    }
    
    pub fn readline(&self, prompt: &str) -> Option<MallocStr> {
        let prompt_c = CString::new(prompt).unwrap();

        unsafe {
            let p = readline(prompt_c.as_ptr());
            if p.is_null() { return None; }
            add_history(p);
            Some(MallocStr(p))
        }
    }
}
impl Drop for Readline {
    fn drop(&mut self) {
        unsafe { write_history(self.0.as_ref().map(|x| x.as_ptr()).unwrap_or_else(null)); }
    }
}
