/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2021 Nils Asmussen, Barkhausen Institut
 *
 * This file is part of M3 (Microkernel-based SysteM for Heterogeneous Manycores).
 *
 * M3 is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License version 2 as
 * published by the Free Software Foundation.
 *
 * M3 is distributed in the hope that it will be useful, but
 * WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU
 * General Public License version 2 for more details.
 */

use core::fmt;
use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::boxed::Box;

/// A reference to an element in the list
pub type BoxRef<T> = NonNull<T>;

/// The trait for the list elements
pub trait BoxItem {
    /// Returns the next element
    fn next(&self) -> Option<BoxRef<Self>>;
    /// Sets the next element to `next`
    fn set_next(&mut self, next: Option<BoxRef<Self>>);

    /// Returns the previous element
    fn prev(&self) -> Option<BoxRef<Self>>;
    /// Sets the previous element to `prev`
    fn set_prev(&mut self, prev: Option<BoxRef<Self>>);
}

/// Convenience macro to implement [`BoxItem`] in the default way.
///
/// The macro expects a `$t` like:
///
/// ```
/// use core::ptr::NonNull;
/// struct Foo {
///     // ...
///     next: Option<NonNull<Foo>>,
///     prev: Option<NonNull<Foo>>,
///     // ...
/// }
/// ```
#[macro_export]
macro_rules! impl_boxitem {
    ($t:ty) => {
        impl $crate::col::BoxItem for $t {
            fn next(&self) -> Option<$crate::col::BoxRef<Self>> {
                self.next
            }

            fn set_next(&mut self, next: Option<$crate::col::BoxRef<Self>>) {
                self.next = next;
            }

            fn prev(&self) -> Option<$crate::col::BoxRef<Self>> {
                self.prev
            }

            fn set_prev(&mut self, prev: Option<$crate::col::BoxRef<Self>>) {
                self.prev = prev;
            }
        }
    };
}

/// The iterator for BoxList
pub struct BoxListIter<'a, T> {
    head: Option<BoxRef<T>>,
    marker: PhantomData<&'a T>,
}

impl<'a, T: BoxItem> Iterator for BoxListIter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        self.head.map(|item| unsafe {
            let item = &*item.as_ptr();
            self.head = item.next();
            item
        })
    }
}

/// The mutable iterator for BoxList
pub struct BoxListIterMut<'a, T: BoxItem> {
    list: &'a mut BoxList<T>,
    head: Option<BoxRef<T>>,
}

impl<'a, T: BoxItem> Iterator for BoxListIterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<&'a mut T> {
        self.head.map(|item| unsafe {
            let item = &mut *item.as_ptr();
            self.head = item.next();
            item
        })
    }
}

impl<'a, T: BoxItem> BoxListIterMut<'a, T> {
    /// Removes the current element from the list and returns it
    ///
    /// # Examples
    ///
    /// ```plain
    /// before remove: 1 2 3 4 5
    ///                  ^
    /// after remove : 1 3 4 5
    ///                ^
    /// ```
    pub fn remove(&mut self) -> Option<Box<T>> {
        match self.head {
            // if we already walked at the list-end, remove the last element
            None => self.list.pop_back(),

            // otherwise, check if there is a current (=prev) element to remove
            Some(mut head) => unsafe {
                head.as_ref().prev().map(|prev| {
                    let prev = prev.as_ptr();
                    match (*prev).prev() {
                        None => {
                            self.list.head = Some(head);
                            head.as_mut().set_prev(None);
                        },
                        Some(mut pp) => {
                            pp.as_mut().set_next(Some(head));
                            head.as_mut().set_prev(Some(pp));
                        },
                    }

                    self.list.len -= 1;
                    Box::from_raw(prev)
                })
            },
        }
    }
}

/// The owning iterator for BoxList
pub struct BoxListIntoIter<T: BoxItem> {
    list: BoxList<T>,
}

/// A doubly linked list that does not allocate nodes, which embed the user object, but directly
/// links the user objects
///
/// In consequence, BoxList leads to less heap allocations. In particular, objects can be moved
/// between lists by just changing a few pointers.
pub struct BoxList<T: BoxItem> {
    head: Option<BoxRef<T>>,
    tail: Option<BoxRef<T>>,
    len: usize,
}

