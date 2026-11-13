// SPDX-License-Identifier: GPL-3.0-or-later
//! Classification benchmarks (PLAN §19 Phase 1: under 100 ns for 9 pixels; §3: classify plus
//! the trigger decision under 2 µs). Run with `cargo bench -p glint-core`.

use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{Criterion, criterion_group, criterion_main};
use glint_core::color::{ColorModel, ColorRules, CompiledRules, ShadePreset};
use glint_core::{Lut, Rgb, SamplePattern, Trigger, TriggerCfg, Verdict};

const TARGET: u32 = 0x3B_D1_6F;
const BACKGROUND: u32 = 0x1E_2A_20;

/// Appendix B's color rules.
fn rules() -> CompiledRules {
    CompiledRules::new(&ColorRules {
        model: ColorModel::Oklch,
        exit_margin: 1.5,
        include: vec![ShadePreset::Normal.entry(ColorModel::Oklch, Rgb::from_u32(TARGET))],
        exclude: vec![ShadePreset::Tight.entry(ColorModel::Oklch, Rgb::from_u32(0x6B_D1_3B))],
    })
}

/// A small deterministic generator, so the benchmark needs no extra dependency.
fn lcg(state: &mut u64) -> u32 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*state >> 40) as u32
}

fn classify(c: &mut Criterion) {
    let lut = Lut::build(&rules());
    let pattern = SamplePattern::from_profile(Default::default(), None, &[], None, None).expect("the 3×3 pattern");
    let present = [TARGET; 9];
    let absent = [BACKGROUND; 9];
    let edge = [
        TARGET, TARGET, BACKGROUND, TARGET, TARGET, BACKGROUND, 0x2D_A6_57, BACKGROUND, BACKGROUND,
    ];
    assert_eq!(pattern.verdict(&lut, &present), Verdict::Enter);
    assert_eq!(pattern.verdict(&lut, &absent), Verdict::Absent);

    c.bench_function("verdict 3x3, color present", |b| {
        b.iter(|| pattern.verdict(black_box(&lut), black_box(&present)))
    });
    c.bench_function("verdict 3x3, color absent", |b| {
        b.iter(|| pattern.verdict(black_box(&lut), black_box(&absent)))
    });
    c.bench_function("verdict 3x3, edge of an appearance", |b| {
        b.iter(|| pattern.verdict(black_box(&lut), black_box(&edge)))
    });

    // Worst case: every pixel a different random color, so most lookups miss the cache.
    let mut seed = 7;
    let captures: Vec<[u32; 9]> = (0..4096)
        .map(|_| std::array::from_fn(|_| lcg(&mut seed) & 0xFF_FFFF))
        .collect();
    c.bench_function("verdict 3x3, random colors (cache misses)", |b| {
        let mut i = 0;
        b.iter(|| {
            i = (i + 1) & 4095;
            pattern.verdict(black_box(&lut), black_box(&captures[i]))
        })
    });

    // One whole check after the capture: pixel agreement and the trigger decision.
    c.bench_function("verdict + trigger step", |b| {
        let mut trigger = Trigger::new(TriggerCfg::default());
        trigger.start(Instant::now());
        let mut now = Instant::now();
        let mut i = 0usize;
        b.iter(|| {
            now += Duration::from_micros(500);
            i = i.wrapping_add(1);
            // 60 ms on, 60 ms off: appearances, re-arming and clicks all happen.
            let px = if (i / 120).is_multiple_of(2) { &present } else { &absent };
            let v = pattern.verdict(black_box(&lut), black_box(px));
            trigger.step(now, v)
        })
    });
}

fn build(c: &mut Criterion) {
    let rules = rules();
    let mut group = c.benchmark_group("lookup table");
    group.sample_size(10);
    group.bench_function("build (16.7 M colors, all threads)", |b| {
        b.iter(|| Lut::build(black_box(&rules)))
    });
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().warm_up_time(Duration::from_secs(1)).measurement_time(Duration::from_secs(3));
    targets = classify, build
}
criterion_main!(benches);
