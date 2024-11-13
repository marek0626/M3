/*
 * Copyright (C) 2021-2022 Nils Asmussen, Barkhausen Institut
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

//! Contains utility functions for parsing data types from text

use crate::kif;
use crate::mem::GlobOff;
use crate::time::TimeDuration;

/// Parses an address from the given string
///
/// If the string starts with "0x", the remainder is interpreted hexadecimal, otherwise decimal.
pub fn addr(s: &str) -> Option<GlobOff> {
    if let Some(hex) = s.strip_prefix("0x") {
        GlobOff::from_str_radix(hex, 16)
    }
    else {
        s.parse::<GlobOff>()
    }
    .ok()
}

/// Parses a size from the given string
///
/// The binary prefixes k/K, m/M, and g/G can be used to denote kibibytes, mebibytes, and gibibytes,
/// respectively.
pub fn size(s: &str) -> Option<usize> {
    let mul = match s.chars().last() {
        Some(c) if c >= '0' && c <= '9' => 1,
        Some('k') | Some('K') => 1024,
        Some('m') | Some('M') => 1024 * 1024,
        Some('g') | Some('G') => 1024 * 1024 * 1024,
        _ => return None,
    };
    Some(match mul {
        1 => int(s)? as usize,
        m => m * int(&s[0..s.len() - 1])? as usize,
    })
}

/// Parses a time from the given string
///
/// The suffixes ns, us, ms, and s can be used to denote nanoseconds, microseconds, milliseconds and
/// seconds.
pub fn time(s: &str) -> Option<TimeDuration> {
    let (width, mul) = if s.ends_with("ns") {
        (2, 1)
    }
    else if s.ends_with("us") {
        (2, 1_000)
    }
    else if s.ends_with("ms") {
        (2, 1_000_000)
    }
    else if s.ends_with('s') {
        (1, 1_000_000_000)
    }
    else {
        return None;
    };
    Some(TimeDuration::from_nanos(mul * int(&s[0..s.len() - width])?))
}

/// Parses a u64 from the given string
pub fn int(s: &str) -> Option<u64> {
    s.parse::<u64>().ok()
}

/// Parses a boolean ("true" or "false") from the given string
pub fn bool(s: &str) -> Option<bool> {
    match s {
        "true" => Some(true),
        "false" => Some(false),
        _ => Some(int(s)? == 1),
    }
}

/// Parses permissions from the given string
///
/// Expects arbitrary combinations of the letters 'r', 'w', and 'x' to denote read, write, and
/// execute permission, respectively.
pub fn perm(s: &str) -> Option<kif::Perm> {
    let mut perm = kif::Perm::empty();
    for c in s.chars() {
        match c {
            'r' => perm |= kif::Perm::R,
            'w' => perm |= kif::Perm::W,
            'x' => perm |= kif::Perm::X,
            _ => return None,
        }
    }
    Some(perm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_addr() {
        assert_eq!(addr("0x123"), Some(0x123));
        assert_eq!(addr("106"), Some(106));
        assert!(addr("1.06").is_none());
        assert!(addr("").is_none());
        assert!(addr("abc").is_none());
    }

    #[test]
    fn test_size() {
        assert_eq!(size("0"), Some(0));
        assert_eq!(size("1"), Some(1));
        assert_eq!(size("1k"), Some(1024));
        assert_eq!(size("4K"), Some(4096));
        assert_eq!(size("200M"), Some(200 * 1024 * 1024));
        assert_eq!(size("0m"), Some(0));
        assert_eq!(size("10G"), Some(10 * 1024 * 1024 * 1024));
        assert!(size("").is_none());
        assert!(size("10a").is_none());
        assert!(size("k").is_none());
        assert!(size("-2").is_none());
        assert!(size("2MM").is_none());
        assert!(size("2k ").is_none());
    }

    #[test]
    fn test_time() {
        assert_eq!(time("0s"), Some(TimeDuration::from_secs(0)));
        assert_eq!(time("1s"), Some(TimeDuration::from_secs(1)));
        assert_eq!(time("100ms"), Some(TimeDuration::from_millis(100)));
        assert_eq!(time("10us"), Some(TimeDuration::from_micros(10)));
        assert_eq!(time("2ns"), Some(TimeDuration::from_nanos(2)));
        assert!(time("0").is_none());
        assert!(time("a").is_none());
        assert!(time("10ns ").is_none());
        assert!(time("-2s").is_none());
        assert!(time("").is_none());
    }

    #[test]
    fn test_bool() {
        assert_eq!(bool("true"), Some(true));
        assert_eq!(bool("false"), Some(false));
        assert!(bool("").is_none());
        assert!(bool("t").is_none());
        assert!(bool("f").is_none());
        assert!(bool(" true ").is_none());
        assert!(bool("TRUE").is_none());
    }

    #[test]
    fn test_perm() {
        use kif::Perm;
        assert_eq!(perm("r"), Some(Perm::R));
        assert_eq!(perm("w"), Some(Perm::W));
        assert_eq!(perm("x"), Some(Perm::X));
        assert_eq!(perm("rw"), Some(Perm::RW));
        assert_eq!(perm("xwr"), Some(Perm::RWX));
        assert_eq!(perm("wxr"), Some(Perm::RWX));
        assert_eq!(perm(""), Some(Perm::empty()));
        assert!(perm("k").is_none());
        assert!(perm("rwa").is_none());
    }
}