impl<T: BoxItem> BoxList<T> {
    /// Creates an empty list
    pub const fn new() -> Self {
        BoxList {
            head: None,
            tail: None,
            len: 0,
        }
    }

    /// Returns the number of elements
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the list is empty
    pub fn is_empty(&self) -> bool {
        self.head.is_none()
    }

    /// Removes all elements from the list
    pub fn clear(&mut self) {
        *self = Self::new();
    }

    /// Returns a reference to the first element
    pub fn front(&self) -> Option<&T> {
        unsafe { self.head.map(|n| &(*n.as_ptr())) }
    }

    /// Returns a mutable reference to the first element
    pub fn front_mut(&mut self) -> Option<&mut T> {
        unsafe { self.head.map(|n| &mut (*n.as_ptr())) }
    }

    /// Returns a reference to the last element
    pub fn back(&self) -> Option<&T> {
        unsafe { self.tail.map(|n| &(*n.as_ptr())) }
    }

    /// Returns a mutable reference to the last element
    pub fn back_mut(&mut self) -> Option<&mut T> {
        unsafe { self.tail.map(|n| &mut (*n.as_ptr())) }
    }

    /// Returns an iterator for the list
    pub fn iter(&self) -> BoxListIter<'_, T> {
        BoxListIter {
            head: self.head,
            marker: PhantomData,
        }
    }

    /// Returns a mutable iterator for the list
    pub fn iter_mut(&mut self) -> BoxListIterMut<'_, T> {
        BoxListIterMut {
            head: self.head,
            list: self,
        }
    }

    /// Removes the first item for which `predicate` is true.
    pub fn remove_if<P>(&mut self, predicate: P) -> Option<Box<T>>
    where
        P: Fn(&T) -> bool,
    {
        let mut it = self.iter_mut();
        while let Some(v) = it.next() {
            if predicate(v) {
                return it.remove();
            }
        }
        None
    }

    /// Inserts the given element at the front of the list
    pub fn push_front(&mut self, mut item: Box<T>) {
        unsafe {
            item.set_next(self.head);
            item.set_prev(None);

            let item_ptr = Some(NonNull::new_unchecked(Box::into_raw(item)));

            match self.head {
                None => self.tail = item_ptr,
                Some(mut head) => head.as_mut().set_prev(item_ptr),
            }

            self.head = item_ptr;
            self.len += 1;
        }
    }

    /// Inserts the given element at the end of the list
    pub fn push_back(&mut self, mut item: Box<T>) {
        unsafe {
            item.set_next(None);
            item.set_prev(self.tail);

            let item_ptr = Some(NonNull::new_unchecked(Box::into_raw(item)));

            match self.tail {
                None => self.head = item_ptr,
                Some(mut tail) => tail.as_mut().set_next(item_ptr),
            }

            self.tail = item_ptr;
            self.len += 1;
        }
    }

    /// Removes the first element of the list and returns it
    pub fn pop_front(&mut self) -> Option<Box<T>> {
        self.head.map(|item| unsafe {
            let item = item.as_ptr();
            self.head = (*item).next();

            match self.head {
                None => self.tail = None,
                Some(mut head) => head.as_mut().set_prev(None),
            }

            self.len -= 1;
            Box::from_raw(item)
        })
    }

    /// Removes the last element of the list and returns it
    pub fn pop_back(&mut self) -> Option<Box<T>> {
        self.tail.map(|item| unsafe {
            let item = item.as_ptr();
            self.tail = (*item).prev();

            match self.tail {
                None => self.head = None,
                Some(mut tail) => tail.as_mut().set_next(None),
            }

            self.len -= 1;
            Box::from_raw(item)
        })
    }

    /// Moves the given element from the current position in the list to the back of the list
    ///
    /// # Safety
    ///
    /// This function assumes that the given element is part of this list
    pub unsafe fn move_to_back(&mut self, item: *mut T) {
        // already at the back? (tail is always Some, because T is in the list)
        if self.tail.unwrap().as_ptr() == item {
            return;
        }

        // remove us from the list
        match (*item).prev() {
            Some(mut p) => p.as_mut().set_next((*item).next()),
            None => self.head = (*item).next(),
        }
        // it's not at the back, so we can assume next() is Some
        (*item).next().unwrap().as_mut().set_prev((*item).prev());

        // let the current tail's next point to us
        let item_ptr = Some(NonNull::new_unchecked(item));
        self.tail.unwrap().as_mut().set_next(item_ptr);

        // add us to the end
        (*item).set_prev(self.tail);
        (*item).set_next(None);
        self.tail = item_ptr;
    }
}

