//! Small shared utilities.
//!
//! `par_map` is the one parallelism primitive the gen path uses: a **pure index→value map** whose
//! result is in index order, so `par_map(n, f)[i] == f(i)` exactly — native (rayon) and wasm (serial)
//! produce **bit-identical** output. Only ever apply it to loops with no inter-element dependency
//! (the determinism contract, ADR 0001, hinges on this: parallel output must equal serial output).

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn par_map<T, F>(n: usize, f: F) -> Vec<T>
where
    T: Send,
    F: Fn(usize) -> T + Send + Sync,
{
    use rayon::prelude::*;
    (0..n).into_par_iter().map(f).collect()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn par_map<T, F>(n: usize, f: F) -> Vec<T>
where
    F: Fn(usize) -> T,
{
    (0..n).map(f).collect()
}
