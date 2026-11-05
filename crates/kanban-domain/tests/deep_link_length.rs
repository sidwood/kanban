//! A deep link longer than any the core mints is refused on its length
//! alone, before any of its text is read: the operating system hands
//! the shell whatever text it was given, of any size. The allocator
//! below counts what the calling thread allocates, so the test sees
//! whether the refusal copied any of the link.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use kanban_domain::IssueKey;

struct CountingAllocator;

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
}

fn note(size: usize) {
    let counting = COUNTING.try_with(Cell::get).unwrap_or(false);
    if counting {
        let _ = ALLOCATED.try_with(|allocated| allocated.set(allocated.get() + size));
    }
}

// SAFETY: every call is forwarded unchanged to the system allocator;
// counting only touches const-initialised thread-locals, which never
// allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract for `alloc` is passed on as is.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: the caller's contract for `alloc_zeroed` is passed on.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size);
        // SAFETY: the caller's contract for `realloc` is passed on.
        unsafe { System.realloc(pointer, layout, new_size) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: the caller's contract for `dealloc` is passed on.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The bytes this thread allocates while `work` runs.
fn allocated_by(work: impl FnOnce()) -> usize {
    ALLOCATED.with(|allocated| allocated.set(0));
    COUNTING.with(|counting| counting.set(true));
    work();
    COUNTING.with(|counting| counting.set(false));
    ALLOCATED.with(Cell::get)
}

#[test]
fn an_oversized_deep_link_is_refused_before_it_is_read() {
    let links = [
        format!("kanban://issue/{}-1", "A".repeat(1 << 20)),
        format!("kanban://issue/CORE-{}12", "0".repeat(1 << 20)),
        format!("kanban://issue/{}-12", "core".repeat(1 << 18)),
        format!("kanban://issue/ABCDEFGH-{}0", u64::MAX),
    ];
    for link in &links {
        let mut named = None;
        let allocated = allocated_by(|| named = IssueKey::from_deep_link(link));

        assert_eq!(named, None, "a {}-byte link names no issue", link.len());
        assert_eq!(
            allocated,
            0,
            "a {}-byte link is refused without copying any of it",
            link.len()
        );
    }
}

#[test]
fn a_minted_deep_link_is_still_read() {
    let link = "kanban://issue/CORE-12";
    let mut named = None;
    let allocated = allocated_by(|| named = IssueKey::from_deep_link(link));

    assert_eq!(
        named.map(|key| key.display()),
        Some("CORE-12".to_owned()),
        "a link the core mints names its issue"
    );
    assert!(
        allocated > 0,
        "reading a link allocates, so a refusal that allocates nothing read none of it"
    );
}
