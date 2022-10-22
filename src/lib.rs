//!  Smalloc is a small and simple memory allocator for Solana programs.
//!  
//!  # Usage:
//!  1. Add this crate as dependency
//!
//!  2. Add a dummy feature called "custom-heap" in  Cargo.toml:
//!  ```
//!  [features]
//!  default = ["custom-heap"]
//!  custom-heap = []
//!  ```
//!
//!  3. Put this in your entrypoint.rs
//! ```
//! // START: Heap start
//! // LENGTH: Heap length
//! // MIN: Minimal allocation size
//! // PAGE_SIZE: Allocation page size
//! #[cfg(target_os = "solana")]
//! #[global_allocator]
//! static ALLOC: Smalloc<{ HEAP_START_ADDRESS as usize }, { HEAP_LENGTH as usize }, 16, 1024> =
//!     Smalloc::new();
//! ```
//!
//! Note: The "dynamic_start" feature is for unit tests.
//!

use solana_program::msg;
use std::{alloc::Layout, cmp, mem::size_of, ptr, ptr::null_mut, usize};

pub struct Smalloc<
    const START: usize,
    const LENGTH: usize,
    const MIN: usize,
    const PAGE_SIZE: usize,
> {
    #[cfg(feature = "dynamic_start")]
    start: usize,
}

impl<const START: usize, const LENGTH: usize, const MIN: usize, const PAGE_SIZE: usize>
    Smalloc<START, LENGTH, MIN, PAGE_SIZE>
{
    #[cfg(not(feature = "dynamic_start"))]
    pub const fn new() -> Smalloc<START, LENGTH, MIN, PAGE_SIZE> {
        checks(START, LENGTH, MIN, PAGE_SIZE);
        Smalloc {}
    }

    #[cfg(feature = "dynamic_start")]
    pub fn new(start: usize) -> Smalloc<START, LENGTH, MIN, PAGE_SIZE> {
        checks(start, LENGTH, MIN, PAGE_SIZE);
        Smalloc { start }
    }

    #[inline]
    fn get_start(&self) -> usize {
        #[cfg(feature = "dynamic_start")]
        let start = self.start;
        #[cfg(not(feature = "dynamic_start"))]
        let start = START;

        start
    }
}

