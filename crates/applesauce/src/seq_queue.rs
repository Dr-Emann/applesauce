use std::sync::{Arc, Mutex};
use std::{fmt, io};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct UnknownError;

impl fmt::Display for UnknownError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unspecified error in sender for sequential queue")
    }
}

impl std::error::Error for UnknownError {}

impl From<UnknownError> for io::Error {
    fn from(value: UnknownError) -> Self {
        io::Error::other(value)
    }
}

type FinalSuccessData<E> = Option<Result<(), Option<E>>>;

#[derive(Debug)]
struct FinalSuccess<E>(Arc<Mutex<FinalSuccessData<E>>>);

// Clone doesn't depend on E being clone
impl<E> Clone for FinalSuccess<E> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<E> FinalSuccess<E> {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }

    fn make_success(self) {
        let mutex = &*self.0;
        let mut lock = mutex.lock().unwrap();
        match *lock {
            Some(_) => {}
            None => {
                *lock = Some(Ok(()));
            }
        }
    }

    fn make_unknown_error(self) {
        let mutex = &*self.0;
        let mut lock = mutex.lock().unwrap();
        match *lock {
            Some(Err(Some(_))) => {}
            _ => {
                *lock = Some(Err(None));
            }
        }
    }

    fn make_error(self, error: E) {
        let mutex = &*self.0;
        let mut lock = mutex.lock().unwrap();
        match *lock {
            Some(Err(Some(_))) => {}
            _ => {
                *lock = Some(Err(Some(error)));
            }
        }
    }

    fn get_result(self) -> Result<(), Option<E>> {
        let mutex = &*self.0;
        let mut lock = mutex.lock().unwrap();
        lock.take().unwrap_or_else(|| Err(None))
    }
}

struct FinalErrorOnDrop<E>(Option<FinalSuccess<E>>);

impl<E> FinalErrorOnDrop<E> {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl<E> Drop for FinalErrorOnDrop<E> {
    fn drop(&mut self) {
        if let Some(final_success) = self.0.take() {
            final_success.make_unknown_error();
        }
    }
}

#[derive(Debug)]
pub struct Sender<T, E>(
    crossbeam_channel::Sender<oneshot::Receiver<T>>,
    FinalSuccess<E>,
);

#[derive(Debug)]
pub struct Receiver<T, E>(
    crossbeam_channel::Receiver<oneshot::Receiver<T>>,
    FinalSuccess<E>,
);

pub struct Slot<T, E>(oneshot::Sender<T>, FinalErrorOnDrop<E>);

pub fn bounded<T, E>(cap: usize) -> (Sender<T, E>, Receiver<T, E>) {
    let final_success = FinalSuccess::new();
    let (tx, rx) = crossbeam_channel::bounded(cap);
    (
        Sender(tx, final_success.clone()),
        Receiver(rx, final_success),
    )
}

impl<T, E> Sender<T, E> {
    pub fn prepare_send(&self) -> Option<Slot<T, E>> {
        let (tx, rx) = oneshot::channel();
        self.0.send(rx).ok()?;
        Some(Slot(tx, FinalErrorOnDrop(Some(self.1.clone()))))
    }

    pub fn finish(self, result: Result<(), E>) {
        match result {
            Ok(()) => self.1.make_success(),
            Err(e) => self.1.make_error(e),
        }
    }
}

impl<T, E> Slot<T, E> {
    pub fn finish(self, item: T) -> Result<(), oneshot::SendError<T>> {
        self.1.disarm();
        self.0.send(item)
    }

    pub fn error(self, error: E) {
        let Self(_sender, mut error_on_drop) = self;
        if let Some(final_success) = error_on_drop.0.take() {
            final_success.make_error(error)
        }
    }
}

impl<T, E> Receiver<T, E> {
    pub fn try_for_each(self, mut f: impl FnMut(T) -> Result<(), E>) -> Result<(), E>
    where
        UnknownError: Into<E>,
    {
        loop {
            match self.recv() {
                Ok(result) => {
                    f(result)?;
                }
                Err(_) => {
                    return self
                        .finish()
                        .map_err(|maybe_e| maybe_e.unwrap_or_else(|| UnknownError.into()))
                }
            }
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        let inner_chan = self.0.recv().map_err(|_| RecvError::Finished)?;
        inner_chan.recv().map_err(|_| RecvError::ItemRecvError)
    }

    pub fn finish(self) -> Result<(), Option<E>> {
        let Self(receiver, final_success) = self;
        if receiver.recv().is_ok() {
            tracing::error!("finish on seq queue received an item");
            return Err(None);
        }
        // Make sure to drop the receiver, to make sure the sender won't block trying to send
        // anything
        drop(receiver);
        final_success.get_result()
    }
}

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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
        let (tx, rx) = bounded::<u8, ()>(2);

        let first = tx.prepare_send().unwrap();
        assert_eq!(rx.0.len(), 1);
        let second = tx.prepare_send().unwrap();
        assert_eq!(rx.0.len(), 2);
        tx.finish(Ok(()));

        second.finish(2).unwrap();
        first.finish(1).unwrap();

        assert_eq!(rx.0.len(), 2);

        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv().unwrap(), 2);
        assert_eq!(rx.recv().unwrap_err(), RecvError::Finished);
        assert_eq!(rx.finish(), Ok(()));
    }

    #[test]
    fn no_success_becomes_err() {
        let (tx, rx) = bounded::<u8, ()>(2);

        let first = tx.prepare_send().unwrap();
        first.finish(1).unwrap();
        drop(tx);

        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv().unwrap_err(), RecvError::Finished);
        assert_eq!(rx.finish(), Err(None));
    }

    #[test]
    fn unfinished_send_becomes_err() {
        let (tx, rx) = bounded::<u8, &str>(2);

        let first = tx.prepare_send().unwrap();
        drop(first);
        tx.finish(Ok(()));

        assert_eq!(rx.recv().unwrap_err(), RecvError::ItemRecvError);
        assert_eq!(rx.finish(), Err(None));
    }

    #[test]
    fn explicit_send_err() {
        let (tx, rx) = bounded::<u8, &str>(2);

        let first = tx.prepare_send().unwrap();
        first.finish(1).unwrap();
        tx.finish(Err("error"));

        assert_eq!(rx.recv().unwrap(), 1);
        assert_eq!(rx.recv().unwrap_err(), RecvError::Finished);
        assert_eq!(rx.finish(), Err(Some("error")));
    }

    #[test]
    fn across_threads() {
        let (tx, rx) = bounded::<u32, ()>(2);

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
            tx.finish(Ok(()));
        });

        for i in 0..1000 {
            assert_eq!(rx.recv().unwrap(), i);
        }
        assert_eq!(rx.recv().unwrap_err(), RecvError::Finished);
        assert_eq!(rx.finish(), Ok(()));

        sender_handle.join().unwrap();
    }
}
