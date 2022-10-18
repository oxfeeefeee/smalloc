//!  Smalloc is a small and simple memory allocator for Solana programs.
//!  
//!  # Usage:
//!  1. Add this crate as dependency
//!
//!  2. Add a dummy feature called "custom-heap":
//!  [features]
//!  default = ["custom-heap"]
//!  custom-heap = []
//!
//!  3. Put this in your entrypoint.rs
//! ```
//! // START: Heap start
//! // LENGTH: Heap length
//! // MIN: Minimal allocation size
//! // MAX: Maximal allocation size
//! // PAGE_SIZE: Allocation page size
//! #[cfg(target_os = "solana")]
//! #[global_allocator]
//! static ALLOC: Smalloc<{ HEAP_START_ADDRESS as usize }, { HEAP_LENGTH as usize }, 16, 1024, 1024> =
//!     Smalloc::new();
//! ```
//!
//! Note: The "with_static" feature is for unit tests.
//!

use std::{alloc::Layout, cmp, mem::size_of, ptr, ptr::null_mut, usize};

pub struct Smalloc<
    const START: usize,
    const LENGTH: usize,
    const MIN: usize,
    const MAX: usize,
    const PAGE_SIZE: usize,
> {
    #[cfg(feature = "with_static")]
    start: usize,
    #[cfg(feature = "with_static")]
    length: usize,
    #[cfg(feature = "with_static")]
    min: usize,
    #[cfg(feature = "with_static")]
    max: usize,
    #[cfg(feature = "with_static")]
    page_size: usize,
}

impl<
        const START: usize,
        const LENGTH: usize,
        const MIN: usize,
        const MAX: usize,
        const PAGE_SIZE: usize,
    > Smalloc<START, LENGTH, MIN, MAX, PAGE_SIZE>
{
    #[cfg(not(feature = "with_static"))]
    pub const fn new() -> Smalloc<START, LENGTH, MIN, MAX, PAGE_SIZE> {
        checks(START, LENGTH, MIN, MAX, PAGE_SIZE);
        Smalloc {}
    }

    #[cfg(feature = "with_static")]
    pub fn new(
        start: usize,
        length: usize,
        min: usize,
        max: usize,
        page_size: usize,
    ) -> Smalloc<START, LENGTH, MIN, MAX, PAGE_SIZE> {
        checks(start, length, min, max, page_size);
        Smalloc {
            start,
            length,
            min,
            max,
            page_size,
        }
    }

    #[inline]
    fn get_start(&self) -> usize {
        #[cfg(feature = "with_static")]
        let start = self.start;
        #[cfg(not(feature = "with_static"))]
        let start = START;

        start
    }
}

unsafe impl<
        const START: usize,
        const LENGTH: usize,
        const MIN: usize,
        const MAX: usize,
        const PAGE_SIZE: usize,
    > std::alloc::GlobalAlloc for Smalloc<START, LENGTH, MIN, MAX, PAGE_SIZE>
{
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let start = self.get_start();
        let p = start as *mut usize;
        let inner = Inner { p };

        if inner.start() == 0 {
            #[cfg(feature = "with_static")]
            let (start, length, min, max, page_size) =
                (self.start, self.length, self.min, self.max, self.page_size);
            #[cfg(not(feature = "with_static"))]
            let (start, length, min, max, page_size) = (START, LENGTH, MIN, MAX, PAGE_SIZE);
            inner.init(start, length, min, max, page_size)
        }

        let level = inner.size_level(layout.size());
        if level > inner.max_level() {
            return null_mut();
        }

        let head = inner.free_list(level);
        // There are free node on the free list
        if *head != 0 {
            return Inner::remove_free0(head);
        }

        // Try allocate new page
        if inner.page_used() < inner.page_count() {
            let page_start = inner.start() + inner.page_size() * inner.page_used();
            let size = 1 << (level + inner.log2_min());
            let count = inner.page_size() / size;
            inner.alloc_page(page_start, size, count);
            let head = inner.free_list(level);
            *head = page_start;
            return Inner::remove_free0(head);
        }

        null_mut()
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !ptr.is_null() {
            let inner = Inner {
                p: self.get_start() as *mut usize,
            };
            inner.insert_free(ptr, inner.size_level(layout.size()));
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let inner = Inner {
            p: self.get_start() as *mut usize,
        };
        // The level is not changed, no need to realloc
        if inner.size_level(layout.size()) == inner.size_level(new_size) {
            return ptr;
        }

        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new_ptr = self.alloc(new_layout);
        if !new_ptr.is_null() {
            ptr::copy_nonoverlapping(ptr, new_ptr, cmp::min(layout.size(), new_size));
            self.dealloc(ptr, layout);
        }
        new_ptr
    }
}

