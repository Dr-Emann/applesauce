pub trait Progress {
    fn set_total_length(&self, length: u64);
    fn increment(&self, amt: u64);
    fn message(&self, message: &str);
}

impl Progress for indicatif::ProgressBar {
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

pub struct ProgressWithTotal {
    total: indicatif::ProgressBar,
    single: indicatif::ProgressBar,
}

impl ProgressWithTotal {
    pub fn new(total: indicatif::ProgressBar, single: indicatif::ProgressBar) -> Self {
        Self { total, single }
    }
}

impl Progress for ProgressWithTotal {
    fn set_total_length(&self, length: u64) {
        self.total.inc_length(length);
        self.single.set_length(length);
    }

    fn increment(&self, amt: u64) {
        self.total.inc(amt);
        self.single.inc(amt);
    }

    fn message(&self, message: &str) {
        self.single.set_message(message.to_string());
    }
}