impl<T: BoxItem> Drop for BoxList<T> {
    fn drop(&mut self) {
        while self.pop_front().is_some() {}
    }
}

impl<T: BoxItem> Default for BoxList<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: BoxItem + fmt::Debug> fmt::Debug for BoxList<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self).finish()
    }
}

impl<T: BoxItem> Iterator for BoxListIntoIter<T> {
    type Item = Box<T>;

    fn next(&mut self) -> Option<Box<T>> {
        self.list.pop_front()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.list.len, Some(self.list.len))
    }
}

impl<T: BoxItem> IntoIterator for BoxList<T> {
    type IntoIter = BoxListIntoIter<T>;
    type Item = Box<T>;

    fn into_iter(self) -> BoxListIntoIter<T> {
        BoxListIntoIter { list: self }
    }
}

impl<'a, T: BoxItem> IntoIterator for &'a BoxList<T> {
    type IntoIter = BoxListIter<'a, T>;
    type Item = &'a T;

    fn into_iter(self) -> BoxListIter<'a, T> {
        self.iter()
    }
}

impl<'a, T: BoxItem> IntoIterator for &'a mut BoxList<T> {
    type IntoIter = BoxListIterMut<'a, T>;
    type Item = &'a mut T;

    fn into_iter(self) -> BoxListIterMut<'a, T> {
        self.iter_mut()
    }
}

impl<T: BoxItem + PartialEq> PartialEq for BoxList<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other)
    }
}

impl<T: BoxItem + Eq> Eq for BoxList<T> {
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::fmt;

    struct TestItem {
        data: u32,
        prev: Option<BoxRef<TestItem>>,
        next: Option<BoxRef<TestItem>>,
    }

    impl_boxitem!(TestItem);

    impl PartialEq for TestItem {
        fn eq(&self, other: &TestItem) -> bool {
            self.data == other.data
        }
    }

