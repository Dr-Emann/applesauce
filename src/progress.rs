use indicatif::ProgressBar;

pub trait Progress {
    fn set_total_length(&self, length: u64);
    fn increment(&self, amt: u64);
    fn message(&self, message: &str);
}

impl Progress for ProgressBar {
    fn set_total_length(&self, length: u64) {
        self.set_length(length);
    }

    fn increment(&self, amt: u64) {
        self.inc(amt);
    }

    fn message(&self, message: &str) {
        self.println(message);
    }
}
