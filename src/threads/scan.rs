use ignore::WalkState;
use std::path::Path;

fn do_it<'a>(paths: impl IntoIterator<Item = &'a Path>) {
    let mut paths = paths.into_iter();
    let first = paths.next().expect("at least one path");
    let mut builder = ignore::WalkBuilder::new(first);
    builder.standard_filters(false);
    paths.for_each(|p| {
        builder.add(p);
    });

    let walker = builder.build_parallel();
    walker.run(|| {
        Box::new(|entry| {
            if let Ok(entry) = entry {
                let _ = entry;
            }
            WalkState::Continue
        })
    })
}
