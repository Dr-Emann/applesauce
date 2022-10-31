use std::fmt;
use std::fmt::Formatter;

#[derive(Debug)]
pub struct Sender<T>(crossbeam_channel::Sender<crossbeam_channel::Receiver<T>>);

#[derive(Debug)]
pub struct Receiver<T>(crossbeam_channel::Receiver<crossbeam_channel::Receiver<T>>);

pub struct Slot<T>(crossbeam_channel::Sender<T>);

pub fn bounded<T>(cap: usize) -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = crossbeam_channel::bounded(cap);
    (Sender(tx), Receiver(rx))
}

impl<T> Sender<T> {
    pub fn prepare_send(&self) -> Option<Slot<T>> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.0.send(rx).ok()?;
        Some(Slot(tx))
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T> Slot<T> {
    pub fn finish(self, item: T) -> Result<(), crossbeam_channel::SendError<T>> {
        self.0.send(item)
    }
}

impl<T> Receiver<T> {
    pub fn recv(&self) -> Result<T, RecvError> {
        let inner_chan = self.0.recv().map_err(|_| RecvError::Finished)?;
        inner_chan.recv().map_err(|_| RecvError::ItemRecvError)
    }
}

pub struct Iter<'a, T> {
    receiver: &'a Receiver<T>,
}

pub struct IntoIter<T> {
    receiver: Receiver<T>,
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        Iter { receiver: self }
    }
}

impl<'a, T> IntoIterator for &'a mut Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        Iter { receiver: self }
    }
}

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter { receiver: self }
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.recv().ok()
    }
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.receiver.recv().ok()
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct SendError;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum RecvError {
    Finished,
    ItemRecvError,
}

impl RecvError {
    const fn message(self) -> &'static str {
        match self {
            RecvError::Finished => "receiving on an empty and disconnected channel",
            RecvError::ItemRecvError => "item in sequential queue was dropped without completion",
        }
    }
}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for RecvError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn order_after_sending() {
        let (tx, rx) = bounded(2);

        let first = tx.prepare_send().unwrap();
        assert_eq!(rx.0.len(), 1);
        let second = tx.prepare_send().unwrap();
        assert_eq!(rx.0.len(), 2);

        drop(tx);

        second.finish(2).unwrap();
        first.finish(1).unwrap();

        assert_eq!(rx.0.len(), 2);

        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv().unwrap(), 2);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn across_threads() {
        let (tx, rx) = bounded(2);

        let sender_handle = std::thread::spawn(move || {
            let tx = tx;
            for i in 0..1000 {
                let slot = tx.prepare_send().unwrap();
                std::thread::spawn(move || {
                    // slow down some finishes
                    if i % 3 == 0 {
                        std::thread::sleep(Duration::from_micros(10));
                    }
                    slot.finish(i).unwrap();
                });
            }
        });

        for i in 0..1000 {
            assert_eq!(rx.recv().unwrap(), i);
        }
        assert_eq!(rx.recv().unwrap_err(), RecvError::Finished);

        sender_handle.join().unwrap();
    }
}
