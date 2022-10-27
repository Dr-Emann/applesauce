use crossbeam_channel::{Receiver, Sender};
use std::sync::Arc;

pub struct SeqQueue<T> {
    tx: Sender<Receiver<T>>,
    rx: Receiver<Receiver<T>>,
}

impl<T> SeqQueue<T> {
    pub fn new() -> Self {
        let (tx, rx) = crossbeam_channel::bounded(8);
        Self { tx, rx }
    }

    pub fn reserve_slot(&self) -> Slot<T> {
        let (tx, rx) = crossbeam_channel::bounded(1);

        self.tx.send(rx).unwrap();
        Slot { tx }
    }

    // TODO: Result
    pub fn next(&self) -> Option<T> {
        let inner = self.rx.recv().ok()?;
        inner.recv().ok()
    }
}

pub struct Slot<T> {
    tx: Sender<T>,
}
