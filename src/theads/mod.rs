use std::thread::JoinHandle;

//pub mod compressing;
//pub mod reader;
pub mod writer;

struct ThreadJoiner {
    threads: Vec<JoinHandle<()>>,
}

impl ThreadJoiner {
    fn new(threads: Vec<JoinHandle<()>>) -> Self {
        Self { threads }
    }
}

impl Drop for ThreadJoiner {
    fn drop(&mut self) {
        for handle in self.threads.drain(..) {
            handle.join().unwrap();
        }
    }
}
