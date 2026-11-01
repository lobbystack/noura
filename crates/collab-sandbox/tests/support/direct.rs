//! The allocator of the tests that measure memory: blocks of 1 MiB or more
//! come straight from the system and go straight back, so the process's
//! memory counts what the test holds. The system allocator keeps large
//! freed blocks for reuse, which would hide whether memory came back.
//!
//! A test binary that measures memory declares it its global allocator.

use std::alloc::{GlobalAlloc, Layout, System};

const LARGE: usize = 1 << 20;
const PAGE: usize = 4096;

pub struct Direct;

fn large(layout: Layout) -> bool {
    layout.size() >= LARGE && layout.align() <= PAGE
}

// SAFETY: large blocks come from mmap, which returns page-aligned,
// zeroed memory or MAP_FAILED, and go back to munmap with the length
// they were mapped with; the rest goes to the system allocator.
unsafe impl GlobalAlloc for Direct {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if !large(layout) {
            return unsafe { System.alloc(layout) };
        }
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                layout.size(),
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            std::ptr::null_mut()
        } else {
            ptr.cast()
        }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if large(layout) {
            unsafe { self.alloc(layout) }
        } else {
            unsafe { System.alloc_zeroed(layout) }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if large(layout) {
            unsafe { libc::munmap(ptr.cast(), layout.size()) };
        } else {
            unsafe { System.dealloc(ptr, layout) }
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_layout = unsafe { Layout::from_size_align_unchecked(new_size, layout.align()) };
        if !large(layout) && !large(new_layout) {
            return unsafe { System.realloc(ptr, layout, new_size) };
        }
        let new = unsafe { self.alloc(new_layout) };
        if !new.is_null() {
            unsafe {
                std::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new
    }
}