    impl fmt::Debug for TestItem {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "data={}", self.data)
        }
    }

    impl TestItem {
        pub fn new(data: u32) -> Self {
            TestItem {
                data,
                prev: None,
                next: None,
            }
        }
    }

    fn gen_list(items: &[u32]) -> BoxList<TestItem> {
        let mut l: BoxList<TestItem> = BoxList::new();
        for i in items {
            l.push_back(Box::new(TestItem::new(*i)));
        }
        l
    }

    #[test]
    fn create() {
        let l: BoxList<TestItem> = BoxList::new();
        assert_eq!(l.len(), 0);
        assert_eq!(l.iter().next(), None);

        let empty = BoxList::<TestItem>::default();
        assert_eq!(empty.len(), 0);
    }

    #[test]
    fn basics() {
        let mut l = gen_list(&[23, 42, 57]);

        assert_eq!(l.len(), 3);
        assert_eq!(l.front().unwrap().data, 23);
        assert_eq!(l.back().unwrap().data, 57);

        assert_eq!(l.front_mut().unwrap().data, 23);
        assert_eq!(l.back_mut().unwrap().data, 57);
    }

    #[test]
    fn iter() {
        use crate::col::Vec;

        let mut l: BoxList<TestItem> = gen_list(&[23, 42, 57]);

        {
            let mut it = l.iter_mut();
            let e = it.next().unwrap();
            assert_eq!(e.data, 23);
            e.data = 32;

            let e = it.next().unwrap();
            assert_eq!(e.data, 42);
            e.data = 24;

            let e = it.next().unwrap();
            assert_eq!(e.data, 57);
            e.data = 75;
        }

        assert_eq!(l, gen_list(&[32, 24, 75]));

        {
            let elems = l.into_iter().collect::<Vec<_>>();
            let mut it = elems.into_iter();
            assert_eq!(it.next().unwrap().data, 32);
            assert_eq!(it.next().unwrap().data, 24);
            assert_eq!(it.next().unwrap().data, 75);
            assert!(it.next().is_none());
        }
    }

    #[test]
    fn iter_remove() {
        {
            let mut l = gen_list(&[23, 42, 57]);

            {
                let mut it = l.iter_mut();
                assert_eq!(it.remove(), None);

                let e = it.next();
                assert_eq!(e.as_ref().unwrap().data, 23);
                assert_eq!(it.remove().unwrap().data, 23);

                let e = it.next();
                assert_eq!(e.as_ref().unwrap().data, 42);
                assert_eq!(it.remove().unwrap().data, 42);

                let e = it.next();
                assert_eq!(e.as_ref().unwrap().data, 57);
                assert_eq!(it.remove().unwrap().data, 57);

                let e = it.next();
                assert_eq!(e, None);
                assert_eq!(it.remove(), None);
            }

            assert!(l.is_empty());
        }

        {
            let mut l = gen_list(&[1, 2, 3]);

            {
                let mut it = l.iter_mut();
                assert_eq!(it.next().as_ref().unwrap().data, 1);
                assert_eq!(it.next().as_ref().unwrap().data, 2);
                assert_eq!(it.remove().unwrap().data, 2);
                assert_eq!(it.remove().unwrap().data, 1);
                assert_eq!(it.remove(), None);
                assert_eq!(it.next().as_ref().unwrap().data, 3);
            }

            assert_eq!(l, gen_list(&[3]));
        }
    }

    #[test]
    fn remove_if() {
        let mut l = gen_list(&[23, 42, 57, 10, 67, 1024]);

        let e = l.remove_if(|e| e.data % 2 == 0).unwrap();
        assert_eq!(e.data, 42);
        assert_eq!(l.len(), 5);

        let e = l.remove_if(|e| e.data == 23).unwrap();
        assert_eq!(e.data, 23);
        assert_eq!(l.len(), 4);

        let e = l.remove_if(|e| e.data > 100).unwrap();
        assert_eq!(e.data, 1024);
        assert_eq!(l.len(), 3);

        assert!(l.remove_if(|e| e.data > 100).is_none());
        assert_eq!(l.len(), 3);

        l.clear();
        assert_eq!(l.len(), 0);
    }

    #[test]
    fn move_back() {
        let mut l = gen_list(&[23, 42, 57, 10, 67, 1024]);

        let e: *mut TestItem = l.iter_mut().nth(3).unwrap();
        unsafe {
            assert_eq!((*e).data, 10);
            l.move_to_back(e);
        }
        assert_eq!(l.len(), 6);
        assert_eq!(l.back().unwrap().data, 10);

        let e: *mut TestItem = l.front_mut().unwrap();
        unsafe {
            assert_eq!((*e).data, 23);
            l.move_to_back(e);
        }
        assert_eq!(l.len(), 6);
        assert_eq!(l.back().unwrap().data, 23);

        let e: *mut TestItem = l.back_mut().unwrap();
        unsafe {
            assert_eq!((*e).data, 23);
            l.move_to_back(e);
        }
        assert_eq!(l.len(), 6);
        assert_eq!(l.back().unwrap().data, 23);

        assert_eq!(
            l.iter().fold(0, |acc, x| acc + x.data),
            23 + 42 + 57 + 10 + 67 + 1024
        );
    }

    #[test]
    fn push_back() {
        let mut l = BoxList::new();

        l.push_back(Box::new(TestItem::new(1)));
        l.push_back(Box::new(TestItem::new(2)));
        l.push_back(Box::new(TestItem::new(3)));

        assert_eq!(l, gen_list(&[1, 2, 3]));
    }

    #[test]
    fn push_front() {
        let mut l = BoxList::new();

        l.push_front(Box::new(TestItem::new(1)));
        l.push_front(Box::new(TestItem::new(2)));
        l.push_front(Box::new(TestItem::new(3)));

        assert_eq!(l, gen_list(&[3, 2, 1]));
    }
}
