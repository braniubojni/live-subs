use criterion::{black_box, criterion_group, criterion_main, Criterion};
use live_subs::stt::partial_window;

/// Copying the 4 s tail against copying the whole 8 s utterance.
/// No whisper binary: the cost that grows with the partial is the samples sent.
fn partial_copy(c: &mut Criterion) {
    let full = vec![0.25_f32; 16_000 * 8];
    let mut group = c.benchmark_group("partial_window");
    group.bench_function("copy_full_8s", |b| {
        b.iter(|| black_box(black_box(&full).to_vec()));
    });
    group.bench_function("copy_tail_4s", |b| {
        b.iter(|| black_box(partial_window(black_box(&full)).to_vec()));
    });
    group.finish();
}

criterion_group!(benches, partial_copy);
criterion_main!(benches);
