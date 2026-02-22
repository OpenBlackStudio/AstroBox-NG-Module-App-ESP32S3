use core::{
    alloc::{GlobalAlloc, Layout},
    ptr,
};
use esp_idf_svc::sys::{
    heap_caps_aligned_alloc, heap_caps_free, heap_caps_malloc, MALLOC_CAP_8BIT,
    MALLOC_CAP_INTERNAL, MALLOC_CAP_SPIRAM,
};

pub struct PsramFirstAllocator;

const PSRAM_CAPS: u32 = (MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT) as u32;
const INTERNAL_CAPS: u32 = (MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT) as u32;
const DEFAULT_ALIGNMENT: usize = core::mem::size_of::<usize>();
const SMALL_ALLOC_INTERNAL_THRESHOLD: usize = 256;

impl PsramFirstAllocator {
    #[inline(always)]
    fn non_null_for_zero(layout: &Layout) -> *mut u8 {
        layout.align() as *mut u8
    }

    #[inline(always)]
    fn size_for_alignment(layout: &Layout) -> Option<usize> {
        let size = layout.size().max(1);
        if layout.align() <= DEFAULT_ALIGNMENT {
            Some(size)
        } else {
            align_up(size, layout.align())
        }
    }

    #[inline(always)]
    unsafe fn alloc_with_caps(layout: &Layout, caps: u32) -> *mut u8 {
        let Some(size) = Self::size_for_alignment(layout) else {
            return ptr::null_mut();
        };

        if layout.align() <= DEFAULT_ALIGNMENT {
            heap_caps_malloc(size, caps) as *mut u8
        } else {
            heap_caps_aligned_alloc(layout.align(), size, caps) as *mut u8
        }
    }
}

unsafe impl GlobalAlloc for PsramFirstAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.size() == 0 {
            return Self::non_null_for_zero(&layout);
        }

        // Keep small control objects in internal RAM to reduce cache-related faults on ESP32-S3.
        if layout.size() <= SMALL_ALLOC_INTERNAL_THRESHOLD {
            let ptr = Self::alloc_with_caps(&layout, INTERNAL_CAPS);
            if !ptr.is_null() {
                return ptr;
            }
            return Self::alloc_with_caps(&layout, PSRAM_CAPS);
        }

        let ptr = Self::alloc_with_caps(&layout, PSRAM_CAPS);
        if !ptr.is_null() {
            return ptr;
        }

        Self::alloc_with_caps(&layout, INTERNAL_CAPS)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() || layout.size() == 0 {
            return;
        }
        heap_caps_free(ptr.cast());
    }
}

#[inline(always)]
fn align_up(value: usize, align: usize) -> Option<usize> {
    debug_assert!(align.is_power_of_two());
    let mask = align - 1;
    value.checked_add(mask).map(|sum| sum & !mask)
}

#[global_allocator]
static GLOBAL_ALLOCATOR: PsramFirstAllocator = PsramFirstAllocator;
