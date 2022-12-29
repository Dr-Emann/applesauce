use std::path::Path;

pub trait Progress {
    type Task: Task;

    fn sub_task(&self, path: &Path, size: u64) -> Self::Task;
}

pub trait Task {
    fn increment(&self, amt: u64);
    fn error(&self, message: &str);
}
