use std::sync::atomic::{AtomicUsize, Ordering};
use std::{alloc::Layout, mem::size_of, ptr::null_mut, usize};

///

/// Minimal allocation size = 8 bytes
const LOG2_MIN_BLOCK: usize = 3;

macro_rules! new_smalloc {
    ($start:expr, $length:expr, $levels:expr, $page_size:expr) => {{
        // 0 is used as a marker
        assert!($start > 0);
        // support size up to u32::MAX
        assert!($page_size <= u32::MAX as usize);
        assert!($page_size < $length && $length % $page_size == 0);

        let max_block: usize = 1 << (LOG2_MIN_BLOCK + $levels - 1);
        assert!(max_block <= $page_size && $page_size % max_block == 0);
        Smalloc {
            start: $start,
            levels: $levels,
            page_size: $page_size,
            page_count: $length / $page_size,
            page_used: AtomicUsize::new(0),
        }
    }};
}

pub struct Smalloc<
    const START: usize,
    const LENGTH: usize,
    const LEVELS: usize,
    const PAGE_SIZE: usize,
> {
    start: usize,
    levels: usize,
    page_size: usize,
    page_count: usize,
    page_used: AtomicUsize,
}

impl<const START: usize, const LENGTH: usize, const LEVELS: usize, const PAGE_SIZE: usize>
    Smalloc<START, LENGTH, LEVELS, PAGE_SIZE>
{
    pub const fn const_new() -> Smalloc<START, LENGTH, LEVELS, PAGE_SIZE> {
        new_smalloc!(START, LENGTH, LEVELS, PAGE_SIZE)
    }

    pub fn new(
        start: usize,
        length: usize,
        levels: usize,
        page_size: usize,
    ) -> Smalloc<START, LENGTH, LEVELS, PAGE_SIZE> {
        new_smalloc!(start, length, levels, page_size)
    }

    #[inline]
    fn log2(size: u32) -> usize {
        const MULTIPLY_DE_BRUIJN_BIT_POSITION: [usize; 32] = [
            LOG2_MIN_BLOCK, //0
            if 1 > LOG2_MIN_BLOCK {
                1 as usize
            } else {
                LOG2_MIN_BLOCK
            },
            16,
            if 2 > LOG2_MIN_BLOCK {
                2 as usize
            } else {
                LOG2_MIN_BLOCK
            },
            29,
            17,
            3,
            22,
            30,
            20,
            18,
            11,
            13,
            4,
            7,
            23,
            31,
            15,
            28,
            21,
            19,
            10,
            12,
            6,
            14,
            27,
            9,
            5,
            26,
            8,
            25,
            24,
        ];
        // first round up to power of 2
        let mut v = size as u32;
        v -= 1;
        v |= v >> 1;
        v |= v >> 2;
        v |= v >> 4;
        v |= v >> 8;
        v |= v >> 16;
        v += 1;
        const MAGIC: u32 = 0x06EB14F9u32;
        MULTIPLY_DE_BRUIJN_BIT_POSITION[(v.overflowing_mul(MAGIC).0 >> 27) as usize]
    }

    #[inline]
    unsafe fn alloc_page(&self, page_start: usize, block_size: usize) {
        let count = self.page_size / block_size;
        for i in 0..(count - 1) {
            let p = (page_start + (block_size * i)) as *mut usize;
            *p = page_start + (block_size * (i + 1));
        }
        let ptr_end = (page_start + self.page_size - block_size) as *mut usize;
        *ptr_end = 0;

        self.page_used.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    unsafe fn init_first_page(&self) {
        const WORD_SIZE: usize = size_of::<usize>();
        // Allocate the first page with block size as size_of usize
        self.alloc_page(self.start, WORD_SIZE);

        // Store free-list-head at the beginning of the page, and init them as 0
        for i in 0..self.levels {
            *((self.start as *mut usize).add(i)) = 0;
        }

        let level = Self::level(WORD_SIZE);
        *self.get_free_list(level) = self.start + self.levels * WORD_SIZE;
    }

    #[inline]
    unsafe fn remove_free0(head: *mut usize) -> *mut u8 {
        let free0 = *head;
        let free0_next = *(free0 as *mut usize);
        *head = free0_next;
        free0 as *mut u8
    }

    /// The first LEVELS blocks are used to store free-list
    #[inline]
    unsafe fn get_free_list(&self, level: usize) -> *mut usize {
        (self.start as *mut usize).add(level)
    }

    #[inline]
    fn page_used(&self) -> usize {
        self.page_used.load(Ordering::Relaxed)
    }

    #[inline]
    fn level(size: usize) -> usize {
        Self::log2(size as u32) - LOG2_MIN_BLOCK
    }
}

unsafe impl<const START: usize, const LENGTH: usize, const LEVELS: usize, const PAGE_SIZE: usize>
    std::alloc::GlobalAlloc for Smalloc<START, LENGTH, LEVELS, PAGE_SIZE>
{
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if self.page_used() == 0 {
            self.init_first_page();
        }

        let level = Self::level(layout.size());
        if level > self.levels {
            return null_mut();
        }

        let head = self.get_free_list(level);

        // There are free node on the free list
        if *head != 0 {
            return Self::remove_free0(head);
        }

        // Try allocate new page
        if self.page_used() < self.page_count {
            let page_start = self.start + self.page_size * self.page_used();
            self.alloc_page(page_start, 1 << (level + LOG2_MIN_BLOCK));
            let head = self.get_free_list(level);
            *head = page_start;
            return Self::remove_free0(head);
        }

        null_mut()
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        let level = Self::level(layout.size());
        let head = self.get_free_list(level);
        let free0 = *head;
        *head = p as usize;
        *(p as *mut usize) = free0;
    }
}

#[cfg(test)]
mod test {
    use std::alloc::{GlobalAlloc, Layout};

    use super::Smalloc;

    #[test]
    fn test_log2() {
        let sizes = vec![
            1, 2, 5, 8, 9, 16, 20, 32, 33, 64, 65, 128, 129, 256, 257, 512, 513, 1024, 1025, 2048,
            2049, 4096, 4097,
        ];
        let logs = vec![
            3, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
        ];
        for (i, &s) in sizes.iter().enumerate() {
            assert_eq!(Smalloc::<8, 1048576, 10, 4096>::log2(s), logs[i]);
        }
    }

    #[test]
    fn test_alloc() {
        unsafe {
            let mem = std::alloc::alloc(Layout::from_size_align(512 * 1024, 8).unwrap());
            let start = mem as usize;

            let a = Smalloc::<0, 0, 0, 0>::new(start, 512 * 1024, 8, 1024);
            let lo = Layout::from_size_align(512, 8).unwrap();
            let p = a.alloc(lo);
            let p2 = a.alloc(lo);
            let p3 = a.alloc(lo);
            let p4 = a.alloc(lo);
            let p5 = a.alloc(lo);
            a.dealloc(p3, lo);
            let p6 = a.alloc(lo);
            dbg!(mem, p, p2, p3, p4, p5, p6);
        }
    }
}
