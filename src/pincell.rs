use std::{
    cell::UnsafeCell,
    marker::{PhantomData, PhantomPinned},
    ptr::NonNull, sync::atomic::{AtomicBool, Ordering},
};

use cordyceps::Linked;
use mutex::{BlockingMutex, ScopedRawMutex};

pub struct PinCellInner<T, U> {
    /// Morally equivalent to `Option<Pin<&mut U>>`
    p: Option<NonNull<U>>,
    /// This is the HEAD of the list. It points to all PinCellWeak instances
    /// with shared access to the T. If the T is to be dropped, it must FIRST
    /// walk this singly linked list, and mark all PinCellWeaks as !valid, before
    /// the T is dropped.
    head: Links<T>,
}

pub struct PinCell<R: ScopedRawMutex, T: Linked<Links<T>>, U> {
    inner: BlockingMutex<R, PinCellInner<T, U>>,
}

pub struct Links<T> {
    /// The next node in the queue.
    pub(crate) next: UnsafeCell<Option<NonNull<T>>>,

    /// Have we been invalidated? TODO: it would be extremely funny to
    /// do this with pointer tagging of low bits in the `next` field
    pub(crate) valid: AtomicBool,

    /// Linked list links must always be `!Unpin`, in order to ensure that they
    /// never recieve LLVM `noalias` annotations; see also
    /// <https://github.com/rust-lang/rust/issues/63818>.
    _unpin: PhantomPinned,
}

pub struct PinCellWeak<'a, R: ScopedRawMutex, T: Linked<Links<T>>, U> {
    pc: &'a PinCell<R, T, U>,
    links: Links<T>,
}

impl<R, T, U> PinCellWeak<'_, R, T, U>
where
    R: ScopedRawMutex,
    T: Linked<Links<T>>,
{
    // todo: this probably needs to be self: Pin<&Self>?
    pub fn with<V, F: FnOnce(&U) -> V>(&self, f: F) -> Option<V> {
        self.pc.inner.with_lock(|inner| {
            // Now that we have the lock, see if we are still in the chain
            let linked = self.links.valid.load(Ordering::Relaxed);
            if !linked {
                return None;
            }
            // We are still in the chain, access to T is still valid
            let t = unsafe {
                // todo: if the chain is live then p can never be null,
                // make this a normal bare pointer? checked in debug?
                &*inner.p.unwrap_unchecked().as_ptr()
            };
            Some(f(t))
        })
    }

    pub fn with_mut<V, F: FnOnce(&mut U) -> V>(&self, f: F) -> Option<V> {
        self.pc.inner.with_lock(|inner| {
            // Now that we have the lock, see if we are still in the chain
            let linked = self.links.valid.load(Ordering::Relaxed);
            if !linked {
                return None;
            }
            // We are still in the chain, access to T is still valid
            let t = unsafe {
                // todo: if the chain is live then p can never be null,
                // make this a normal bare pointer? checked in debug?
                &mut *inner.p.unwrap_unchecked().as_ptr()
            };
            Some(f(t))
        })
    }
}

// TODO: Implementing drop for PinCellWeak is going to be funny because we need to
// take OUR .next, and if it is some, put it at the front of the list. This probably
// requires some O(n) traversal. The alternative is making this a doubly linked list
// and doing a normal node removal
//
// What does all this extra guff about Weak get us? We have to pin the Weak ref, which
// is possible, but is verbose in embedded context. The one thing it really gets us is
// that we can be sure that we're referencing the SAME U across multiple calls, and that
// we know if the thing has been removed and another re-added. It seems like a lot for
// that kind of value. We could also stick a waker in the Links and wake on eviction?
