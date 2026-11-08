//! Shared pending ownership policy; native ChildTrack supplies exact-process observations.
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedExit {
    pub code: Option<i32>,
}

type Receipt = Arc<Mutex<Option<ObservedExit>>>;

pub struct PendingRegistry<T> {
    entries: Vec<(T, Receipt)>,
}

impl<T> PendingRegistry<T> {
    pub const fn new() -> Self { Self { entries: Vec::new() } }

    pub fn len(&self) -> usize { self.entries.len() }

    pub fn replacement_allowed(&self) -> bool { self.entries.is_empty() }

    /// Each callback is one nonblocking observation of the owned resource, never a PID lookup.
    pub fn poll(&mut self, mut observe: impl FnMut(&mut T) -> Option<ObservedExit>) {
        self.entries.retain_mut(|(owned, receipt)| {
            if let Some(exit) = observe(owned) {
                *receipt.lock().unwrap_or_else(|e| e.into_inner()) = Some(exit);
                false
            } else {
                true
            }
        });
    }
}

/// The resource moves to its registry; a live diagnostic owner keeps only this exit receipt.
pub struct RetainedChild<T> {
    pub(crate) owned: Option<T>,
    receipt: Option<Receipt>,
}

impl<T> RetainedChild<T> {
    pub fn new(owned: T) -> Self { Self { owned: Some(owned), receipt: None } }

    pub fn retain_pending(&mut self, registry: &mut PendingRegistry<T>) {
        if let Some(owned) = self.owned.take() {
            let receipt = Arc::new(Mutex::new(None));
            registry.entries.push((owned, receipt.clone()));
            self.receipt = Some(receipt);
        }
    }

    #[cfg(windows)]
    pub(crate) fn registered(&self) -> bool { self.receipt.is_some() }

    pub fn cleanup_pending(&self) -> bool {
        self.receipt.as_ref().is_some_and(|receipt| {
            receipt.lock().unwrap_or_else(|e| e.into_inner()).is_none()
        })
    }

    pub fn poll_pending(&mut self, registry: &mut PendingRegistry<T>,
        observe: impl FnMut(&mut T) -> Option<ObservedExit>) -> Option<ObservedExit> {
        registry.poll(observe);
        let exit = *self.receipt.as_ref()?.lock().unwrap_or_else(|e| e.into_inner());
        if exit.is_some() { self.receipt = None; }
        exit
    }
}