struct Inner {
    p: *mut usize,
}

impl Inner {
    #[inline]
    fn start(&self) -> usize {
        self.nth_val(0)
    }

    /// log2(min_alloc_size)
    #[inline]
    fn log2_min(&self) -> usize {
        self.nth_val(1)
    }

    /// Max level, starting from 0
    #[inline]
    fn max_level(&self) -> usize {
        self.nth_val(2)
    }

    #[inline]
    fn page_size(&self) -> usize {
        self.nth_val(3)
    }

    #[inline]
    fn page_count(&self) -> usize {
        self.nth_val(4)
    }

    #[inline]
    fn page_used(&self) -> usize {
        self.nth_val(5)
    }

    #[inline]
    fn page_used_inc(&self) {
        unsafe {
            *self.nth_ptr(5) = self.page_used() + 1;
        }
    }

    /// The linked lists of free memory for each size level
    #[inline]
    fn free_list(&self, level: usize) -> *mut usize {
        self.nth_ptr(6 + level)
    }

    #[inline]
    fn nth_ptr(&self, n: usize) -> *mut usize {
        unsafe { self.p.add(n) }
    }

    #[inline]
    fn nth_val(&self, n: usize) -> usize {
        unsafe { *self.nth_ptr(n) }
    }

    #[inline]
    unsafe fn init(&self, start: usize, length: usize, min: usize, max: usize, page_size: usize) {
        let log2_min = round_up_log2(min as u32);
        let log2_max = round_up_log2(max as u32);
        let max_level = log2_max - log2_min;

        *self.nth_ptr(0) = start;
        *self.nth_ptr(1) = log2_min;
        *self.nth_ptr(2) = max_level;
        *self.nth_ptr(3) = page_size;
        *self.nth_ptr(4) = length / page_size;
        *self.nth_ptr(5) = 0;

        let block_size: usize = cmp::max(min, size_of::<usize>());
        let used_count = 6 + self.max_level() + 1;
        let start = self.start() + used_count * block_size;
        let block_count = self.page_size() / block_size - used_count;
        // Allocate the first page and spare space for the Inner
        self.alloc_page(start, block_size, block_count);

        // init free-list-heads
        for i in 0..(self.max_level() + 1) {
            *self.free_list(i) = 0;
        }
        *self.free_list(self.size_level(block_size)) = start;
    }

    #[inline]
    unsafe fn alloc_page(&self, page_start: usize, size: usize, count: usize) {
        for i in 0..(count - 1) {
            let p = (page_start + (size * i)) as *mut usize;
            *p = page_start + (size * (i + 1));
        }
        let ptr_end = (page_start + (count - 1) * size) as *mut usize;
        *ptr_end = 0;

        self.page_used_inc();
    }

    #[inline]
    unsafe fn remove_free0(head: *mut usize) -> *mut u8 {
        let free0 = *head;
        let free0_next = *(free0 as *mut usize);
        *head = free0_next;
        free0 as *mut u8
    }

    #[inline]
    unsafe fn insert_free(&self, p: *mut u8, level: usize) {
        let head = self.free_list(level);
        let free0 = *head;
        *head = p as usize;
        *(p as *mut usize) = free0;
    }

    #[inline]
    fn size_level(&self, size: usize) -> usize {
        let log2 = round_up_log2(size as u32);
        let log2_min = self.log2_min();
        cmp::max(log2, log2_min) - log2_min
    }
}

