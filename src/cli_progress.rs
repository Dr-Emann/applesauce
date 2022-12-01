use applesauce::progress::{Progress, Task};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const DELAY: Duration = Duration::from_millis(700);

pub struct ProgressBars {
    style: ProgressStyle,
    total_bar: ProgressBar,
    bars: MultiProgress,
}

impl ProgressBars {
    pub fn finish(&self) {
        self.total_bar.finish();
    }
}

impl ProgressBars {
    pub fn new() -> Self {
        let bars = MultiProgress::new();
        let total_style = ProgressStyle::with_template(
            "{prefix:>25.bold} {wide_bar:.green} {bytes:>11}/{total_bytes:<11} {eta:6}",
        )
        .unwrap();
        let style = ProgressStyle::with_template(
            "{prefix:>25.dim} {wide_bar} {bytes:>11}/{total_bytes:<11} {eta:6}",
        )
        .unwrap();
        let total_bar = bars
            .add(ProgressBar::new(0))
            .with_style(total_style)
            .with_prefix("Total:");

        Self {
            style,
            total_bar,
            bars,
        }
    }

    pub fn prefix_len(&self) -> usize {
        // We want this to be a method, even though we don't use self
        let _ = self;
        25
    }

    pub fn multi_progress(&self) -> &MultiProgress {
        &self.bars
    }
}

enum State {
    Unattached {
        bars: MultiProgress,
        time_to_attach: Instant,
    },
    Attached,
}

pub struct ProgressWithTotal {
    total: ProgressBar,
    single: ProgressBar,
    state: Mutex<State>,
}

impl ProgressWithTotal {
    fn maybe_attach(&self) {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        if let State::Unattached {
            ref bars,
            time_to_attach,
        } = *state
        {
            if time_to_attach <= now {
                bars.add(self.single.clone());
                *state = State::Attached;
            }
        }
    }
}

impl Progress for ProgressBars {
    type Task = ProgressWithTotal;

    fn sub_task(&self, path: &Path, size: u64) -> Self::Task {
        let prefix = crate::truncate_path(path, self.prefix_len());

        let total = self.total_bar.clone();
        let single = ProgressBar::hidden()
            .with_style(self.style.clone())
            .with_prefix(prefix.to_string_lossy().into_owned());

        single.set_length(size);
        total.inc_length(size);
        ProgressWithTotal {
            total,
            single,
            state: Mutex::new(State::Unattached {
                bars: self.bars.clone(),
                time_to_attach: Instant::now() + DELAY,
            }),
        }
    }
}

impl Task for ProgressWithTotal {
    fn increment(&self, amt: u64) {
        self.total.inc(amt);
        self.single.inc(amt);
        self.maybe_attach();
    }

    fn error(&self, message: &str) {
        self.single.set_message(message.to_string());
        self.maybe_attach();
    }
}

pub struct ProgressBarWriter<W> {
    multi_progress: MultiProgress,
    inner: W,
}

impl<W> ProgressBarWriter<W> {
    pub fn new(multi_progress: MultiProgress, inner: W) -> Self {
        Self {
            multi_progress,
            inner,
        }
    }
}

impl<W: Write> Write for ProgressBarWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.multi_progress.suspend(|| self.inner.write(buf))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.multi_progress.suspend(|| self.inner.flush())
    }
}
