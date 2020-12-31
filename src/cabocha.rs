//! JpDependencyParser LibPort

use libc::*;
use std::ptr::NonNull;
use std::ffi::{CStr, CString};
use std::slice;

#[repr(C)] pub struct Chunk
{
    pub link: c_int, pub head_pos: usize, pub func_pos: usize, pub token_size: usize, pub token_pos: usize,
    pub score: c_float, pub feature_list: *const *const c_char, pub additional_info: *const c_char,
    pub feature_list_size: c_ushort
}
#[repr(C)] pub struct Token
{
    pub surface: *const c_char, pub normalized_surface: *const c_char,
    pub feature: *const c_char, pub feature_list: *const *const c_char, pub feature_list_size: c_ushort,
    pub ne: *const c_char, pub additional_info: *const c_char, pub chunk: *const Chunk
}
impl Chunk
{
    /*pub fn features(&self) -> Vec<&str> {
        unsafe {
            slice::from_raw_parts(self.feature_list, self.feature_list_size as _)
                .iter().map(|&p| CStr::from_ptr(p).to_str().unwrap()).collect()
        }
    }*/

    pub fn is_root(&self) -> bool { self.link < 0 }
}
impl Token {
    pub fn surface_normalized(&self) -> &str {
        unsafe { CStr::from_ptr(self.normalized_surface).to_str().unwrap() }
    }
    pub fn features(&self) -> impl Iterator<Item = &str> {
        unsafe {
            slice::from_raw_parts(self.feature_list, self.feature_list_size as _)
                .iter().map(|&p| CStr::from_ptr(p).to_str().unwrap())
        }
    }
    /// 品詞
    pub fn primary_part(&self) -> &str { self.features().next().unwrap() }
    /// 原型
    pub fn base_form(&self) -> &str { self.features().nth(6).unwrap() }
    /// 読み(カタカナ)
    pub fn reading_form(&self) -> Option<&str> { self.features().nth(7) }
}

#[link(name = "cabocha")] extern "C" {
    fn cabocha_new2(arg: *const c_char) -> *mut c_void;
    fn cabocha_destroy(cabocha: *mut c_void);
    fn cabocha_sparse_totree2(cabocha: *mut c_void, s: *const c_char, length: usize) -> *mut c_void;

    fn cabocha_tree_chunk_size(tree: *mut c_void) -> usize;
    fn cabocha_tree_token(tree: *mut c_void, i: usize) -> *mut Token;
    fn cabocha_tree_chunk(tree: *mut c_void, i: usize) -> *mut Chunk;
}

pub struct Cabocha(NonNull<c_void>);
impl Cabocha {
    pub fn new(arg: &str) -> Self {
        let carg = CString::new(arg).unwrap();
        NonNull::new(unsafe { cabocha_new2(carg.as_ptr()) }).map(Cabocha).unwrap()
    }
}
impl Drop for Cabocha {
    fn drop(&mut self) { unsafe { cabocha_destroy(self.0.as_ptr()) } }
}

pub struct TreeRef(NonNull<c_void>);
impl Cabocha {
    pub fn parse_str_to_tree(&self, s: &str) -> TreeRef {
        NonNull::new(unsafe { cabocha_sparse_totree2(self.0.as_ptr(), s.as_ptr() as *const _, s.as_bytes().len()) })
            .map(TreeRef).unwrap()
    }
}
impl TreeRef {
    pub fn chunk_size(&self) -> usize { unsafe { cabocha_tree_chunk_size(self.0.as_ptr()) } }
    pub fn chunk(&self, index: usize) -> Option<&Chunk> {
        unsafe { cabocha_tree_chunk(self.0.as_ptr(), index).as_ref() }
    }
    pub fn token(&self, index: usize) -> Option<&Token> {
        unsafe { cabocha_tree_token(self.0.as_ptr(), index).as_ref() }
    }
}

pub struct ChunkIter<'c> { source: &'c TreeRef, current: usize }
impl TreeRef {
    pub fn chunks<'c>(&'c self) -> ChunkIter<'c> { ChunkIter { source: self, current: 0 } }
}
/*impl<'c> ChunkIter<'c> {
    pub fn at(&self, index: usize) -> Option<&'c Token> {
        let ix = self.current + index;
        if ix < self.source.chunk_size() { self.source.token(ix) } else { None }
    }
}*/
impl<'c> Iterator for ChunkIter<'c> {
    type Item = &'c Chunk;

    fn next(&mut self) -> Option<&'c Chunk> {
        if self.current == self.source.chunk_size() { return None; }
        let t = self.source.chunk(self.current);
        if t.is_some() { self.current += 1; }
        return t;
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (self.len(), Some(self.len())) }
}
impl<'c> ExactSizeIterator for ChunkIter<'c> { fn len(&self) -> usize { self.source.chunk_size() - self.current } }
pub struct TokenIter<'c> { source: &'c TreeRef, current: usize, end: usize }
impl Chunk {
    pub fn tokens<'c>(&self, source: &'c TreeRef) -> TokenIter<'c> {
        TokenIter { source, current: self.token_pos, end: self.token_pos + self.token_size }
    }
}
impl<'c> TokenIter<'c> {
    pub fn at(&self, index: usize) -> Option<&'c Token> {
        let ix = self.current + index;
        if ix < self.end { self.source.token(ix) } else { None }
    }
}
impl<'c> Iterator for TokenIter<'c> {
    type Item = &'c Token;

    fn next(&mut self) -> Option<&'c Token> {
        if self.current == self.end { return None; }
        let t = self.source.token(self.current);
        if t.is_some() { self.current += 1; }
        return t;
    }
    fn size_hint(&self) -> (usize, Option<usize>) { (self.len(), Some(self.len())) }
}
impl<'c> ExactSizeIterator for TokenIter<'c> { fn len(&self) -> usize { self.end - self.current } }