const fn checks(start: usize, length: usize, min: usize, max: usize, page_size: usize) {
    // 0 is used as a marker
    assert!(start > 0);
    // support size up to u32::MAX
    assert!(page_size <= u32::MAX as usize);
    assert!(page_size < length && length % page_size == 0);
    // min & max have to be power of 2

    assert!(min >= size_of::<usize>());
    assert!(round_up_to_power2(min as u32) == min as u32);
    assert!(round_up_to_power2(max as u32) == max as u32);
    assert!(max >= min);
    assert!(max <= page_size && page_size % max == 0);
    // Big enough to store inner data
    assert!(page_size / min >= 32);
}

const fn round_up_to_power2(size: u32) -> u32 {
    let mut v = size as u32;
    v -= 1;
    v |= v >> 1;
    v |= v >> 2;
    v |= v >> 4;
    v |= v >> 8;
    v |= v >> 16;
    v += 1;
    v
}

/// Get log2(size_round_up_to_power_of_2)
#[inline]
const fn round_up_log2(size: u32) -> usize {
    const MULTIPLY_DE_BRUIJN_BIT_POSITION: [usize; 32] = [
        0, 1, 16, 2, 29, 17, 3, 22, 30, 20, 18, 11, 13, 4, 7, 23, 31, 15, 28, 21, 19, 10, 12, 6,
        14, 27, 9, 5, 26, 8, 25, 24,
    ];
    // first round up to power of 2
    let v = round_up_to_power2(size);
    const MAGIC: u32 = 0x06EB14F9u32;
    MULTIPLY_DE_BRUIJN_BIT_POSITION[(v.overflowing_mul(MAGIC).0 >> 27) as usize]
}

#[cfg(test)]
mod test {
    use super::Smalloc;
    use std::alloc::GlobalAlloc;
    use std::alloc::Layout;

    #[test]
    fn test_log2() {
        let sizes = vec![
            1, 2, 5, 8, 9, 16, 20, 32, 33, 64, 65, 128, 129, 256, 257, 512, 513, 1024, 1025, 2048,
            2049, 4096, 4097,
        ];
        let logs = vec![
            0, 1, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
        ];
        for (i, &s) in sizes.iter().enumerate() {
            assert_eq!(super::round_up_log2(s), logs[i]);
        }
    }

    fn max_level(min: usize, max: usize) -> usize {
        let log2_min = super::round_up_log2(min as u32);
        let log2_max = super::round_up_log2(max as u32);
        log2_max - log2_min
    }

    unsafe fn assign(p: *mut u8, v: u8) {
        if !p.is_null() {
            *p = v;
        }
    }

    #[test]
    fn test_alloc() {
        unsafe {
            let length = 256 * 1024;
            let min = 8;
            let max = 1024;
            let page_size = 1024;
            let mem = std::alloc::alloc(Layout::from_size_align(512 * 1024, page_size).unwrap());
            let start = mem as usize;
            let end = mem.add(length);

            dbg!(super::round_up_log2(min as u32));

            let max_level = max_level(min, max);
            let real_star_8 = (start + (6 + max_level + 1) * min) as *mut u8;
            dbg!(mem, end, max_level, real_star_8);

            let a = Smalloc::<0, 0, 0, 0, 0>::new(start, length, min, max, page_size);
            let lo = Layout::from_size_align(8, 8).unwrap();
            for i in 0..250 {
                dbg!(i);
                let p = a.alloc(lo);
                let p2 = a.alloc(lo);
                let p3 = a.alloc(lo);
                let p4 = a.alloc(lo);
                let p5 = a.alloc(lo);
                //a.dealloc(p3, lo);
                //a.dealloc(p4, lo);
                let p6 = a.alloc(lo);
                let p7 = a.alloc(lo);
                let p8 = a.alloc(lo);
                let p9 = a.alloc(lo);
                //a.alloc(lo);
                //a.alloc(lo);
                let p10 = a.alloc(lo);
                assign(p, 123);
                assign(p2, 123);
                assign(p3, 123);
                assign(p4, 123);
                assign(p5, 123);
                assign(p6, 123);
                assign(p7, 123);
                assign(p8, 123);
                assign(p9, 123);
                assign(p10, 123);
                //let pp = a.realloc(p, lo, 64);
                dbg!(p, p2, p3, p4, p5, p6, p7, p8, p9, p10);
            }
        }
    }
}
