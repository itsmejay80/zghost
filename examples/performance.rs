use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
use std::time::Instant;
use zghost::sources::{Context, ExecutableSource, FileSource, SuggestionSource};

struct Meter;
static BYTES: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        BYTES.fetch_add(layout.size(), Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout);
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        BYTES.fetch_add(size, Relaxed);
        System.realloc(ptr, layout, size)
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

fn main() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..1500 {
        let path = dir.path().join(format!("tool-{i:04}"));
        std::fs::write(&path, "").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::env::set_var("PATH", dir.path());
    let executable = ExecutableSource::default();
    for (name, source, buffer, fuzzy) in [
        (
            "executable cache miss-prefix",
            &executable as &dyn SuggestionSource,
            "absent",
            false,
        ),
        ("selective file prefix", &FileSource, "cat tool-149", false),
        ("fuzzy executable typo", &executable, "tpol-149", true),
    ] {
        let ctx = Context {
            buffer: buffer.into(),
            fuzzy,
            cwd: dir.path().into(),
            prev_command: None,
            now: 1_700_000_000,
            line: zghost::parser::parse(buffer),
        };
        let expected: Vec<_> = source.suggest(&ctx).into_iter().map(|s| s.text).collect();
        let before = BYTES.load(Relaxed);
        let start = Instant::now();
        const QUERIES: usize = 200;
        for _ in 0..QUERIES {
            let actual = source.suggest(black_box(&ctx));
            assert!(actual.iter().map(|s| &s.text).eq(expected.iter()));
            black_box(actual);
        }
        let micros = start.elapsed().as_secs_f64() * 1e6 / QUERIES as f64;
        let allocated = (BYTES.load(Relaxed) - before) / QUERIES;
        println!(
            "{name}: {micros:.1} us/query, {allocated} allocated bytes/query, {} candidates",
            expected.len()
        );
    }
}