unsafe impl<const START: usize, const LENGTH: usize, const MIN: usize, const PAGE_SIZE: usize>
    std::alloc::GlobalAlloc for Smalloc<START, LENGTH, MIN, PAGE_SIZE>
{
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let start = self.get_start();
        let p = start as *mut usize;
        let inner = Inner::<LENGTH, MIN, PAGE_SIZE> { p };

        if inner.start() == 0 {
            inner.init(self.get_start());
        }

        let level = inner.size_level(layout.size());
        if level < Inner::<LENGTH, MIN, PAGE_SIZE>::free_list_count() {
            let head = inner.free_list(level);
            // There are free node on the free list
            if *head != 0 {
                Inner::<LENGTH, MIN, PAGE_SIZE>::remove_free0(head)
            } else {
                // Try allocate new page
                inner.alloc_page_for_small(level)
            }
        } else {
            inner.alloc_n_page_for_big(layout.size())
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if !ptr.is_null() {
            let inner = Inner::<LENGTH, MIN, PAGE_SIZE> {
                p: self.get_start() as *mut usize,
            };
            inner.dealloc(ptr, layout.size());
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let inner = Inner::<LENGTH, MIN, PAGE_SIZE> {
            p: self.get_start() as *mut usize,
        };
        // The level is not changed, no need to realloc
        let level = inner.size_level(layout.size());

        if level < Inner::<LENGTH, MIN, PAGE_SIZE>::free_list_count()
            && level == inner.size_level(new_size)
        {
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

const FREE_LIST_ADDR: usize = 1;

struct PageHeader {
    pub used: bool,
}

struct Inner<const LENGTH: usize, const MIN: usize, const PAGE_SIZE: usize> {
    p: *mut usize,
}

impl<const LENGTH: usize, const MIN: usize, const PAGE_SIZE: usize> Inner<LENGTH, MIN, PAGE_SIZE> {
    const fn page_size() -> usize {
        PAGE_SIZE
    }

    const fn page_count() -> usize {
        LENGTH / PAGE_SIZE
    }

    /// log2(min_alloc_size)
    const fn log2_min() -> usize {
        round_up_log2(MIN as u32)
    }

    /// Max sub-page-memory level
    const fn free_list_count() -> usize {
        round_up_log2(PAGE_SIZE as u32) - Self::log2_min()
    }

    #[inline]
    fn start(&self) -> usize {
        self.nth_val(0)
    }

    /// The linked lists of free memory for each size level
    #[inline]
    fn free_list(&self, level: usize) -> *mut usize {
        self.nth_ptr(FREE_LIST_ADDR + level)
    }

    #[inline]
    fn page_header(&self, index: usize) -> *mut PageHeader {
        // Right after free list
        let begin_addr = self.free_list(Self::free_list_count()) as *mut PageHeader;
        unsafe { begin_addr.add(index) }
    }

    #[inline]
    fn nth_ptr(&self, n: usize) -> *mut usize {
        unsafe { self.p.add(n) }
    }

    #[inline]
    fn nth_val(&self, n: usize) -> usize {
        unsafe { *self.nth_ptr(n) }
    }

    /// Init inner structure, we assume the memory is zeroed
    #[inline]
    unsafe fn init(&self, start: usize) {
        *self.nth_ptr(0) = start;
        // Allocate the first page and spare space for the Inner
        self.alloc_page_for_small(self.size_level(MIN));

        // Write start again, as it was overwritten by alloc_page_for_small
        *self.nth_ptr(0) = start;
        // Init free-list-heads
        for i in 0..Self::free_list_count() {
            *self.free_list(i) = 0;
        }
        // Set the first free memory block
        let mut inner_data_end = self.page_header(Self::page_count()) as usize;

        if inner_data_end % MIN != 0 {
            inner_data_end += MIN - (inner_data_end % MIN);
        }
        dbg!(self.start() as *mut u8);
        assert!(inner_data_end < self.start() + Self::page_size());
        *self.free_list(0) = inner_data_end;
    }

    #[inline]
    unsafe fn alloc_page_for_small(&self, level: usize) -> *mut u8 {
        let size = 1 << (level + Inner::<LENGTH, MIN, PAGE_SIZE>::log2_min());
        let mut page_index = None;
        for i in 0..Self::page_count() {
            let header = self.page_header(i).as_mut().unwrap();
            if !header.used {
                page_index = Some(i);
                header.used = true;
                break;
            }
        }

        match page_index {
            Some(i) => {
                let page_start = self.start() + i * Self::page_size();
                let count = Self::page_size() / size;
                for i in 0..(count - 1) {
                    let p = (page_start + (size * i)) as *mut usize;
                    *p = page_start + (size * (i + 1));
                }
                let ptr_end = (page_start + (count - 1) * size) as *mut usize;
                *ptr_end = 0;

                let head = self.free_list(level);
                *head = page_start;

                Self::remove_free0(head)
            }
            None => null_mut(),
        }
    }

    #[inline]
    fn alloc_n_page_for_big(&self, size: usize) -> *mut u8 {
        let n = Self::need_page_count(size);
        // Brute force search. TODO: optimize
        unsafe {
            let first_posible_begin = Self::page_count() - n;
            for i in 0..(Self::page_count() - n) {
                let begin = first_posible_begin - i;
                let mut ok = true;
                for j in 0..n {
                    if (*self.page_header(begin + j)).used {
                        ok = false;
                        break;
                    }
                }
                if ok {
                    for j in 0..n {
                        let header = &mut (*self.page_header(begin + j));
                        header.used = true;
                    }
                    return (self.start() + begin * Self::page_size()) as *mut u8;
                }
            }
        }

        null_mut()
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, size: usize) {
        let level = self.size_level(size);
        if level < Inner::<LENGTH, MIN, PAGE_SIZE>::free_list_count() {
            self.insert_free(ptr, level);
        } else {
            let loc = ptr as usize - self.start();
            assert!(loc % Self::page_size() == 0);
            let index = loc / Self::page_size();
            let n = Self::need_page_count(size);
            for i in index..(index + n) {
                (*self.page_header(i)).used = false;
            }
        }
    }

    #[inline]
    fn need_page_count(size: usize) -> usize {
        if size % Self::page_size() == 0 {
            size / Self::page_size()
        } else {
            size / Self::page_size() + 1
        }
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
        let log2_min = Self::log2_min();
        cmp::max(log2, log2_min) - log2_min
    }
}

const fn checks(start: usize, length: usize, min: usize, page_size: usize) {
    // 0 is used as a marker
    assert!(start > 0);
    // support size up to u32::MAX
    assert!(page_size <= u32::MAX as usize);
    assert!(page_size < length && length % page_size == 0);
    // min & max have to be power of 2

    assert!(min >= size_of::<usize>());
    assert!(round_up_to_power2(min as u32) == min as u32);
    assert!(min <= page_size && page_size % min == 0);
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
            const length: usize = 256 * 1024;
            const min: usize = 16;
            const page_size: usize = 2048;
            let mem = std::alloc::alloc(Layout::from_size_align(512 * 1024, page_size).unwrap());
            let start = mem as usize;
            let end = mem.add(length);

            dbg!(super::round_up_log2(min as u32));

            let max_level = max_level(min, page_size);
            let real_star_8 = (start + (super::FREE_LIST_ADDR + max_level + 1) * min) as *mut u8;
            dbg!(mem, end, max_level, real_star_8);

            let a = Smalloc::<0, length, min, page_size>::new(start);
            let lo = Layout::from_size_align(16, 8).unwrap();
            for i in 0..2 {
                dbg!(i);
                let big_layout = Layout::from_size_align(2 * 1024, 8).unwrap();

                let big = a.alloc(big_layout);
                let big2 = a.alloc(big_layout);
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
                a.dealloc(big, big_layout);
                a.dealloc(big2, big_layout);
                //let pp = a.realloc(p, lo, 64);
                dbg!(p, p2, p3, p4, p5, p6, p7, p8, p9, p10, big, big2);
            }
        }
    }
}
