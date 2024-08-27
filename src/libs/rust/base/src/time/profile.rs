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

//! Contains types to simplify profiling

use core::fmt;

use crate::col::Vec;
use crate::time::{Duration, Instant};
use crate::util::math;

/// A container for the measured execution times
#[derive(Clone)]
pub struct Results<T: Duration> {
    times: Vec<T>,
}

impl<T: Duration> Results<T> {
    /// Creates an empty result container for the given number of runs
    pub fn new(runs: usize) -> Self {
        Results {
            times: Vec::with_capacity(runs),
        }
    }

    /// Returns the vector with all measured times
    pub fn times(&self) -> &Vec<T> {
        &self.times
    }

    /// Pushes the given time to the container
    pub fn push(&mut self, time: T) {
        self.times.push(time);
    }

    /// Returns the number of runs
    pub fn runs(&self) -> usize {
        self.times.len()
    }

    /// Returns the arithmetic mean of the runtimes
    pub fn avg(&self) -> T {
        let mut sum = 0;
        for t in &self.times {
            sum += t.as_raw();
        }
        if self.times.is_empty() {
            T::from_raw(sum)
        }
        else {
            T::from_raw(sum / (self.times.len() as u64))
        }
    }

    /// Returns the standard deviation of the runtimes
    pub fn stddev(&self) -> T {
        let mut sum = 0;
        let average = self.avg().as_raw();
        for t in &self.times {
            let val = if t.as_raw() < average {
                average - t.as_raw()
            }
            else {
                t.as_raw() - average
            };
            sum += val * val;
        }
        if self.times.is_empty() {
            T::from_raw(0)
        }
        else {
            T::from_raw(math::sqrt((sum as f32) / (self.times.len() as f32)) as u64)
        }
    }
}

impl<T: Duration> fmt::Display for Results<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?} (+/- {:?} with {} runs)",
            self.avg(),
            self.stddev(),
            self.runs(),
        )
    }
}

/// Allows to measure execution times
///
/// # Examples
///
/// Simple usage:
///
/// ```no_run
/// use base::time::{CycleInstant, Profiler};
///
/// let mut prof = Profiler::default();
/// println!("{}", prof.run::<CycleInstant, _>(|| { /* my benchmark */ }));
/// ```
///
/// Advanced usage:
///
/// ```no_run
/// use base::time::{CycleInstant, Runner, Profiler};
///
/// #[derive(Default)]
/// struct Tester();
///
/// impl Runner for Tester {
///     fn run(&mut self) {
///         // my benchmark
///     }
///     fn post(&mut self) {
///         // my cleanup action
///     }
/// }
///
/// let mut prof = Profiler::default().repeats(10).warmup(2);
/// println!("{}", prof.runner::<CycleInstant, _>(&mut Tester::default()));
/// ```
pub struct Profiler {
    repeats: u64,
    warmup: u64,
}

/// A runner is used to run the benchmarks and allows to perform pre- and post-actions.
pub trait Runner {
    /// Is executed before the benchmark
    fn pre(&mut self) {
    }

    /// Executes the benchmark
    fn run(&mut self);

    /// Is executed after the benchmark
    fn post(&mut self) {
    }
}

impl Profiler {
    /// Sets the number of runs to `repeats`
    pub fn repeats(mut self, repeats: u64) -> Self {
        self.repeats = repeats;
        self
    }

    /// Sets the number of warmup runs to `warmup`
    pub fn warmup(mut self, warmup: u64) -> Self {
        self.warmup = warmup;
        self
    }

    /// Runs `func` as benchmark and returns the result
    #[inline(always)]
    pub fn run<T: Instant, F: FnMut()>(&self, mut func: F) -> Results<T::Duration> {
        let mut res = Results::new((self.warmup + self.repeats) as usize);
        for i in 0..self.warmup + self.repeats {
            let start = T::now();
            func();
            let end = T::now();

            if i >= self.warmup {
                res.push(end.duration_since(start));
            }
        }
        res
    }

    /// Runs the given runner as benchmark and returns the result
    #[inline(always)]
    pub fn runner<T: Instant, R: Runner>(&self, runner: &mut R) -> Results<T::Duration> {
        let mut res = Results::new((self.warmup + self.repeats) as usize);
        for i in 0..self.warmup + self.repeats {
            runner.pre();

            let start = T::now();
            runner.run();
            let end = T::now();

            runner.post();

            if i >= self.warmup {
                res.push(end.duration_since(start));
            }
        }
        res
    }
}

impl Default for Profiler {
    /// Creates a default profiler with 100 runs and 10 warmup runs
    fn default() -> Self {
        Profiler {
            repeats: 100,
            warmup: 10,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::StaticCell;
    use crate::time::CycleDuration;

    static NEXT_TIME: StaticCell<u64> = StaticCell::new(0);

    struct MyInstant(u64);

    impl Instant for MyInstant {
        type Duration = CycleDuration;

        fn now() -> Self {
            NEXT_TIME.set(NEXT_TIME.get() + 10);
            MyInstant(NEXT_TIME.get() - 10)
        }

        fn duration_since(&self, earlier: Self) -> Self::Duration {
            CycleDuration::new(self.0 - earlier.0)
        }
    }

    struct MyRunner(u64, u64, u64);
    impl Runner for MyRunner {
        fn pre(&mut self) {
            self.0 += 1;
        }

        fn run(&mut self) {
            self.1 += 1;
        }

        fn post(&mut self) {
            self.2 += 1;
        }
    }

    #[test]
    fn run() {
        let prof = Profiler::default().warmup(2).repeats(5);
        let res = prof.run::<MyInstant, _>(|| {});
        for r in res.times() {
            assert_eq!(r.as_raw(), 10);
        }
        assert_eq!(res.avg().as_raw(), 10);
        assert_eq!(res.stddev().as_raw(), 0);
        assert_eq!(res.runs(), 5);
        assert_eq!(
            crate::format!("{}", res),
            "10 cycles (+/- 0 cycles with 5 runs)"
        );
    }

    #[test]
    fn runner() {
        let prof = Profiler::default().warmup(2).repeats(4);
        let mut runner = MyRunner(0, 0, 0);
        let res = prof.runner::<MyInstant, _>(&mut runner);
        assert_eq!(runner.0, 6);
        assert_eq!(runner.1, 6);
        assert_eq!(runner.2, 6);
        for r in res.times() {
            assert_eq!(r.as_raw(), 10);
        }
        assert_eq!(res.avg().as_raw(), 10);
        assert_eq!(res.stddev().as_raw(), 0);
        assert_eq!(res.runs(), 4);
    }
}
