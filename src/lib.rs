use std::{alloc::Layout, mem::size_of, ptr::null_mut, usize};
///

/// Minimal allocation size = 8 bytes
const LOG2_MIN_BLOCK: usize = 3;

pub struct Smalloc<
    const START: usize,
    const LENGTH: usize,
    const LEVELS: usize,
    const PAGE_SIZE: usize,
> {
    #[cfg(feature = "with_static")]
    start: usize,
    #[cfg(feature = "with_static")]
    length: usize,
    #[cfg(feature = "with_static")]
    levels: usize,
    #[cfg(feature = "with_static")]
    page_size: usize,
}

impl<const START: usize, const LENGTH: usize, const LEVELS: usize, const PAGE_SIZE: usize>
    Smalloc<START, LENGTH, LEVELS, PAGE_SIZE>
{
    #[cfg(feature = "with_static")]
    pub fn new(
        start: usize,
        length: usize,
        levels: usize,
        page_size: usize,
    ) -> Smalloc<START, LENGTH, LEVELS, PAGE_SIZE> {
        Smalloc {
            start,
            length,
            levels,
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

unsafe impl<const START: usize, const LENGTH: usize, const LEVELS: usize, const PAGE_SIZE: usize>
    std::alloc::GlobalAlloc for Smalloc<START, LENGTH, LEVELS, PAGE_SIZE>
{
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let start = self.get_start();
        let p = start as *mut usize;
        let inner = Inner { p };

        if inner.start() == 0 {
            dbg!("init");
            #[cfg(feature = "with_static")]
            let (start, length, levels, page_size) =
                (self.start, self.length, self.levels, self.page_size);
            #[cfg(not(feature = "with_static"))]
            let (start, length, levels, page_size) = (START, LENGTH, LEVELS, PAGE_SIZE);
            inner.init(start, length, levels, page_size)
        }

        let level = Inner::size_level(layout.size());
        if level > inner.levels() {
            return null_mut();
        }

        let head = inner.free_list(level);
        dbg!(head, *head);
        // There are free node on the free list
        if *head != 0 {
            return Inner::remove_free0(head);
        }

        // Try allocate new page
        dbg!(inner.page_used(), inner.page_count());
        if inner.page_used() < inner.page_count() {
            let page_start = inner.start() + inner.page_size() * inner.page_used();
            inner.alloc_page(page_start, 1 << (level + LOG2_MIN_BLOCK));
            let head = inner.free_list(level);
            *head = page_start;
            return Inner::remove_free0(head);
        }

        null_mut()
    }

    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        if p as usize != 0 {
            let inner = Inner {
                p: self.get_start() as *mut usize,
            };
            inner.insert_free(p, Inner::size_level(layout.size()));
        }
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

    #[inline]
    fn levels(&self) -> usize {
        self.nth_val(1)
    }

    #[inline]
    fn page_size(&self) -> usize {
        self.nth_val(2)
    }

    #[inline]
    fn page_count(&self) -> usize {
        self.nth_val(3)
    }

    #[inline]
    fn page_used(&self) -> usize {
        self.nth_val(4)
    }

    #[inline]
    fn page_used_inc(&self) {
        unsafe {
            *self.nth_ptr(4) = self.page_used() + 1;
        }
    }

    #[inline]
    fn free_list(&self, level: usize) -> *mut usize {
        self.nth_ptr(5 + level)
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
    unsafe fn init(&self, start: usize, length: usize, levels: usize, page_size: usize) {
        // 0 is used as a marker
        assert!(start > 0);
        // support size up to u32::MAX
        assert!(page_size <= u32::MAX as usize);
        assert!(page_size < length && length % page_size == 0);
        let max_block: usize = 1 << (LOG2_MIN_BLOCK + levels - 1);
        assert!(max_block <= page_size && page_size % max_block == 0);

        *self.nth_ptr(0) = start;
        *self.nth_ptr(1) = levels;
        *self.nth_ptr(2) = page_size;
        *self.nth_ptr(3) = length / page_size;
        *self.nth_ptr(4) = 0;

        const WORD_SIZE: usize = size_of::<usize>();
        let start = self.start() + (5 + self.levels()) * WORD_SIZE;
        // Allocate the first page for size_level(size_of(usize)),
        // and spare space for the Inner
        self.alloc_page(start, WORD_SIZE);

        // init free-list-heads
        for i in 0..self.levels() {
            *self.free_list(i) = 0;
        }
        *self.free_list(Inner::size_level(WORD_SIZE)) = start;
    }

    #[inline]
    unsafe fn alloc_page(&self, page_start: usize, block_size: usize) {
        let count = self.page_size() / block_size;
        for i in 0..(count - 1) {
            let p = (page_start + (block_size * i)) as *mut usize;
            *p = page_start + (block_size * (i + 1));
        }
        let ptr_end = (page_start + self.page_size() - block_size) as *mut usize;
        *ptr_end = 0;

        self.page_used_inc()
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
    fn size_level(size: usize) -> usize {
        round_up_log2(size as u32) - LOG2_MIN_BLOCK
    }
}

/// Get log2(size) and round up to LOG2_MIN_BLOCK
#[inline]
fn round_up_log2(size: u32) -> usize {
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
            3, 3, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
        ];
        for (i, &s) in sizes.iter().enumerate() {
            assert_eq!(super::round_up_log2(s), logs[i]);
        }
    }

    #[test]
    fn test_alloc() {
        unsafe {
            let mem = std::alloc::alloc(Layout::from_size_align(512 * 1024, 8).unwrap());
            let start = mem as usize;
            let length = 2 * 1024;
            let levels = 8;
            let page_size = 1024;

            let real_star_8 = (start + (5 + levels) * 8) as *mut u8;
            dbg!(real_star_8);

            let a = Smalloc::<0, 0, 0, 0>::new(start, length, levels, page_size);
            let lo = Layout::from_size_align(512, 8).unwrap();
            for i in 0..10 {
                let p = a.alloc(lo);
                let p2 = a.alloc(lo);
                let p3 = a.alloc(lo);
                let p4 = a.alloc(lo);
                let p5 = a.alloc(lo);
                a.dealloc(p3, lo);
                a.dealloc(p4, lo);
                let p6 = a.alloc(lo);
                let p7 = a.alloc(lo);
                let p8 = a.alloc(lo);
                dbg!(mem, p, p2, p3, p4, p5, p6, p7, p8);
            }
        }
    }
}
