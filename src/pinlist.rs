use cordyceps::list::Links;
use cordyceps::{Linked, List, list};
use mutex::ConstInit;
use mutex::{BlockingMutex, ScopedRawMutex};
use pin_project::{pin_project, pinned_drop};
use std::marker::PhantomPinned;
use std::ptr::addr_of_mut;
use std::{
    cell::UnsafeCell,
    pin::Pin,
    ptr::{self, NonNull},
};

pub struct PinList<T, R: ScopedRawMutex> {
    list: BlockingMutex<R, List<PinListNodeInner<T>>>,
}

impl<T, R: ScopedRawMutex + ConstInit> PinList<T, R> {
    pub const fn new() -> Self {
        Self {
            list: BlockingMutex::new(List::new()),
        }
    }
}

impl<T, R: ScopedRawMutex> PinList<T, R> {
    pub fn search_mut<U, F: Fn(Pin<&mut T>) -> Option<U>>(&self, f: F) -> Option<U> {
        self.list.with_lock(|ls| {
            let iter = ls.iter_mut();
            for i in iter {
                let i: Pin<&mut PinListNodeInner<T>> = i;
                let i: *mut Node<T> = i.node.get();
                // SAFETY: Items must be pinned while they are in the list, and while we hold
                // the lock, no items can be removed from the list.
                let t: &mut T = unsafe { &mut *addr_of_mut!((*i).t) };
                let t: Pin<&mut T> = unsafe { Pin::new_unchecked(t) };
                let res = f(t);
                if res.is_some() {
                    return res;
                }
            }
            None
        })
    }
}

impl<T, R: ScopedRawMutex + ConstInit> Default for PinList<T, R> {
    fn default() -> Self {
        Self::new()
    }
}

// this is the equivalent of Node
#[pin_project]
pub struct Node<T> {
    links: list::Links<PinListNodeInner<T>>,
    #[pin]
    t: T,
    // This type is !Unpin due to the heuristic from:
    // <https://github.com/rust-lang/rust/pull/82834>
    _pin: PhantomPinned,
}

// This is the equivalent of Waiter
pub struct PinListNodeInner<T> {
    node: UnsafeCell<Node<T>>,
}

impl<T> PinListNodeInner<T> {
    pub fn insert_into<R: ScopedRawMutex>(mut self: Pin<&mut Self>, list: &PinList<T, R>) {
        list.list.with_lock(|ls| {
            let ptr = unsafe { NonNull::from(Pin::into_inner_unchecked(self.as_mut())) };
            ls.push_front(ptr);
        })
    }
}

// This is the equivalent of Wait
#[pin_project(PinnedDrop)]
pub struct PinListNode<'a, T, R: ScopedRawMutex> {
    list: &'a PinList<T, R>,

    #[pin]
    inner: PinListNodeInner<T>,
}

impl<'a, T, R: ScopedRawMutex> PinListNode<'a, T, R> {
    pub fn new_for_list(list: &'a PinList<T, R>, t: T) -> Self {
        Self {
            list,
            inner: PinListNodeInner { node: UnsafeCell::new(Node {
                links: Links::new(),
                t,
                _pin: PhantomPinned,
            }) },
        }
    }

    pub fn attach(self: Pin<&mut Self>) {
        let this = self.project();
        this.inner.insert_into(this.list);
    }
}

impl<T, R: ScopedRawMutex> PinListNode<'_, T, R> {
    pub fn insert(self: Pin<&mut Self>) {
        let this = self.project();
        this.inner.insert_into(this.list);
    }
}

#[pinned_drop]
impl<T, R: ScopedRawMutex> PinnedDrop for PinListNode<'_, T, R> {
    fn drop(mut self: Pin<&mut Self>) {
        let mut this = self.project();
        this.list.list.with_lock(|ls| {
            let ptr = unsafe { NonNull::from(Pin::into_inner_unchecked(this.inner.as_mut())) };
            unsafe { ls.remove(ptr) };
        })
    }
}

unsafe impl<T> Linked<list::Links<PinListNodeInner<T>>> for PinListNodeInner<T> {
    type Handle = NonNull<PinListNodeInner<T>>;

    fn into_ptr(r: Self::Handle) -> NonNull<Self> {
        r
    }

    unsafe fn from_ptr(ptr: NonNull<Self>) -> Self::Handle {
        ptr
    }

    unsafe fn links(target: NonNull<Self>) -> NonNull<list::Links<PinListNodeInner<T>>> {
        // Safety: using `ptr::addr_of!` avoids creating a temporary
        // reference, which stacked borrows dislikes.
        let node: *const UnsafeCell<Node<T>> = unsafe { ptr::addr_of!((*target.as_ptr()).node) };

        // From WaitQueue, using its UnsafeCell
        // (*node).with_mut(|node| {
        //     let links = ptr::addr_of_mut!((*node).links);
        //     // Safety: since the `target` pointer is `NonNull`, we can assume
        //     // that pointers to its members are also not null, making this use
        //     // of `new_unchecked` fine.
        //     NonNull::new_unchecked(links)
        // })

        // AJM: reimpl
        let node: *mut Node<T> = unsafe { (*node).get() };
        let links = unsafe { ptr::addr_of_mut!((*node).links) };
        // Safety: since the `target` pointer is `NonNull`, we can assume
        // that pointers to its members are also not null, making this use
        // of `new_unchecked` fine.
        unsafe { NonNull::new_unchecked(links) }
    }
}

#[cfg(test)]
mod test {
    use std::pin::pin;

    use crate::pinlist::{PinList, PinListNode};

    #[test]
    fn smoke() {
        static LIST: PinList<u64, mutex::raw_impls::cs::CriticalSectionRawMutex> = PinList::new();
        let one = PinListNode::new_for_list(&LIST, 1);
        let two = PinListNode::new_for_list(&LIST, 2);
        let three = PinListNode::new_for_list(&LIST, 3);
        let one = pin!(one);
        let two = pin!(two);
        let three = pin!(three);
        one.attach();
        two.attach();
        three.attach();

        for x in 1..=3 {
            let found = LIST.search_mut(|i| {
                if *i.as_ref() == x {
                    Some(true)
                } else {
                    None
                }
            });
            assert_eq!(found, Some(true));
        }
        for x in 4..=6 {
            let found = LIST.search_mut(|i| {
                if *i.as_ref() == x {
                    Some(true)
                } else {
                    None
                }
            });
            assert_eq!(found, None);
        }
    }
}
