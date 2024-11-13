/*
 * Copyright (C) 2018 Nils Asmussen <nils@os.inf.tu-dresden.de>
 * Economic rights: Technische Universitaet Dresden (Germany)
 *
 * Copyright (C) 2019-2022 Nils Asmussen, Barkhausen Institut
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

//! Contains the serializing basics, which is used for IPC

mod de;
mod error;
mod ser;

pub use self::de::M3Deserializer;
pub use self::ser::{M3Serializer, Sink, SliceSink, VecSink};
pub use error::SerdeError;
pub use serde::{self, Deserialize, Deserializer, Serialize, Serializer};
pub use serde_bytes as bytes;

use crate::col::{String, Vec};
use crate::libc;

/// Constructs a message with the arguments `$args` into the given message buffer `$msg`
#[macro_export]
macro_rules! build_vmsg {
    ( $msg:expr, $( $args:expr ),* ) => ({
        // safety: we initialize these bytes below
        let sink = unsafe { $crate::serialize::SliceSink::new($msg.words_mut()) };
        let mut ser = $crate::serialize::M3Serializer::new(sink);
        $( ser.push(&$args); )*
        let bytes = ser.size();
        // safety: we just have initialized these bytes
        unsafe { $msg.set_size(bytes) };
    });
}

/// Copies the given string into the given word slice
///
/// # Safety
///
/// Assumes that words has sufficient space
pub unsafe fn copy_from_str(words: &mut [u64], s: &str) {
    let bytes = words.as_mut_ptr() as *mut u8;
    libc::memcpy(
        bytes as *mut libc::c_void,
        s.as_bytes().as_ptr() as *const libc::c_void,
        s.len(),
    );
    // null termination
    *bytes.add(s.len()) = 0u8;
}

/// Copies a string of given length from the given slice
///
/// # Safety
///
/// Assumes that `s` points to a valid string of given length
#[allow(clippy::uninit_vec)]
pub unsafe fn copy_str_from(s: &[u64], len: usize) -> String {
    let mut v = Vec::<u8>::with_capacity(len);
    // we deliberately use uninitialize memory here, because it's performance critical
    // safety: this is okay, because libc::memcpy (our implementation) does not read from `dst`
    v.set_len(len);
    let src = s.as_ptr() as *mut libc::c_void;
    let dst = v.as_mut_ptr() as *mut _ as *mut libc::c_void;
    libc::memcpy(dst, src, len);
    String::from_utf8(v).unwrap()
}

/// Returns a reference to the string in the given slice of given length
///
/// # Safety
///
/// Assumes that `s` points to a valid string of given length
pub unsafe fn str_slice_from(s: &[u64], len: usize) -> &'static str {
    let slice = core::slice::from_raw_parts(s.as_ptr() as *const u8, len);
    core::str::from_utf8(slice).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vec;

    #[test]
    fn basics() {
        let mut vec = vec![];
        let mut ser = M3Serializer::new(VecSink::new(&mut vec));
        ser.push(1u8);
        ser.push(2i8);
        ser.push(3u16);
        ser.push(4i16);
        ser.push(5u32);
        ser.push(6i32);
        ser.push(7u64);
        ser.push(8i64);
        ser.push(9.5f32);
        ser.push(10.8f64);
        ser.push('a');
        ser.push(true);
        ser.push(());
        ser.push::<Option<i32>>(None);
        ser.push(Some(42));

        let mut de = M3Deserializer::new(&vec);
        assert_eq!(de.pop::<u8>(), Ok(1u8));
        assert_eq!(de.pop::<i8>(), Ok(2i8));
        assert_eq!(de.pop::<u16>(), Ok(3u16));
        assert_eq!(de.pop::<i16>(), Ok(4i16));
        assert_eq!(de.pop::<u32>(), Ok(5u32));
        assert_eq!(de.pop::<i32>(), Ok(6i32));
        assert_eq!(de.pop::<u64>(), Ok(7u64));
        assert_eq!(de.pop::<i64>(), Ok(8i64));
        assert_eq!(de.pop::<f32>(), Ok(9.5f32));
        assert_eq!(de.pop::<f64>(), Ok(10.8f64));
        assert_eq!(de.pop::<char>(), Ok('a'));
        assert_eq!(de.pop::<bool>(), Ok(true));
        assert_eq!(de.pop::<()>(), Ok(()));
        assert_eq!(de.pop::<Option<i32>>(), Ok(None));
        assert_eq!(de.pop::<Option<i32>>(), Ok(Some(42)));
        assert!(de.pop::<u32>().is_err());
    }

    #[test]
    fn slice_sink() {
        let mut buf = [0u64; 128];
        let mut ser = M3Serializer::new(SliceSink::new(&mut buf));

        ser.push(42);
        ser.push("test");
        ser.push(serde_bytes::Bytes::new(&[8, 7, 6]));

        let mut de = M3Deserializer::new(&buf);
        assert_eq!(de.pop::<u32>(), Ok(42));
        assert_eq!(de.pop::<&str>(), Ok("test"));
        assert_eq!(
            de.pop::<&serde_bytes::Bytes>().unwrap(),
            &serde_bytes::Bytes::new(&[8, 7, 6])
        );
    }

    #[test]
    fn strings() {
        let mut vec = vec![];
        let mut ser = M3Serializer::new(VecSink::new(&mut vec));
        ser.push("foo");
        ser.push(String::from("bar"));

        let mut de = M3Deserializer::new(&vec);
        assert_eq!(de.pop::<&str>(), Ok("foo"));
        assert_eq!(de.pop::<String>(), Ok(String::from("bar")));
    }

    #[test]
    fn sequences() {
        let mut vec = vec![];
        let mut ser = M3Serializer::new(VecSink::new(&mut vec));
        ser.push((1, 2, 3));
        ser.push(vec![4, 5, 6]);

        let mut de = M3Deserializer::new(&vec);
        assert_eq!(de.pop::<(_, _, _)>(), Ok((1, 2, 3)));
        assert_eq!(de.pop::<Vec<_>>(), Ok(vec![4, 5, 6]));
    }

    #[test]
    fn bytes() {
        let mut vec = vec![];
        let mut ser = M3Serializer::new(VecSink::new(&mut vec));
        let buf = serde_bytes::Bytes::new(&[0, 5, 8, 10]);
        ser.push(&buf);

        let mut de = M3Deserializer::new(&vec);
        assert_eq!(
            de.pop::<&serde_bytes::Bytes>().unwrap(),
            &serde_bytes::Bytes::new(&[0, 5, 8, 10])
        );
    }

    #[test]
    fn byte_buf() {
        let mut vec = vec![];
        let mut ser = M3Serializer::new(VecSink::new(&mut vec));
        let mut buf = serde_bytes::ByteBuf::new();
        buf.push(42);
        buf.push(23);
        buf.push(100);
        ser.push(buf);

        let mut de = M3Deserializer::new(&vec);
        assert_eq!(de.pop::<serde_bytes::ByteBuf>().unwrap().into_vec(), vec![
            42, 23, 100
        ]);
    }

    #[test]
    fn structs() {
        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        struct Foo {
            a: u32,
            b: bool,
            c: String,
        }

        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        struct FooUnit;

        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        struct FooNewType(u32);

        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        struct FooTupleStruct(u32, bool, u8);

        let mut vec = vec![];
        let mut ser = M3Serializer::new(VecSink::new(&mut vec));
        ser.push(Foo {
            a: 1,
            b: true,
            c: String::from("test"),
        });
        ser.push(FooUnit);
        ser.push(FooNewType(14));
        ser.push(FooTupleStruct(4, true, 16));

        let mut de = M3Deserializer::new(&vec);
        assert_eq!(
            de.pop::<Foo>(),
            Ok(Foo {
                a: 1,
                b: true,
                c: String::from("test")
            })
        );
        assert_eq!(de.pop::<FooUnit>(), Ok(FooUnit));
        assert_eq!(de.pop::<FooNewType>(), Ok(FooNewType(14)));
        assert_eq!(de.pop::<FooTupleStruct>(), Ok(FooTupleStruct(4, true, 16)));
    }

    #[test]
    fn enums() {
        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        enum Bar {
            A,
            B,
        }

        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        enum Zoo {
            A(u32),
            B(bool),
        }

        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        enum ZooTupleVariant {
            A(u32, u64),
            B(bool, u8),
        }

        #[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
        enum Zar {
            A { a: u8, b: usize },
            B { c: String },
        }

        let mut vec = vec![];
        let mut ser = M3Serializer::new(VecSink::new(&mut vec));
        ser.push(Bar::A);
        ser.push(Bar::B);
        ser.push(Zoo::A(2));
        ser.push(Zoo::B(false));
        ser.push(ZooTupleVariant::A(0, 10));
        ser.push(ZooTupleVariant::B(true, 255));
        ser.push(Zar::A { a: 4, b: 6 });
        ser.push(Zar::B {
            c: String::from("zar"),
        });

        let mut de = M3Deserializer::new(&vec);
        assert_eq!(de.pop::<Bar>(), Ok(Bar::A));
        assert_eq!(de.pop::<Bar>(), Ok(Bar::B));
        assert_eq!(de.pop::<Zoo>(), Ok(Zoo::A(2)));
        assert_eq!(de.pop::<Zoo>(), Ok(Zoo::B(false)));
        assert_eq!(de.pop::<ZooTupleVariant>(), Ok(ZooTupleVariant::A(0, 10)));
        assert_eq!(
            de.pop::<ZooTupleVariant>(),
            Ok(ZooTupleVariant::B(true, 255))
        );
        assert_eq!(de.pop::<Zar>(), Ok(Zar::A { a: 4, b: 6 }));
        assert_eq!(
            de.pop::<Zar>(),
            Ok(Zar::B {
                c: String::from("zar")
            })
        );
    }
}
