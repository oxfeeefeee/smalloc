use std::{alloc::Layout, mem::size_of, ptr::null_mut, usize};

pub struct Smalloc {
    pub start: usize,
    pub length: usize,
}

unsafe impl std::alloc::GlobalAlloc for Smalloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pos_ptr: *mut usize = self.start as *mut usize;
        let top_address: usize = self.start + self.length;
        let bottom_address: usize = self.start + size_of::<*mut u8>();

        let mut pos = *pos_ptr;
        if pos == 0 {
            // First time, set starting position
            pos = top_address;
        }
        pos = pos.saturating_sub(layout.size());
        pos &= !(layout.align().saturating_sub(1));
        if pos < bottom_address {
            return null_mut();
        }
        *pos_ptr = pos;
        pos as *mut u8
    }
    #[inline]
    unsafe fn dealloc(&self, _: *mut u8, layout: Layout) {}
}
